//! Detection-only duplicate file finder.
//!
//! Nothing in this module writes, moves or deletes anything: it walks a [`FsView`], groups
//! same-size files by content and reports the groups. There is deliberately no "clean duplicates"
//! API and no id that a later phase could feed into a deleter.

use crate::fs::{
    canonical_for, display, is_removable, is_unc, path_contains, strip_verbatim, REPARSE_POINT,
};
use crate::model::FsView;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Legacy default minimum file size considered for duplicate detection (1 MiB). Kept exported for
/// compatibility; callers (e.g. the dashboard) pass the value from config in `DupsOptions`.
pub const DEFAULT_MIN_SIZE: u64 = 1024 * 1024;
/// Default cap on the number of candidate files collected.
pub const DEFAULT_MAX_FILES: usize = 200_000;
/// Default cap on the total number of bytes hashed (8 GiB).
pub const DEFAULT_MAX_BYTES_HASH: u64 = 8 * 1024 * 1024 * 1024;

/// Stable identity of a physical file (NTFS volume serial + file index).
///
/// Two paths with the same `FileId` are hard links to one physical file and must be collapsed
/// into a single copy — they do not represent wasted space.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FileId {
    pub volume: u64,
    pub index: u64,
}

/// One path in a duplicate group, together with its (optional) physical identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DupFile {
    pub path: String,
    pub identity: Option<FileId>,
}

/// A set of distinct physical files with identical content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DupGroup {
    /// Lowercase hex SHA-256 of the shared content. A plain string, never logged to stdout.
    pub hash: String,
    /// Distinct copies, sorted by path for deterministic output.
    pub files: Vec<DupFile>,
    pub size_each: u64,
    /// `size_each * (number of distinct copies - 1)`. Hard links are not counted.
    pub wasted: u64,
}

/// Configuration for [`find_duplicates`]. No new config key is introduced elsewhere in the crate.
#[derive(Debug, Clone)]
pub struct DupsOptions {
    pub roots: Vec<PathBuf>,
    pub forbidden: Vec<PathBuf>,
    pub min_size: u64,
    pub max_files: usize,
    pub max_bytes_hash: u64,
    pub now_secs: u64,
}

/// Result of [`find_duplicates`].
#[derive(Debug, Clone, Default)]
pub struct DupsReport {
    /// Groups sorted by `wasted` descending, tie-broken by the first file path.
    pub groups: Vec<DupGroup>,
    /// Candidate files actually collected (passed all collection filters).
    pub scanned: usize,
    /// Entries deliberately not considered (reparse points, `.git`, forbidden, too small, ...).
    pub skipped: usize,
    /// Total bytes hashed during the discovery pass; never exceeds `max_bytes_hash`.
    /// Verification re-hashes performed after re-stat are not counted here.
    pub bytes_hashed: u64,
    pub errors: Vec<String>,
    /// True when `max_files` or `max_bytes_hash` stopped the scan early.
    pub truncated: bool,
}

/// A file that passed the collection filters.
#[derive(Clone)]
struct Candidate {
    path: PathBuf,
    len: u64,
    identity: Option<FileId>,
    modified: Option<u64>,
}

/// A candidate with its full-content hash computed.
struct Hashed {
    candidate: Candidate,
    hash: String,
}

fn error(report: &mut DupsReport, message: String) {
    if !report.errors.contains(&message) {
        report.errors.push(message);
    }
}

fn is_git_name(name: &str) -> bool {
    name.eq_ignore_ascii_case(".git")
}

