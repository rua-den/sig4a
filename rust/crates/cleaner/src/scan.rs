//! Pure read-only scanner. No function in this module writes, moves or deletes anything.

use crate::fs::{
    canonical_for, display, is_removable, is_unc, path_contains, strip_verbatim, REPARSE_POINT,
};
use crate::model::{EntryInfo, FsView, HeavyEntry, Item, Manifest, RuleKind, ScanOptions};
use std::cmp::{Ordering, Reverse};
use std::collections::{BinaryHeap, HashMap};
use std::path::{Path, PathBuf};

/// Biểu đồ chỉ giữ 8 mục nặng nhất cho mỗi loại.
const TOP_LIMIT: usize = 8;

/// Comparison key for the top-files heap: larger size is "greater"; on equal size the
/// lexicographically smaller path wins so results never depend on traversal order.
#[derive(Debug, Clone, PartialEq, Eq)]
struct HeavyCandidate {
    size: u64,
    path: String,
}

impl Ord for HeavyCandidate {
    fn cmp(&self, other: &Self) -> Ordering {
        self.size
            .cmp(&other.size)
            .then_with(|| other.path.cmp(&self.path))
    }
}

impl PartialOrd for HeavyCandidate {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Bounded min-heap (via [`Reverse`]) of the heaviest files seen so far. It never holds more than
/// [`TOP_LIMIT`] entries, so walking a large tree does not require sorting it in memory.
#[derive(Default)]
struct TopHeap(BinaryHeap<Reverse<HeavyCandidate>>);

impl TopHeap {
    fn offer(&mut self, path: String, size: u64) {
        let candidate = HeavyCandidate { size, path };
        if self.0.len() < TOP_LIMIT {
            self.0.push(Reverse(candidate));
            return;
        }
        // The heap root is the least desirable of the eight kept entries.
        if self.0.peek().is_some_and(|worst| candidate > worst.0) {
            self.0.pop();
            self.0.push(Reverse(candidate));
        }
    }

    /// Drain into a manifest-ready, deterministically ordered (size desc, path asc) vector.
    fn take(&mut self) -> Vec<HeavyEntry> {
        let mut kept: Vec<HeavyCandidate> = std::mem::take(&mut self.0)
            .into_iter()
            .map(|Reverse(c)| c)
            .collect();
        kept.sort_by(|a, b| b.cmp(a));
        kept.into_iter()
            .map(|c| HeavyEntry {
                path: c.path,
                size: c.size,
            })
            .collect()
    }
}

/// Group matched items by the direct parent of their path (`proj\bin` + `proj\obj` → `proj`),
/// sum their sizes and return the 8 heaviest folders, ties broken by path ascending.
fn top_folders_from_items(items: &[Item]) -> Vec<HeavyEntry> {
    let mut sums: HashMap<String, u64> = HashMap::new();
    for item in items {
        let Some(parent) = Path::new(&item.path).parent().and_then(Path::to_str) else {
            continue;
        };
        if parent.is_empty() {
            continue;
        }
        let slot = sums.entry(parent.to_string()).or_insert(0);
        *slot = slot.saturating_add(item.size);
    }
    let mut folders: Vec<HeavyEntry> = sums
        .into_iter()
        .map(|(path, size)| HeavyEntry { path, size })
        .collect();
    folders.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.path.cmp(&b.path)));
    folders.truncate(TOP_LIMIT);
    folders
}

enum Candidate {
    None,
    /// TestResults without a project marker: needs a `.trx` file inside to qualify.
    NeedTrx,
    Rule(RuleKind),
}

#[derive(Debug, Clone, Copy)]
struct DirStats {
    size: u64,
    newest: Option<u64>,
    has_trx: bool,
}

struct Scanner<'a> {
    opts: &'a ScanOptions,
    fs: &'a dyn FsView,
    /// Caller's forbidden paths, canonicalised once so every comparison is component-wise.
    forbidden: Vec<PathBuf>,
    manifest: Manifest,
    /// Bounded set of the heaviest files found while computing directory sizes.
    top: TopHeap,
    dirs: usize,
    out_of_budget: bool,
}

