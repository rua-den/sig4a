//! Detection-only duplicate tests. Nothing here deletes a file or directory: the in-memory views
//! only mutate their own bookkeeping, and the RealFs tests leak their temp dirs via `into_path()`.

use cu_cleaner::{
    find_duplicates, strip_verbatim, DupsOptions, EntryInfo, FileId, FsView, RealFs,
    DEFAULT_MAX_BYTES_HASH, DEFAULT_MAX_FILES, DEFAULT_MIN_SIZE, REPARSE_POINT,
};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone)]
struct Node {
    path: PathBuf,
    is_dir: bool,
    attrs: u32,
    len: u64,
    modified: Option<u64>,
    identity: Option<FileId>,
    content: Vec<u8>,
}

/// In-memory filesystem with fake identities and content.
///
/// `entries_calls` lets a test make the *second* enumeration report a different mtime, which is
/// how we simulate "file changed between stat and hash" without touching any real file.
struct DupFs {
    nodes: Vec<Node>,
    restat_modified: Option<u64>,
    entries_calls: AtomicUsize,
    /// After `mutate_from_read` calls, `read()` serves this content instead (same length here).
    mutated_content: Option<Vec<u8>>,
    mutate_from_read: usize,
    read_calls: AtomicUsize,
}

fn key(path: &Path) -> String {
    path.to_string_lossy().to_lowercase()
}

impl DupFs {
    fn new() -> Self {
        Self {
            nodes: Vec::new(),
            restat_modified: None,
            entries_calls: AtomicUsize::new(0),
            mutated_content: None,
            mutate_from_read: usize::MAX,
            read_calls: AtomicUsize::new(0),
        }
    }

    fn get(&self, path: &Path) -> Option<&Node> {
        let wanted = key(path);
        self.nodes.iter().find(|n| key(&n.path) == wanted)
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
                    identity: None,
                    content: Vec::new(),
                });
            }
            current = parent.parent();
        }
    }

    fn dir(&mut self, path: &str) -> &mut Self {
        let p = PathBuf::from(path);
        self.ensure_parents(&p);
        if self.get(&p).is_none() {
            self.nodes.push(Node {
                path: p,
                is_dir: true,
                attrs: 0,
                len: 0,
                modified: None,
                identity: None,
                content: Vec::new(),
            });
        }
        self
    }

    fn file(
        &mut self,
        path: &str,
        content: &[u8],
        identity: Option<FileId>,
        modified: Option<u64>,
    ) -> &mut Self {
        let p = PathBuf::from(path);
        self.ensure_parents(&p);
        if self.get(&p).is_none() {
            self.nodes.push(Node {
                len: content.len() as u64,
                path: p,
                is_dir: false,
                attrs: 0,
                modified,
                identity,
                content: content.to_vec(),
            });
        }
        self
    }

    fn reparse(&mut self, path: &str) -> &mut Self {
        let p = PathBuf::from(path);
        if self.get(&p).is_none() {
            self.dir(path);
        }
        let wanted = key(&p);
        self.nodes
            .iter_mut()
            .find(|n| key(&n.path) == wanted)
            .expect("just inserted")
            .attrs |= REPARSE_POINT;
        self
    }

    fn changes_on_restat(&mut self, modified: u64) -> &mut Self {
        self.restat_modified = Some(modified);
        self
    }

    fn mutates_content_after_hash(&mut self, content: &[u8], from_read: usize) -> &mut Self {
        self.mutated_content = Some(content.to_vec());
        self.mutate_from_read = from_read;
        self
    }
}

impl FsView for DupFs {
    fn entries(&self, dir: &Path) -> io::Result<Vec<EntryInfo>> {
        let wanted = key(dir);
        match self
            .nodes
            .iter()
            .find(|n| key(&n.path) == wanted && n.is_dir)
        {
            Some(_) => {}
            None => return Err(io::Error::new(io::ErrorKind::NotFound, "no such directory")),
        }
        let call = self.entries_calls.fetch_add(1, Ordering::SeqCst);
        Ok(self
            .nodes
            .iter()
            .filter(|n| n.path.parent().is_some_and(|p| key(p) == wanted))
            .map(|n| {
                let modified = if call > 0 {
                    self.restat_modified.or(n.modified)
                } else {
                    n.modified
                };
                EntryInfo {
                    name: n
                        .path
                        .file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    path: n.path.clone(),
                    is_dir: n.is_dir,
                    attrs: n.attrs,
                    len: n.len,
                    modified,
                }
            })
            .collect())
    }

