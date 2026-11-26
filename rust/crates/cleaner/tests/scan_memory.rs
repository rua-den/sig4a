//! Scanner tests on a fully in-memory filesystem — no real junction/symlink creation needed.

use cu_cleaner::{scan, EntryInfo, FsView, Mode, RuleKind, ScanOptions};
use std::io;
use std::path::{Component, Path, PathBuf};

const REPARSE: u32 = 0x400;
/// Fixed "now" keeps `skip_recent_days` tests deterministic.
const NOW: u64 = 1_700_000_000;

#[derive(Clone)]
struct Node {
    path: PathBuf,
    is_dir: bool,
    attrs: u32,
    len: u64,
    modified: Option<u64>,
}

#[derive(Default)]
struct MemoryFs {
    nodes: Vec<Node>,
}

fn key(path: &Path) -> String {
    path.to_string_lossy().to_lowercase()
}

impl MemoryFs {
    fn new() -> Self {
        Self::default()
    }

    fn get(&self, path: &Path) -> Option<&Node> {
        let wanted = key(path);
        self.nodes.iter().find(|n| key(&n.path) == wanted)
    }

    fn get_mut(&mut self, path: &Path) -> Option<&mut Node> {
        let wanted = key(path);
        self.nodes.iter_mut().find(|n| key(&n.path) == wanted)
    }

    fn ensure_parents(&mut self, path: &Path) {
        let mut current = path.parent();
        while let Some(parent) = current {
            if parent.as_os_str().is_empty() {
                break;
            }
            let components: Vec<Component<'_>> = parent.components().collect();
            // Do not materialise `D:\` itself.
            if components.len() <= 2 && matches!(components.first(), Some(Component::Prefix(_))) {
                break;
            }
            if self.get(parent).is_none() {
                self.nodes.push(Node {
                    path: parent.to_path_buf(),
                    is_dir: true,
                    attrs: 0,
                    len: 0,
                    modified: None,
                });
            }
            current = parent.parent();
        }
    }

    fn dir(&mut self, path: &str) -> &mut Self {
        self.dir_at(path, None)
    }

    fn dir_at(&mut self, path: &str, modified: Option<u64>) -> &mut Self {
        let p = PathBuf::from(path);
        self.ensure_parents(&p);
        if self.get(&p).is_none() {
            self.nodes.push(Node {
                path: p,
                is_dir: true,
                attrs: 0,
                len: 0,
                modified,
            });
        }
        self
    }

    fn file(&mut self, path: &str, len: u64) -> &mut Self {
        self.file_at(path, len, None)
    }

    fn file_at(&mut self, path: &str, len: u64, modified: Option<u64>) -> &mut Self {
        let p = PathBuf::from(path);
        self.ensure_parents(&p);
        if self.get(&p).is_none() {
            self.nodes.push(Node {
                path: p,
                is_dir: false,
                attrs: 0,
                len,
                modified,
            });
        }
        self
    }

    fn reparse(&mut self, path: &str) -> &mut Self {
        let p = PathBuf::from(path);
        self.ensure_parents(&p);
        if self.get(&p).is_none() {
            self.nodes.push(Node {
                path: p.clone(),
                is_dir: true,
                attrs: 0,
                len: 0,
                modified: None,
            });
        }
        self.get_mut(&p).expect("just inserted").attrs |= REPARSE;
        self
    }
}

impl FsView for MemoryFs {
    fn entries(&self, dir: &Path) -> io::Result<Vec<EntryInfo>> {
        match self.get(dir) {
            Some(node) if node.is_dir => {}
            _ => return Err(io::Error::new(io::ErrorKind::NotFound, "no such directory")),
        }
        let wanted = key(dir);
        Ok(self
            .nodes
            .iter()
            .filter(|n| n.path.parent().is_some_and(|p| key(p) == wanted))
            .map(|n| EntryInfo {
                name: n
                    .path
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                path: n.path.clone(),
                is_dir: n.is_dir,
                attrs: n.attrs,
                len: n.len,
                modified: n.modified,
            })
            .collect())
    }

    fn attrs(&self, path: &Path) -> Option<u32> {
        self.get(path).map(|n| n.attrs)
    }
}