/// Read-only walk of one validated root. Stops at the first budget breach.
fn collect_root(
    raw: &Path,
    opts: &DupsOptions,
    fs: &dyn FsView,
    forbidden: &[PathBuf],
    candidates: &mut Vec<Candidate>,
    report: &mut DupsReport,
) {
    if !raw.is_absolute() {
        error(
            report,
            format!("invalid root '{}': root is not absolute", display(raw)),
        );
        return;
    }
    if is_unc(raw) {
        error(
            report,
            format!("invalid root '{}': UNC paths are not scanned", display(raw)),
        );
        return;
    }
    // A drive root (e.g. `D:\`) is a valid scan root: the caller opts in explicitly through
    // `opts.roots`. Only UNC, blocked drive types, reparse points and forbidden subtrees are
    // rejected below. `is_root_dir` still protects *deletion* targets elsewhere in the crate.
    // canonicalize is only meaningful for real, existing paths; in-memory views keep the input.
    if crate::has_reparse_component(raw, fs) {
        error(report, format!("invalid root '{}': reparse points are not scanned", display(raw)));
        return;
    }
    let canonical = std::fs::canonicalize(raw).ok().map(|p| strip_verbatim(&p));
    let path = canonical.clone().unwrap_or_else(|| raw.to_path_buf());
    let Some(attrs) = fs.attrs(&path) else {
        error(
            report,
            format!(
                "invalid root '{}': path is missing or unreadable",
                display(&path)
            ),
        );
        return;
    };
    if attrs & REPARSE_POINT != 0 {
        error(
            report,
            format!(
                "invalid root '{}': reparse points are not scanned",
                display(&path)
            ),
        );
        return;
    }
    if fs.is_real_fs() && canonical.is_some() && is_removable(&path) {
        error(
            report,
            format!(
                "invalid root '{}': removable media is not scanned",
                display(&path)
            ),
        );
        return;
    }
    for f in forbidden {
        if path_contains(f, &path) {
            error(
                report,
                format!(
                    "invalid root '{}': lies inside forbidden path '{}'",
                    display(&path),
                    display(f)
                ),
            );
            return;
        }
    }

    let mut stack = vec![path];
    while let Some(dir) = stack.pop() {
        if report.truncated {
            return;
        }
        let entries = match fs.entries(&dir) {
            Ok(e) => e,
            Err(_) => {
                report.skipped += 1;
                error(
                    report,
                    format!("cannot enumerate directory '{}'", display(&dir)),
                );
                continue;
            }
        };
        for entry in entries {
            if report.truncated {
                return;
            }
            if entry.attrs & REPARSE_POINT != 0 {
                report.skipped += 1;
                continue;
            }
            if entry.is_dir && is_git_name(&entry.name) {
                report.skipped += 1;
                continue;
            }
            if forbidden.iter().any(|f| path_contains(f, &entry.path)) {
                report.skipped += 1;
                continue;
            }
            if entry.is_dir {
                stack.push(entry.path);
                continue;
            }
            // A zero-byte file wastes nothing even when min_size is 0; never report it.
            if entry.len == 0 || entry.len < opts.min_size {
                report.skipped += 1;
                continue;
            }
            if candidates.len() >= opts.max_files {
                report.truncated = true;
                error(
                    report,
                    format!(
                        "duplicate scan truncated: budget max_files ({}) reached; results are incomplete",
                        opts.max_files
                    ),
                );
                return;
            }
            report.scanned += 1;
            let identity = fs.identity(&entry.path);
            candidates.push(Candidate {
                path: entry.path,
                len: entry.len,
                identity,
                modified: entry.modified,
            });
        }
    }
}

/// Collapse hard links: among files sharing one `(volume, index)` keep only the smallest path.
///
/// A file whose identity could not be determined is **excluded entirely** — it is neither used
/// for collapsing nor counted as an independent copy. Otherwise a locked/unreadable file would
/// silently inflate `wasted`.
fn collapse_hardlinks(group: &[Candidate], report: &mut DupsReport) -> Vec<Candidate> {
    let mut sorted: Vec<&Candidate> = group.iter().collect();
    sorted.sort_by(|a, b| a.path.cmp(&b.path));
    let mut seen: HashSet<FileId> = HashSet::new();
    let mut out = Vec::new();
    for c in sorted {
        match c.identity.as_ref() {
            Some(id) => {
                if seen.insert((*id).clone()) {
                    out.push((*c).clone());
                }
                // otherwise: a hard link to a representative we already kept.
            }
            None => {
                report.skipped += 1;
                error(
                    report,
                    format!(
                        "cannot determine file identity for '{}' (excluded from duplicate grouping)",
                        display(&c.path)
                    ),
                );
            }
        }
    }
    out
}