    fn attrs(&self, path: &Path) -> Option<u32> {
        self.get(path).map(|n| n.attrs)
    }

    fn identity(&self, path: &Path) -> Option<FileId> {
        self.get(path).and_then(|n| n.identity.clone())
    }

    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        let call = self.read_calls.fetch_add(1, Ordering::SeqCst);
        if call >= self.mutate_from_read {
            if let Some(content) = &self.mutated_content {
                return Ok(content.clone());
            }
        }
        self.get(path)
            .filter(|n| !n.is_dir)
            .map(|n| n.content.clone())
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no such file"))
    }
}

fn opts(roots: &[&str]) -> DupsOptions {
    DupsOptions {
        roots: roots.iter().map(PathBuf::from).collect(),
        forbidden: Vec::new(),
        min_size: 1,
        max_files: 10_000,
        max_bytes_hash: 10 * 1024 * 1024,
        now_secs: 0,
    }
}

fn id(volume: u64, index: u64) -> Option<FileId> {
    Some(FileId { volume, index })
}

#[test]
fn default_constants_are_public() {
    assert_eq!(DEFAULT_MIN_SIZE, 1024 * 1024);
    assert_eq!(DEFAULT_MAX_FILES, 200_000);
    assert_eq!(DEFAULT_MAX_BYTES_HASH, 8 * 1024 * 1024 * 1024);
}

#[test]
fn same_size_different_content_is_not_a_group() {
    let mut fs = DupFs::new();
    fs.dir(r"D:\dup1")
        .file(r"D:\dup1\a.bin", b"aaaabbbb", id(1, 1), Some(1))
        .file(r"D:\dup1\b.bin", b"ccccdddd", id(1, 2), Some(1));
    let before = fs.nodes.len();

    let report = find_duplicates(&opts(&[r"D:\dup1"]), &fs);

    assert!(report.groups.is_empty(), "{:?}", report.groups);
    assert_eq!(report.scanned, 2);
    assert_eq!(report.bytes_hashed, 16);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert!(!report.truncated);
    assert_eq!(fs.nodes.len(), before, "detection must not change the tree");
}