fn opts(roots: &[&str]) -> ScanOptions {
    ScanOptions {
        roots: roots.iter().map(PathBuf::from).collect(),
        forbidden: Vec::new(),
        skip_recent_days: 0,
        now_secs: NOW,
        max_items: 10_000,
        max_dirs: 100_000,
    }
}

fn kinds_and_sizes(manifest: &cu_cleaner::Manifest) -> Vec<(RuleKind, u64)> {
    manifest.items.iter().map(|i| (i.kind, i.size)).collect()
}

#[test]
fn bin_and_obj_need_a_project_marker_and_are_two_items() {
    let mut fs = MemoryFs::new();
    fs.dir(r"D:\cf\p")
        .file(r"D:\cf\p\p.csproj", 1)
        .dir(r"D:\cf\p\bin")
        .file(r"D:\cf\p\bin\a.dll", 10)
        .dir(r"D:\cf\p\obj")
        .file(r"D:\cf\p\obj\b.dll", 20)
        .dir(r"D:\cf\q")
        .dir(r"D:\cf\q\bin")
        .file(r"D:\cf\q\bin\c.dll", 999);

    let m = scan(&opts(&[r"D:\cf"]), &fs);
    let mut got = kinds_and_sizes(&m);
    got.sort_by_key(|(k, _)| format!("{k:?}"));
    assert_eq!(got, vec![(RuleKind::Bin, 10), (RuleKind::Obj, 20)]);
    assert_eq!(m.matched, 2);
    assert_eq!(m.total, 30);
    assert_eq!(m.items[0].id, m.items[0].path, "id must equal path");
    assert_eq!(m.created_at, NOW, "created_at comes from opts.now_secs");
    assert_eq!(
        m.items.iter().map(|i| i.size).collect::<Vec<_>>(),
        vec![20, 10],
        "items are sorted by size descending"
    );
    assert!(m.errors.is_empty(), "{:?}", m.errors);
}

#[test]
fn node_modules_needs_sibling_package_json() {
    let mut fs = MemoryFs::new();
    fs.dir(r"D:\cn\a")
        .file(r"D:\cn\a\package.json", 4)
        .dir(r"D:\cn\a\node_modules")
        .file(r"D:\cn\a\node_modules\x.js", 6)
        .dir(r"D:\cn\b")
        .dir(r"D:\cn\b\node_modules")
        .file(r"D:\cn\b\node_modules\y.js", 7);

    let m = scan(&opts(&[r"D:\cn"]), &fs);
    assert_eq!(kinds_and_sizes(&m), vec![(RuleKind::NodeModules, 6)]);
}

#[test]
fn testresults_needs_project_marker_or_trx_file() {
    let mut fs = MemoryFs::new();
    fs.dir(r"D:\ct\p")
        .file(r"D:\ct\p\p.fsproj", 1)
        .dir(r"D:\ct\p\TestResults")
        .file(r"D:\ct\p\TestResults\r.trx", 9)
        .dir(r"D:\ct\q")
        .dir(r"D:\ct\q\TestResults")
        .file(r"D:\ct\q\TestResults\notes.txt", 3)
        .dir(r"D:\ct\s")
        .dir(r"D:\ct\s\TestResults")
        .file(r"D:\ct\s\TestResults\result.TRX", 5);

    let m = scan(&opts(&[r"D:\ct"]), &fs);
    let mut got = kinds_and_sizes(&m);
    got.sort_by_key(|(_, size)| *size);
    assert_eq!(
        got,
        vec![(RuleKind::TestResults, 5), (RuleKind::TestResults, 9)]
    );
}

#[test]
fn vs_is_matched_by_name_alone() {
    let mut fs = MemoryFs::new();
    fs.dir(r"D:\cv\p")
        .dir(r"D:\cv\p\.vs")
        .file(r"D:\cv\p\.vs\cache", 2);
    let m = scan(&opts(&[r"D:\cv"]), &fs);
    assert_eq!(kinds_and_sizes(&m), vec![(RuleKind::Vs, 2)]);
}