pub fn scan(opts: &ScanOptions, fs: &dyn FsView) -> Manifest {
    let forbidden = opts.forbidden.iter().flat_map(|p| [p.clone(), canonical_for(p)]).collect();
    let mut scanner = Scanner {
        opts,
        fs,
        forbidden,
        manifest: Manifest::new(),
        top: TopHeap::default(),
        dirs: 0,
        out_of_budget: false,
    };
    scanner.manifest.created_at = opts.now_secs;
    let validated: Vec<PathBuf> = opts.roots.iter().filter_map(|raw| scanner.validated_root(raw)).collect();
    for (i, root) in validated.iter().enumerate() {
        if scanner.out_of_budget {
            break;
        }
        let already_covered = validated.iter().enumerate().any(|(j, other)| {
            i != j && path_contains(other, root) && (j < i || !path_contains(root, other))
        });
        if !already_covered {
            scanner.manifest.roots.push(display(root));
            scanner.walk(root.clone());
        }
    }
    // UI wants the top items first. Sorting does not change total/matched.
    scanner
        .manifest
        .items
        .sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.path.cmp(&b.path)));
    // Chart data is derived only from what was actually scanned; counters/items stay untouched.
    let top_files = scanner.top.take();
    let top_folders = top_folders_from_items(&scanner.manifest.items);
    scanner.manifest.top_files = top_files;
    scanner.manifest.top_folders = top_folders;
    scanner.manifest
}

fn eq_name(name: &str, expected: &str) -> bool {
    name.eq_ignore_ascii_case(expected)
}

fn has_extension(name: &str, ext: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case(ext))
}

fn has_project_marker(entries: &[EntryInfo]) -> bool {
    entries.iter().any(|e| {
        !e.is_dir
            && ["csproj", "fsproj", "vbproj"]
                .iter()
                .any(|ext| has_extension(&e.name, ext))
    })
}

fn has_package_json(entries: &[EntryInfo]) -> bool {
    entries
        .iter()
        .any(|e| !e.is_dir && eq_name(&e.name, "package.json"))
}

/// Decide a rule from the entry name and the entries of its **parent** directory.
fn candidate_rule(name: &str, parent_entries: &[EntryInfo]) -> Candidate {
    if eq_name(name, ".vs") {
        return Candidate::Rule(RuleKind::Vs);
    }
    if eq_name(name, "bin") {
        return if has_project_marker(parent_entries) {
            Candidate::Rule(RuleKind::Bin)
        } else {
            Candidate::None
        };
    }
    if eq_name(name, "obj") {
        return if has_project_marker(parent_entries) {
            Candidate::Rule(RuleKind::Obj)
        } else {
            Candidate::None
        };
    }
    if eq_name(name, "node_modules") {
        return if has_package_json(parent_entries) {
            Candidate::Rule(RuleKind::NodeModules)
        } else {
            Candidate::None
        };
    }
    if eq_name(name, "TestResults") {
        return if has_project_marker(parent_entries) {
            Candidate::Rule(RuleKind::TestResults)
        } else {
            Candidate::NeedTrx
        };
    }
    Candidate::None
}

/// Check that a rule still matches the **name** of a path. Kept public for callers that only need
/// the name check; the cleanup engine deliberately uses the stronger [`validate_item_kind`] which
/// also re-checks the guard.
pub fn rule_matches_name(kind: RuleKind, path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    match kind {
        RuleKind::Vs => eq_name(name, ".vs"),
        RuleKind::Bin => eq_name(name, "bin"),
        RuleKind::Obj => eq_name(name, "obj"),
        RuleKind::NodeModules => eq_name(name, "node_modules"),
        RuleKind::TestResults => eq_name(name, "TestResults"),
    }
}

/// True for a `.trx` result file. Shared by the scanner's byte-count pass and the cleanup re-check
/// so the "TestResults without a project marker" guard means the same thing in both phases.
fn is_trx_entry(entry: &EntryInfo) -> bool {
    !entry.is_dir && has_extension(&entry.name, "trx")
}