fn hex_sha256(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for b in digest.iter() {
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// Re-read `(len, modified, identity)` immediately before the final content comparison.
/// Implemented via the existing `entries` method so no extra trait surface is needed; the path
/// we pass is the one collected earlier.
fn restat(fs: &dyn FsView, c: &Candidate) -> Option<(u64, Option<u64>, Option<FileId>)> {
    let parent = c.path.parent()?;
    let entries = fs.entries(parent).ok()?;
    let fresh = entries.iter().find(|e| e.path == c.path)?;
    Some((fresh.len, fresh.modified, fs.identity(&c.path)))
}

/// Find duplicate files without modifying the filesystem.
pub fn find_duplicates(opts: &DupsOptions, fs: &dyn FsView) -> DupsReport {
    let mut report = DupsReport::default();
    let forbidden: Vec<PathBuf> = opts.forbidden.iter().flat_map(|p| [p.clone(), canonical_for(p)]).collect();

    // 1. Collect candidate files (read-only walk, budgeted by max_files).
    let mut candidates: Vec<Candidate> = Vec::new();
    for raw in &opts.roots {
        if report.truncated {
            break;
        }
        collect_root(raw, opts, fs, &forbidden, &mut candidates, &mut report);
    }

    // 2. Bucket by size; duplicates must have identical length.
    let mut by_size: HashMap<u64, Vec<Candidate>> = HashMap::new();
    for c in candidates {
        by_size.entry(c.len).or_default().push(c);
    }

    // 3. Hash representatives of size groups with at least two distinct physical files.
    let mut hashed: Vec<Hashed> = Vec::new();
    let mut sizes: Vec<u64> = by_size.keys().copied().collect();
    sizes.sort_unstable();
    for size in sizes {
        if report.truncated {
            break;
        }
        let reps = collapse_hardlinks(&by_size[&size], &mut report);
        if reps.len() < 2 {
            continue;
        }
        for c in reps {
            // Check the whole-file cost up front: never hash a prefix and report it as a match.
            if report.bytes_hashed.saturating_add(c.len) > opts.max_bytes_hash {
                report.truncated = true;
                error(
                    &mut report,
                    format!(
                        "duplicate scan truncated: budget max_bytes_hash ({}) reached; results are incomplete",
                        opts.max_bytes_hash
                    ),
                );
                break;
            }
            let bytes = match fs.read(&c.path) {
                Ok(b) => b,
                Err(e) => {
                    report.skipped += 1;
                    error(
                        &mut report,
                        format!("cannot read '{}': {e}", display(&c.path)),
                    );
                    continue;
                }
            };
            report.bytes_hashed = report.bytes_hashed.saturating_add(bytes.len() as u64);
            hashed.push(Hashed {
                candidate: c,
                hash: hex_sha256(&bytes),
            });
        }
    }

    // 4. Bucket by hash, then re-stat + re-read bytes immediately before the final comparison.
    let mut by_hash: HashMap<String, Vec<Hashed>> = HashMap::new();
    for h in hashed {
        by_hash.entry(h.hash.clone()).or_default().push(h);
    }
    for (hash, mut bucket) in by_hash {
        if bucket.len() < 2 {
            continue;
        }
        bucket.sort_by(|a, b| a.candidate.path.cmp(&b.candidate.path));

        // Re-stat FIRST, then read the bytes at that same moment. A content change that keeps
        // size + mtime + identity is still caught by re-hashing these fresh bytes against `hash`.
        let mut first_bytes: Option<Vec<u8>> = None;
        let mut confirmed: Vec<Hashed> = Vec::new();
        for h in bucket {
            let changed = match restat(fs, &h.candidate) {
                Some((len, modified, identity)) => {
                    len != h.candidate.len
                        || modified != h.candidate.modified
                        || identity != h.candidate.identity
                }
                None => true,
            };
            if changed {
                error(
                    &mut report,
                    format!(
                        "file changed during duplicate scan: '{}' (discarded)",
                        display(&h.candidate.path)
                    ),
                );
                continue;
            }
            let bytes = match fs.read(&h.candidate.path) {
                Ok(b) => b,
                Err(e) => {
                    error(
                        &mut report,
                        format!("cannot re-read '{}': {e}", display(&h.candidate.path)),
                    );
                    continue;
                }
            };
            if hex_sha256(&bytes) != h.hash {
                error(
                    &mut report,
                    format!(
                        "file changed during duplicate scan: '{}' (discarded)",
                        display(&h.candidate.path)
                    ),
                );
                continue;
            }
            // Byte-for-byte comparison rules out a (practically impossible) SHA-256 collision.
            match first_bytes.as_ref() {
                None => {
                    first_bytes = Some(bytes);
                    confirmed.push(h);
                }
                Some(first) if first == &bytes => confirmed.push(h),
                Some(_) => {
                    error(
                        &mut report,
                        format!(
                            "hash collision discarded after byte comparison at '{}'",
                            display(&h.candidate.path)
                        ),
                    );
                }
            }
        }
        if confirmed.len() < 2 {
            continue;
        }

        let size_each = confirmed[0].candidate.len;
        let files: Vec<DupFile> = confirmed
            .iter()
            .map(|h| DupFile {
                path: display(&h.candidate.path),
                identity: h.candidate.identity.clone(),
            })
            .collect();
        let wasted = size_each.saturating_mul((files.len() - 1) as u64);
        report.groups.push(DupGroup {
            hash,
            files,
            size_each,
            wasted,
        });
    }

    report.groups.sort_by(|a, b| {
        b.wasted.cmp(&a.wasted).then_with(|| {
            let pa = a.files.first().map(|f| f.path.as_str()).unwrap_or("");
            let pb = b.files.first().map(|f| f.path.as_str()).unwrap_or("");
            pa.cmp(pb)
        })
    });

    report
}