#[test]
fn identical_content_different_paths_is_one_group_with_correct_wasted() {
    let mut fs = DupFs::new();
    fs.dir(r"D:\dup2")
        .file(r"D:\dup2\z.bin", b"duplicate", id(9, 20), Some(1))
        .file(r"D:\dup2\a.bin", b"duplicate", id(9, 10), Some(1));
    let before = fs.nodes.len();

    let report = find_duplicates(&opts(&[r"D:\dup2"]), &fs);

    assert_eq!(report.groups.len(), 1, "{:?}", report.groups);
    let g = &report.groups[0];
    assert_eq!(g.hash.len(), 64, "sha256 must be full hex");
    assert!(g
        .hash
        .chars()
        .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    assert_eq!(g.size_each, 9);
    assert_eq!(g.wasted, 9, "two distinct copies waste exactly one");
    assert_eq!(g.files.len(), 2);
    assert_eq!(
        g.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
        vec![r"D:\dup2\a.bin", r"D:\dup2\z.bin"],
        "files are sorted by path"
    );
    assert_eq!(report.bytes_hashed, 18);
    assert_eq!(fs.nodes.len(), before, "detection must not change the tree");
}

#[test]
fn hardlinks_are_collapsed_and_do_not_count_as_wasted() {
    let mut fs = DupFs::new();
    fs.dir(r"D:\dup3")
        .file(r"D:\dup3\one.bin", b"hardlinked", id(7, 42), Some(1))
        .file(r"D:\dup3\two.bin", b"hardlinked", id(7, 42), Some(1))
        .file(r"D:\dup3\three.bin", b"hardlinked", id(7, 42), Some(1));
    let before = fs.nodes.len();

    let report = find_duplicates(&opts(&[r"D:\dup3"]), &fs);

    assert_eq!(report.scanned, 3);
    assert!(
        report.groups.is_empty(),
        "one physical file cannot duplicate itself"
    );
    assert_eq!(
        report.bytes_hashed, 0,
        "a single representative is not hashed"
    );
    assert_eq!(fs.nodes.len(), before, "detection must not change the tree");
}

#[test]
fn min_size_reparse_git_and_forbidden_are_skipped() {
    let mut fs = DupFs::new();
    fs.dir(r"D:\dup4")
        .file(r"D:\dup4\small.bin", b"xy", id(1, 1), Some(1))
        .file(r"D:\dup4\keep.bin", b"12345678", id(1, 2), Some(1))
        .file(r"D:\dup4\link.bin", b"12345678", id(1, 3), Some(1))
        .reparse(r"D:\dup4\link.bin")
        .file(
            r"D:\dup4\.git\objects\huge.bin",
            b"12345678",
            id(1, 4),
            Some(1),
        )
        .dir(r"D:\dup4\vault")
        .file(r"D:\dup4\vault\secret.bin", b"12345678", id(1, 5), Some(1));
    let before = fs.nodes.len();
    let mut o = opts(&[r"D:\dup4"]);
    o.min_size = 4;
    o.forbidden = vec![PathBuf::from(r"D:\dup4\vault")];

    let report = find_duplicates(&o, &fs);

    assert_eq!(report.scanned, 1, "only keep.bin qualifies");
    assert!(report.skipped >= 4, "skipped={}", report.skipped);
    assert!(report.groups.is_empty(), "{:?}", report.groups);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(fs.nodes.len(), before, "detection must not change the tree");
}

#[test]
fn max_files_truncates_with_error() {
    let mut fs = DupFs::new();
    fs.dir(r"D:\dup5");
    for i in 0..3 {
        fs.file(
            &format!(r"D:\dup5\f{i}.bin"),
            b"same-content",
            id(1, i + 1),
            Some(1),
        );
    }
    let before = fs.nodes.len();
    let mut o = opts(&[r"D:\dup5"]);
    o.max_files = 2;

    let report = find_duplicates(&o, &fs);

    assert!(report.truncated);
    assert_eq!(report.scanned, 2);
    assert!(
        report.errors.iter().any(|e| e.contains("max_files")),
        "{:?}",
        report.errors
    );
    assert_eq!(fs.nodes.len(), before, "detection must not change the tree");
}

#[test]
fn max_bytes_hash_truncates_without_reporting_prefix_matches() {
    let mut fs = DupFs::new();
    let payload = vec![0x5a_u8; 100];
    fs.dir(r"D:\dup6")
        .file(r"D:\dup6\a.bin", &payload, id(1, 1), Some(1))
        .file(r"D:\dup6\b.bin", &payload, id(1, 2), Some(1));
    let before = fs.nodes.len();
    let mut o = opts(&[r"D:\dup6"]);
    o.max_bytes_hash = 50;

    let report = find_duplicates(&o, &fs);

    assert!(report.truncated);
    assert_eq!(
        report.scanned, 2,
        "collection is not budgeted by hash bytes"
    );
    assert_eq!(
        report.bytes_hashed, 0,
        "no file is hashed at all when over budget"
    );
    assert!(
        report.groups.is_empty(),
        "a prefix must never be reported as a match"
    );
    assert!(
        report.errors.iter().any(|e| e.contains("max_bytes_hash")),
        "{:?}",
        report.errors
    );
    assert_eq!(fs.nodes.len(), before, "detection must not change the tree");
}

#[test]
fn file_changed_between_stat_and_hash_is_dropped() {
    let mut fs = DupFs::new();
    fs.dir(r"D:\dup7")
        .file(r"D:\dup7\a.bin", b"abcdefgh", id(1, 1), Some(1))
        .file(r"D:\dup7\b.bin", b"abcdefgh", id(1, 2), Some(1))
        .changes_on_restat(999);
    let before = fs.nodes.len();

    let report = find_duplicates(&opts(&[r"D:\dup7"]), &fs);

    assert!(report.groups.is_empty(), "{:?}", report.groups);
    assert!(
        report.errors.iter().any(|e| e.contains("changed")),
        "{:?}",
        report.errors
    );
    assert_eq!(fs.nodes.len(), before, "detection must not change the tree");
}

#[test]
fn unknown_identity_is_excluded_and_never_inflates_wasted() {
    // A: one valid + one unknown identity → no group, with an explicit error.
    let mut fs = DupFs::new();
    fs.dir(r"D:\dup8a")
        .file(r"D:\dup8a\a.bin", b"identical!", id(1, 1), Some(1))
        .file(r"D:\dup8a\b.bin", b"identical!", None, Some(1));
    let before = fs.nodes.len();
    let report = find_duplicates(&opts(&[r"D:\dup8a"]), &fs);
    assert!(report.groups.is_empty(), "{:?}", report.groups);
    assert!(
        report.errors.iter().any(|e| e.contains("identity")),
        "{:?}",
        report.errors
    );
    assert_eq!(fs.nodes.len(), before, "detection must not change the tree");

    // B: two valid + one unknown identity → group of exactly two, wasted = one copy only.
    let mut fs = DupFs::new();
    fs.dir(r"D:\dup8b")
        .file(r"D:\dup8b\a.bin", b"another!!!", id(2, 1), Some(1))
        .file(r"D:\dup8b\b.bin", b"another!!!", id(2, 2), Some(1))
        .file(r"D:\dup8b\c.bin", b"another!!!", None, Some(1));
    let before = fs.nodes.len();
    let report = find_duplicates(&opts(&[r"D:\dup8b"]), &fs);
    assert_eq!(report.groups.len(), 1, "{:?}", report.groups);
    assert_eq!(report.groups[0].files.len(), 2);
    assert_eq!(report.groups[0].size_each, 10);
    assert_eq!(
        report.groups[0].wasted, 10,
        "unknown identity must not add a copy"
    );
    assert!(
        report.errors.iter().any(|e| e.contains("identity")),
        "{:?}",
        report.errors
    );
    assert_eq!(fs.nodes.len(), before, "detection must not change the tree");
}

#[test]
fn content_change_with_same_size_and_mtime_is_dropped() {
    let mut fs = DupFs::new();
    fs.dir(r"D:\dup9")
        .file(r"D:\dup9\a.bin", b"abcdefgh", id(1, 1), Some(1))
        .file(r"D:\dup9\b.bin", b"abcdefgh", id(1, 2), Some(1))
        // Hash reads are #0/#1; confirmation reads #2/#3 serve this same-length new content.
        // mtime and identity stay unchanged, so only the re-hash after re-stat can catch it.
        .mutates_content_after_hash(b"ABCDEFGH", 2);
    let before = fs.nodes.len();

    let report = find_duplicates(&opts(&[r"D:\dup9"]), &fs);

    assert!(report.groups.is_empty(), "{:?}", report.groups);
    assert!(
        report.errors.iter().any(|e| e.contains("changed")),
        "{:?}",
        report.errors
    );
    assert_eq!(fs.nodes.len(), before, "detection must not change the tree");
}

#[test]
fn drive_root_scan_finds_small_duplicates_in_root_and_subfolder() {
    let mut fs = DupFs::new();
    // `D:\` is materialised explicitly: a drive root is now a legitimate opt-in scan root.
    fs.dir(r"D:\")
        .file(r"D:\A.txt", b"tiny-dup", id(1, 1), Some(1))
        .file(r"D:\stu\A.txt", b"tiny-dup", id(1, 2), Some(1))
        .file(r"D:\uniq.txt", b"other", id(1, 3), Some(1));
    let before = fs.nodes.len();
    let mut o = opts(&[r"D:\"]);
    o.min_size = 0; // sub-1KB files must be considered

    let report = find_duplicates(&o, &fs);

    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.scanned, 3);
    assert_eq!(report.groups.len(), 1, "{:?}", report.groups);
    let g = &report.groups[0];
    assert_eq!(g.size_each, 8);
    assert_eq!(g.wasted, 8, "two distinct copies waste exactly one");
    assert_eq!(g.files.len(), 2);
    assert_eq!(
        g.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
        vec![r"D:\A.txt", r"D:\stu\A.txt"],
        "root file and nested file must be grouped, sorted by path"
    );
    assert_eq!(fs.nodes.len(), before, "detection must not change the tree");
}

#[test]
fn zero_byte_files_are_skipped_even_when_min_size_is_zero() {
    let mut fs = DupFs::new();
    fs.dir(r"D:\zero")
        .file(r"D:\zero\a.txt", b"", id(1, 1), Some(1))
        .file(r"D:\zero\b.txt", b"", id(1, 2), Some(1))
        .file(r"D:\zero\c.txt", b"x", id(1, 3), Some(1));

    let mut o = opts(&[r"D:\zero"]);
    o.min_size = 0;
    let report = find_duplicates(&o, &fs);

    assert_eq!(report.scanned, 1, "only the non-empty file is a candidate");
    assert_eq!(report.skipped, 2, "two 0-byte files are skipped");
    assert!(report.groups.is_empty(), "{:?}", report.groups);
    assert_eq!(report.bytes_hashed, 0);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
}

#[test]
fn forbidden_drive_root_is_still_rejected() {
    let mut fs = DupFs::new();
    fs.dir(r"D:\")
        .file(r"D:\A.txt", b"tiny-dup", id(1, 1), Some(1));
    let mut o = opts(&[r"D:\"]);
    o.min_size = 0;
    o.forbidden = vec![PathBuf::from(r"D:\")];

    let report = find_duplicates(&o, &fs);

    assert_eq!(report.scanned, 0);
    assert!(report.groups.is_empty(), "{:?}", report.groups);
    assert!(
        report.errors.iter().any(|e| e.contains("forbidden")),
        "{:?}",
        report.errors
    );
}

#[test]
fn unc_and_reparse_roots_are_still_rejected() {
    let mut fs = DupFs::new();
    fs.dir(r"D:\link").reparse(r"D:\link");

    let unc = find_duplicates(&opts(&[r"\\server\share\pub"]), &fs);
    assert_eq!(unc.scanned, 0);
    assert!(
        unc.errors.iter().any(|e| e.contains("UNC")),
        "{:?}",
        unc.errors
    );

    let reparse = find_duplicates(&opts(&[r"D:\link"]), &fs);
    assert_eq!(reparse.scanned, 0);
    assert!(
        reparse.errors.iter().any(|e| e.contains("reparse")),
        "{:?}",
        reparse.errors
    );
}

#[test]
fn drive_root_scan_reports_truncation() {
    let mut fs = DupFs::new();
    fs.dir(r"D:\")
        .file(r"D:\A.txt", b"dup!", id(1, 1), Some(1))
        .file(r"D:\stu\A.txt", b"dup!", id(1, 2), Some(1));
    let mut o = opts(&[r"D:\"]);
    o.min_size = 0;
    o.max_files = 1;

    let report = find_duplicates(&o, &fs);

    assert!(report.truncated);
    assert_eq!(report.scanned, 1);
    assert!(report.groups.is_empty(), "{:?}", report.groups);
    assert!(
        report.errors.iter().any(|e| e.contains("max_files")),
        "{:?}",
        report.errors
    );
}

#[test]
#[allow(deprecated)]
fn real_fs_detects_two_identical_files_without_deleting_them() {
    let dir = tempfile::tempdir().expect("temp dir");
    // `into_path` disables automatic cleanup: this suite never removes a file or directory.
    let root = strip_verbatim(&std::fs::canonicalize(dir.into_path()).expect("canonicalize root"));
    let a = root.join("a.bin");
    let b = root.join("b.bin");
    std::fs::write(&a, b"real duplicate payload").expect("write a");
    std::fs::write(&b, b"real duplicate payload").expect("write b");

    let o = DupsOptions {
        roots: vec![root],
        forbidden: Vec::new(),
        min_size: 1,
        max_files: 100,
        max_bytes_hash: 1024 * 1024,
        now_secs: 0,
    };
    let report = find_duplicates(&o, &RealFs);

    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.groups.len(), 1, "{:?}", report.groups);
    let g = &report.groups[0];
    assert_eq!(g.files.len(), 2);
    assert_eq!(g.size_each, 22);
    assert_eq!(g.wasted, 22);
    assert_eq!(report.bytes_hashed, 44);
    assert!(
        a.exists() && b.exists(),
        "both fixture files must still exist"
    );
}

#[test]
#[allow(deprecated)]
fn real_fs_finds_small_duplicates_and_skips_zero_bytes_without_deleting_them() {
    let dir = tempfile::tempdir().expect("temp dir");
    // `into_path` disables automatic cleanup: this suite never removes a file or directory.
    let root = strip_verbatim(&std::fs::canonicalize(dir.into_path()).expect("canonicalize root"));
    let sub = root.join("stu");
    std::fs::create_dir(&sub).expect("create subdir");
    let a = root.join("A.txt");
    let b = sub.join("A.txt");
    let empty = root.join("empty.txt");
    std::fs::write(&a, b"tiny-dup").expect("write a");
    std::fs::write(&b, b"tiny-dup").expect("write b");
    std::fs::write(&empty, b"").expect("write empty");

    let o = DupsOptions {
        roots: vec![root],
        forbidden: Vec::new(),
        min_size: 0,
        max_files: 100,
        max_bytes_hash: 1024 * 1024,
        now_secs: 0,
    };
    let report = find_duplicates(&o, &RealFs);

    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.scanned, 2, "0-byte empty.txt must be skipped");
    assert_eq!(report.groups.len(), 1, "{:?}", report.groups);
    assert_eq!(report.groups[0].size_each, 8);
    assert_eq!(report.groups[0].wasted, 8);
    assert_eq!(report.groups[0].files.len(), 2);
    assert!(
        a.exists() && b.exists() && empty.exists(),
        "all fixture files must still exist"
    );
}