/// Re-validate a manifest item's rule kind against the current filesystem. The rule must still
/// match the directory name **and** its guard must still hold: a project marker for `bin`/`obj`,
/// a sibling `package.json` for `node_modules`, and a project marker or a `.trx` result for
/// `TestResults`. Reads only through `fs`; it never writes or deletes anything.
///
/// This is the injectable counterpart used by the cleanup engine so it applies exactly the same
/// classification rules as the scan, including the guards that `rule_matches_name` alone misses.
pub fn validate_item_kind(item: &Item, fs: &dyn FsView) -> bool {
    let path = Path::new(&item.path);
    if !rule_matches_name(item.kind, path) {
        return false;
    }
    let Some(parent) = path.parent() else {
        return false;
    };
    let Ok(parent_entries) = fs.entries(parent) else {
        return false;
    };
    match item.kind {
        RuleKind::Vs => true,
        RuleKind::Bin | RuleKind::Obj => has_project_marker(&parent_entries),
        RuleKind::NodeModules => has_package_json(&parent_entries),
        RuleKind::TestResults => has_project_marker(&parent_entries) || dir_has_trx(fs, path),
    }
}

/// Recursive `.trx` presence check, mirroring `Scanner::stats_of`'s `has_trx` accumulation but
/// read-only through an arbitrary [`FsView`]. Reparse points and `.git` are skipped just like the
/// scanner skips them.
fn dir_has_trx(fs: &dyn FsView, dir: &Path) -> bool {
    let Ok(entries) = fs.entries(dir) else {
        return false;
    };
    let mut stack: Vec<PathBuf> = Vec::new();
    for entry in &entries {
        if is_trx_entry(entry) {
            return true;
        }
        if entry.is_dir && entry.attrs & REPARSE_POINT == 0 && !eq_name(&entry.name, ".git") {
            stack.push(entry.path.clone());
        }
    }
    while let Some(current) = stack.pop() {
        let Ok(children) = fs.entries(&current) else {
            continue;
        };
        for entry in &children {
            if is_trx_entry(entry) {
                return true;
            }
            if entry.is_dir && entry.attrs & REPARSE_POINT == 0 && !eq_name(&entry.name, ".git") {
                stack.push(entry.path.clone());
            }
        }
    }
    false
}

pub(crate) fn is_recent(newest: Option<u64>, days: u64, now: u64) -> bool {
    let Some(newest) = newest else { return false };
    let window = days.saturating_mul(86_400);
    newest.saturating_add(window) > now
}

impl<'a> Scanner<'a> {
    fn error(&mut self, message: String) {
        self.manifest.errors.push(message);
    }

    /// True when a path sits inside ANY caller-forbidden subtree — including when the forbidden
    /// subtree itself lives inside the root currently being scanned.
    fn is_forbidden(&self, path: &Path) -> bool {
        self.forbidden.iter().any(|f| path_contains(f, path))
    }

    fn truncate(&mut self, what: &str) {
        let limit = if what == "max_dirs" {
            self.opts.max_dirs
        } else {
            self.opts.max_items
        };
        let message =
            format!("scan truncated: budget {what} ({limit}) reached; results are incomplete");
        self.out_of_budget = true;
        if !self.manifest.errors.contains(&message) {
            self.manifest.errors.push(message);
        }
    }

    /// Count one directory enumeration. Returns false when the budget is exhausted.
    fn bump_dir(&mut self) -> bool {
        if self.dirs >= self.opts.max_dirs {
            self.truncate("max_dirs");
            return false;
        }
        self.dirs += 1;
        self.manifest.scanned += 1;
        true
    }