#[test]
fn reparse_points_are_skipped_even_deep_inside_a_match() {
    let mut fs = MemoryFs::new();
    fs.dir(r"D:\cr\p")
        .file(r"D:\cr\p\p.vbproj", 1)
        .dir(r"D:\cr\p\bin")
        .file(r"D:\cr\p\bin\real.dll", 3);
    fs.file(r"D:\cr\p\bin\link.dll", 500);
    fs.reparse(r"D:\cr\p\bin\link.dll");
    fs.file(r"D:\cr\p\bin\sub\deep.dll", 1000);
    fs.reparse(r"D:\cr\p\bin\sub");
    // A reparse point called `bin` must not be matched either.
    fs.dir(r"D:\cr\p2").file(r"D:\cr\p2\p2.csproj", 1);
    fs.file(r"D:\cr\p2\bin\huge.dll", 7000);
    fs.reparse(r"D:\cr\p2\bin");

    let m = scan(&opts(&[r"D:\cr"]), &fs);
    assert_eq!(kinds_and_sizes(&m), vec![(RuleKind::Bin, 3)]);
    assert!(m.skipped >= 3, "skipped={}", m.skipped);
}

#[test]
fn git_is_skipped_and_nested_bin_is_collapsed_into_one_item() {
    let mut fs = MemoryFs::new();
    fs.file(r"D:\cg\.git\objects\pack\huge", 5000)
        .dir(r"D:\cg\proj")
        .file(r"D:\cg\proj\proj.csproj", 1)
        .dir(r"D:\cg\proj\bin")
        .file(r"D:\cg\proj\bin\a.dll", 5)
        .dir(r"D:\cg\proj\bin\Debug\bin")
        .file(r"D:\cg\proj\bin\Debug\bin\b.dll", 7);

    let m = scan(&opts(&[r"D:\cg"]), &fs);
    assert_eq!(kinds_and_sizes(&m), vec![(RuleKind::Bin, 12)]);
    assert_eq!(m.total, 12, ".git contents must not be counted");
}

#[test]
fn skip_recent_days_uses_the_newest_mtime_in_the_matched_dir() {
    let old = NOW - 100 * 86_400;
    let fresh = NOW - 86_400;
    let mut fs = MemoryFs::new();
    fs.dir_at(r"D:\cm\fresh\p", Some(fresh))
        .file(r"D:\cm\fresh\p\p.csproj", 1)
        .dir_at(r"D:\cm\fresh\p\bin", Some(fresh))
        .file_at(r"D:\cm\fresh\p\bin\a.dll", 4, Some(fresh))
        .dir_at(r"D:\cm\old\p", Some(old))
        .file(r"D:\cm\old\p\p.csproj", 1)
        .dir_at(r"D:\cm\old\p\bin", Some(old))
        .file_at(r"D:\cm\old\p\bin\b.dll", 8, Some(old));

    let mut recent = opts(&[r"D:\cm"]);
    recent.skip_recent_days = 30;
    let m = scan(&recent, &fs);
    assert_eq!(kinds_and_sizes(&m), vec![(RuleKind::Bin, 8)]);
    assert!(m.skipped >= 1);

    let m = scan(&opts(&[r"D:\cm"]), &fs);
    let mut got = kinds_and_sizes(&m);
    got.sort_by_key(|(_, size)| *size);
    assert_eq!(got, vec![(RuleKind::Bin, 4), (RuleKind::Bin, 8)]);
}

#[test]
fn root_validation_is_component_wise_and_honours_forbidden() {
    let mut fs = MemoryFs::new();
    fs.dir(r"D:\stu")
        .dir(r"D:\stu\child")
        .dir(r"D:\studies")
        .dir(r"D:\studies\app")
        .file(r"D:\studies\app\a.csproj", 1)
        .dir(r"D:\studies\app\bin")
        .file(r"D:\studies\app\bin\x.dll", 3);

    let mut o = opts(&[r"D:\studies"]);
    o.forbidden = vec![PathBuf::from(r"D:\stu")];
    let m = scan(&o, &fs);
    assert!(m.errors.is_empty(), "{:?}", m.errors);
    assert_eq!(m.roots, vec![r"D:\studies"]);
    assert_eq!(
        kinds_and_sizes(&m),
        vec![(RuleKind::Bin, 3)],
        "D:\\studies is not inside D:\\stu"
    );

    let mut o = opts(&[r"D:\stu\child"]);
    o.forbidden = vec![PathBuf::from(r"D:\stu")];
    let m = scan(&o, &fs);
    assert!(m.roots.is_empty());
    assert!(
        m.errors.iter().any(|e| e.contains("forbidden")),
        "{:?}",
        m.errors
    );
}