    fn validated_root(&mut self, raw: &Path) -> Option<PathBuf> {
        if !raw.is_absolute() {
            self.error(format!(
                "invalid root '{}': root is not absolute",
                display(raw)
            ));
            return None;
        }
        if is_unc(raw) {
            self.error(format!(
                "invalid root '{}': UNC paths are not scanned",
                display(raw)
            ));
            return None;
        }
        // A drive root (e.g. `D:\`) is a valid scan root: the caller opts in explicitly through
        // `opts.roots`. Only UNC, blocked drive types, reparse points and forbidden subtrees are
        // rejected below. `is_root_dir` still protects *deletion* targets elsewhere in the crate.
        // canonicalize is only meaningful for real, existing paths; in-memory views keep the input.
        if crate::has_reparse_component(raw, self.fs) {
            self.error(format!("invalid root '{}': reparse points are not scanned", display(raw)));
            return None;
        }
        let canonical = std::fs::canonicalize(raw).ok().map(|p| strip_verbatim(&p));
        let path = canonical.clone().unwrap_or_else(|| raw.to_path_buf());
        let Some(attrs) = self.fs.attrs(&path) else {
            self.error(format!(
                "invalid root '{}': path is missing or unreadable",
                display(&path)
            ));
            return None;
        };
        if attrs & REPARSE_POINT != 0 {
            self.error(format!(
                "invalid root '{}': reparse points are not scanned",
                display(&path)
            ));
            return None;
        }
        if self.fs.is_real_fs() && canonical.is_some() && is_removable(&path) {
            self.error(format!(
                "invalid root '{}': removable media is not scanned",
                display(&path)
            ));
            return None;
        }
        for forbidden_path in &self.forbidden {
            if path_contains(forbidden_path, &path) {
                self.error(format!(
                    "invalid root '{}': lies inside forbidden path '{}'",
                    display(&path),
                    display(forbidden_path)
                ));
                return None;
            }
        }
        Some(path)
    }

    /// Recursive byte count of a matched directory, plus its newest mtime and `.trx` presence.
    /// Descendant reparse points and `.git` are ignored; unreadable entries are skipped.
    fn stats_of(&mut self, dir: &Path, own_modified: Option<u64>) -> Option<DirStats> {
        let mut stats = DirStats {
            size: 0,
            newest: own_modified,
            has_trx: false,
        };
        let mut stack = vec![dir.to_path_buf()];
        while let Some(current) = stack.pop() {
            if !self.bump_dir() {
                return None;
            }
            let entries = match self.fs.entries(&current) {
                Ok(e) => e,
                Err(_) => {
                    self.manifest.skipped += 1;
                    continue;
                }
            };
            for entry in entries {
                if entry.attrs & REPARSE_POINT != 0 {
                    self.manifest.skipped += 1;
                    continue;
                }
                if entry.is_dir && eq_name(&entry.name, ".git") {
                    self.manifest.skipped += 1;
                    continue;
                }
                if self.is_forbidden(&entry.path) {
                    self.manifest.skipped += 1;
                    continue;
                }
                if let Some(modified) = entry.modified {
                    stats.newest = Some(stats.newest.map_or(modified, |n| n.max(modified)));
                }
                if entry.is_dir {
                    stack.push(entry.path);
                } else {
                    stats.size = stats.size.saturating_add(entry.len);
                    // Reparse points / .git / forbidden paths were already skipped above. Record a
                    // normal display path (a custom FsView may still hand us a `\\?\` path).
                    self.top
                        .offer(display(&strip_verbatim(&entry.path)), entry.len);
                    if is_trx_entry(&entry) {
                        stats.has_trx = true;
                    }
                }
            }
        }
        Some(stats)
    }

    fn accept(&mut self, entry: &EntryInfo, kind: RuleKind, stats: DirStats) {
        if self.opts.skip_recent_days > 0
            && is_recent(stats.newest, self.opts.skip_recent_days, self.opts.now_secs)
        {
            self.manifest.skipped += 1;
            return;
        }
        if self.manifest.items.len() >= self.opts.max_items {
            self.truncate("max_items");
            return;
        }
        let path = display(&entry.path);
        self.manifest.total = self.manifest.total.saturating_add(stats.size);
        self.manifest.items.push(Item::new(path, stats.size, kind));
        self.manifest.matched += 1;
    }

    /// Walk one validated root. A matched directory is collapsed into one item and is never
    /// descended into looking for nested rules.
    fn walk(&mut self, root: PathBuf) {
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            if self.out_of_budget || !self.bump_dir() {
                return;
            }
            let entries = match self.fs.entries(&dir) {
                Ok(e) => e,
                Err(_) => {
                    self.manifest.skipped += 1;
                    continue;
                }
            };
            // Snapshot the children: classifying needs `&entries` while `stats_of` needs `&mut self`.
            // A forbidden subtree nested inside the scanned root is skipped here too.
            let mut children: Vec<EntryInfo> = Vec::new();
            for entry in &entries {
                if entry.attrs & REPARSE_POINT != 0 {
                    self.manifest.skipped += 1;
                    continue;
                }
                if entry.is_dir && eq_name(&entry.name, ".git") {
                    self.manifest.skipped += 1;
                    continue;
                }
                if !entry.is_dir {
                    continue;
                }
                if self.is_forbidden(&entry.path) {
                    self.manifest.skipped += 1;
                    continue;
                }
                children.push(entry.clone());
            }
            for child in children {
                if self.out_of_budget {
                    return;
                }
                if self.forbidden.iter().any(|protected| path_contains(&child.path, protected)) {
                    // Still traverse eligible siblings, but never offer the parent for recursive deletion.
                    stack.push(child.path);
                    continue;
                }
                match candidate_rule(&child.name, &entries) {
                    Candidate::None => stack.push(child.path),
                    Candidate::NeedTrx => {
                        let Some(stats) = self.stats_of(&child.path, child.modified) else {
                            return;
                        };
                        if stats.has_trx {
                            self.accept(&child, RuleKind::TestResults, stats);
                        } else {
                            stack.push(child.path);
                        }
                    }
                    Candidate::Rule(kind) => {
                        let Some(stats) = self.stats_of(&child.path, child.modified) else {
                            return;
                        };
                        self.accept(&child, kind, stats);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_heap_is_bounded_and_sorted_by_size_then_path() {
        let mut heap = TopHeap::default();
        for i in 0..12 {
            heap.offer(format!(r"D:\r\p\f{i:02}.bin"), i * 10);
        }
        let got = heap.take();
        assert_eq!(got.len(), TOP_LIMIT);
        assert_eq!(
            got.iter().map(|e| e.size).collect::<Vec<_>>(),
            vec![110, 100, 90, 80, 70, 60, 50, 40]
        );
        assert_eq!(got[0].path, r"D:\r\p\f11.bin");

        // Một file nhỏ đến sau không được đẩy file nặng hơn ra.
        for i in 0..12 {
            heap.offer(format!(r"D:\r\p\f{i:02}.bin"), i * 10);
        }
        heap.offer(r"D:\r\p\tiny.bin".into(), 1);
        let again = heap.take();
        assert_eq!(again.len(), TOP_LIMIT);
        assert_eq!(again[0].size, 110);
        assert!(again.iter().all(|e| e.size >= 40));
    }

    #[test]
    fn top_heap_breaks_size_ties_by_smaller_path_regardless_of_insertion_order() {
        let mut heap = TopHeap::default();
        for i in (0..10).rev() {
            heap.offer(format!(r"D:\r\p\z{i}.bin"), 100);
        }
        let got = heap.take();
        assert_eq!(got.len(), TOP_LIMIT);
        assert_eq!(
            got.iter().map(|e| e.path.clone()).collect::<Vec<_>>(),
            (0..8)
                .map(|i| format!(r"D:\r\p\z{i}.bin"))
                .collect::<Vec<_>>()
        );
        assert!(got.iter().all(|e| e.size == 100));
    }

    #[test]
    fn top_folders_group_by_direct_parent_and_sum_child_sizes() {
        let items = vec![
            Item::new(r"D:\r\proj\bin".into(), 10, RuleKind::Bin),
            Item::new(r"D:\r\proj\obj".into(), 20, RuleKind::Obj),
            Item::new(r"D:\r\proj\nested\.vs".into(), 5, RuleKind::Vs),
            Item::new(r"D:\r\other\node_modules".into(), 25, RuleKind::NodeModules),
        ];
        assert_eq!(
            top_folders_from_items(&items),
            vec![
                HeavyEntry {
                    path: r"D:\r\proj".into(),
                    size: 30,
                },
                HeavyEntry {
                    path: r"D:\r\other".into(),
                    size: 25,
                },
                HeavyEntry {
                    path: r"D:\r\proj\nested".into(),
                    size: 5,
                },
            ]
        );
    }

    #[test]
    fn top_folders_is_bounded_and_ties_break_by_path() {
        let items: Vec<Item> = (0..10)
            .map(|i| Item::new(format!(r"D:\r\p{i:02}\bin"), 7, RuleKind::Bin))
            .collect();
        let got = top_folders_from_items(&items);
        assert_eq!(got.len(), TOP_LIMIT);
        assert_eq!(got[0].path, r"D:\r\p00");
        assert_eq!(got[TOP_LIMIT - 1].path, r"D:\r\p07");
    }
}