#[test]
fn forbidden_subtree_inside_a_valid_root_is_not_reported() {
    let mut fs = MemoryFs::new();
    fs.dir(r"D:\stu")
        .dir(r"D:\stu\repo")
        .file(r"D:\stu\repo\p.csproj", 1)
        .dir(r"D:\stu\repo\bin")
        .file(r"D:\stu\repo\bin\keep.dll", 3)
        .dir(r"D:\stu\repo\vault")
        .file(r"D:\stu\repo\vault\p.csproj", 1)
        .dir(r"D:\stu\repo\vault\bin")
        .file(r"D:\stu\repo\vault\bin\secret.dll", 900);

    let mut o = opts(&[r"D:\stu"]);
    o.forbidden = vec![PathBuf::from(r"D:\stu\repo\vault")];
    let m = scan(&o, &fs);
    assert!(m.errors.is_empty(), "{:?}", m.errors);
    assert_eq!(kinds_and_sizes(&m), vec![(RuleKind::Bin, 3)]);
    assert!(
        m.items
            .iter()
            .all(|i| !i.path.to_lowercase().contains("vault")),
        "the forbidden nested subtree must yield no item"
    );
}

#[test]
fn shared_safety_helpers_are_public() {
    use cu_cleaner::{path_contains, REPARSE_POINT};
    assert_eq!(REPARSE_POINT, 0x400);
    assert!(path_contains(
        Path::new(r"D:\STU"),
        Path::new(r"D:\stu\repo")
    ));
    assert!(!path_contains(
        Path::new(r"D:\stu"),
        Path::new(r"D:\studies")
    ));
}

#[test]
fn mode_deserialises_from_lowercase_ui_values() {
    assert_eq!(
        serde_json::from_str::<Mode>(r#""permanent""#).expect("permanent"),
        Mode::Permanent
    );
    assert_eq!(
        serde_json::from_str::<Mode>(r#""recycle""#).expect("recycle"),
        Mode::Recycle
    );
    assert_eq!(
        serde_json::to_string(&Mode::Recycle).expect("serialize"),
        r#""recycle""#
    );
}

#[test]
fn invalid_roots_are_reported_without_panicking() {
    let mut fs = MemoryFs::new();
    fs.dir(r"D:\cu_cleaner_bad").reparse(r"D:\cu_cleaner_bad");
    let o = opts(&[
        r"relative\path",
        r"D:\cu_cleaner_missing",
        r"D:\cu_cleaner_bad",
        r"\\server\share\proj",
    ]);
    let m = scan(&o, &fs);
    assert_eq!(m.errors.len(), 4, "{:?}", m.errors);
    assert!(m.roots.is_empty());
    assert!(m.errors.iter().any(|e| e.contains("reparse")));
    assert!(m.errors.iter().any(|e| e.contains("UNC")));
    assert!(m.errors.iter().any(|e| e.contains("root is not absolute")));
}

#[test]
fn drive_root_is_a_valid_scan_root_in_memory() {
    let mut fs = MemoryFs::new();
    // Materialise the fake `D:\` itself; `canonicalize`/drive-type probes must not decide this.
    fs.dir(r"D:\");
    let m = scan(&opts(&[r"D:\"]), &fs);
    assert!(m.errors.is_empty(), "{:?}", m.errors);
    assert_eq!(m.roots, vec![r"D:\"]);
}

#[test]
fn budget_limits_produce_a_truncation_error() {
    let mut fs = MemoryFs::new();
    fs.dir(r"D:\cb");
    for i in 0..6 {
        let base = format!(r"D:\cb\p{i}");
        fs.dir(&base)
            .file(&format!(r"{base}\p.csproj"), 1)
            .dir(&format!(r"{base}\bin"))
            .file(&format!(r"{base}\bin\f.dll"), 1);
    }

    let mut o = opts(&[r"D:\cb"]);
    o.max_items = 2;
    let m = scan(&o, &fs);
    assert_eq!(m.matched, 2);
    assert!(
        m.errors.iter().any(|e| e.contains("max_items")),
        "{:?}",
        m.errors
    );

    let mut o = opts(&[r"D:\cb"]);
    o.max_dirs = 3;
    let m = scan(&o, &fs);
    assert!(
        m.errors.iter().any(|e| e.contains("max_dirs")),
        "{:?}",
        m.errors
    );
}
