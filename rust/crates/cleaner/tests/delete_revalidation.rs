//! Revalidation tests using an injected in-memory [`FsView`]. No test in this file performs a real
//! filesystem deletion: the only "removal" is `MemoryFs::forget`, which drops a node from a `Vec`,
//! and every delete goes through `RecordingDeleter`.

use cu_cleaner::{
    clean_with, validate_item_kind, validate_target_with, CleanRecord, DeleteOutcome, EntryInfo,
    FsView, Item, Journal, Manifest, Mode, RecordingDeleter, RuleKind, ScanOptions, REPARSE_POINT,
};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Clone)]
struct Node {
    path: PathBuf,
    is_dir: bool,
    attrs: u32,
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

    fn dir(&mut self, path: &str) -> &mut Self {
        self.nodes.push(Node {
            path: PathBuf::from(path),
            is_dir: true,
            attrs: 0,
        });
        self
    }

    fn file(&mut self, path: &str) -> &mut Self {
        self.nodes.push(Node {
            path: PathBuf::from(path),
            is_dir: false,
            attrs: 0,
        });
        self
    }

    /// Simulate the item becoming a reparse point after the scan.
    fn set_reparse(&mut self, path: &str) {
        let wanted = key(Path::new(path));
        if let Some(node) = self.nodes.iter_mut().find(|n| key(&n.path) == wanted) {
            node.attrs |= REPARSE_POINT;
        }
    }

    /// Simulate a guard file disappearing after the scan. In-memory only: no real delete API.
    fn forget(&mut self, path: &str) {
        let wanted = key(Path::new(path));
        self.nodes.retain(|n| key(&n.path) != wanted);
    }
}

impl FsView for MemoryFs {
    fn entries(&self, dir: &Path) -> io::Result<Vec<EntryInfo>> {
        let wanted = key(dir);
        if !self
            .nodes
            .iter()
            .any(|n| key(&n.path) == wanted && n.is_dir)
        {
            return Err(io::Error::new(io::ErrorKind::NotFound, "no such directory"));
        }
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
                len: 1,
                modified: None,
            })
            .collect())
    }

    fn attrs(&self, path: &Path) -> Option<u32> {
        let wanted = key(path);
        self.nodes
            .iter()
            .find(|n| key(&n.path) == wanted)
            .map(|n| n.attrs)
    }
}

#[derive(Default)]
struct MemoryJournal {
    #[allow(dead_code)]
    records: Mutex<Vec<CleanRecord>>,
}

impl Journal for MemoryJournal {
    fn record(&self, r: &CleanRecord) -> io::Result<()> {
        self.records.lock().unwrap().push(r.clone());
        Ok(())
    }
}

fn opts(root: &str) -> ScanOptions {
    ScanOptions {
        roots: vec![PathBuf::from(root)],
        forbidden: Vec::new(),
        skip_recent_days: 0,
        now_secs: 0,
        max_items: 100,
        max_dirs: 1000,
    }
}

fn manifest_of(item: &Item) -> Manifest {
    Manifest {
        items: vec![item.clone()],
        ..Manifest::new()
    }
}

#[test]
fn item_turning_into_reparse_before_delete_is_rejected_without_deleting() {
    let mut fs = MemoryFs::new();
    fs.dir(r"D:\rv\app")
        .file(r"D:\rv\app\p.csproj")
        .dir(r"D:\rv\app\bin");
    let item = Item::new(r"D:\rv\app\bin".into(), 1, RuleKind::Bin);
    let o = opts(r"D:\rv");
    assert!(validate_target_with(&item, &o, &fs).is_ok());
    assert!(validate_item_kind(&item, &fs), "valid at scan time");

    // Between scan and cleanup the target becomes a junction/symlink.
    fs.set_reparse(r"D:\rv\app\bin");
    assert!(validate_target_with(&item, &o, &fs)
        .unwrap_err()
        .contains("reparse"));

    let deleter = RecordingDeleter::new();
    let outcomes = clean_with(
        &manifest_of(&item),
        std::slice::from_ref(&item.id),
        Mode::Permanent,
        &o,
        &fs,
        &deleter,
        &MemoryJournal::default(),
    );

    assert_eq!(outcomes.len(), 1);
    assert!(!outcomes[0].ok);
    assert!(
        outcomes[0]
            .err
            .as_deref()
            .is_some_and(|e| e.contains("reparse")),
        "{:?}",
        outcomes[0]
    );
    assert!(deleter.calls.lock().unwrap().is_empty());
}

#[test]
fn missing_package_json_guard_is_rejected_without_deleting() {
    let mut fs = MemoryFs::new();
    fs.dir(r"D:\rn\app")
        .file(r"D:\rn\app\package.json")
        .dir(r"D:\rn\app\node_modules")
        .file(r"D:\rn\app\node_modules\x.js");
    let item = Item::new(r"D:\rn\app\node_modules".into(), 1, RuleKind::NodeModules);
    let o = opts(r"D:\rn");
    assert!(validate_target_with(&item, &o, &fs).is_ok());
    assert!(validate_item_kind(&item, &fs), "valid at scan time");

    // The sibling package.json disappears before cleanup — name still matches, guard does not.
    fs.forget(r"D:\rn\app\package.json");
    assert!(!validate_item_kind(&item, &fs));

    let deleter = RecordingDeleter::new();
    let outcomes = clean_with(
        &manifest_of(&item),
        std::slice::from_ref(&item.id),
        Mode::Recycle,
        &o,
        &fs,
        &deleter,
        &MemoryJournal::default(),
    );

    assert_eq!(outcomes.len(), 1);
    assert!(!outcomes[0].ok);
    assert!(
        outcomes[0]
            .err
            .as_deref()
            .is_some_and(|e| e.contains("rule")),
        "{:?}",
        outcomes[0]
    );
    assert!(deleter.calls.lock().unwrap().is_empty());
}

#[test]
fn missing_project_marker_guard_is_rejected_without_deleting() {
    let mut fs = MemoryFs::new();
    fs.dir(r"D:\rc\app")
        .file(r"D:\rc\app\p.csproj")
        .dir(r"D:\rc\app\bin")
        .file(r"D:\rc\app\bin\a.dll");
    let item = Item::new(r"D:\rc\app\bin".into(), 1, RuleKind::Bin);
    let o = opts(r"D:\rc");
    assert!(validate_item_kind(&item, &fs), "valid at scan time");

    fs.forget(r"D:\rc\app\p.csproj");
    assert!(!validate_item_kind(&item, &fs));

    let deleter = RecordingDeleter::new();
    let outcomes = clean_with(
        &manifest_of(&item),
        std::slice::from_ref(&item.id),
        Mode::Permanent,
        &o,
        &fs,
        &deleter,
        &MemoryJournal::default(),
    );

    assert_eq!(outcomes.len(), 1);
    assert!(!outcomes[0].ok);
    assert!(deleter.calls.lock().unwrap().is_empty());
}

#[test]
fn valid_item_is_deleted_through_the_injected_view() {
    let mut fs = MemoryFs::new();
    fs.dir(r"D:\rok\app")
        .file(r"D:\rok\app\p.csproj")
        .dir(r"D:\rok\app\bin")
        .file(r"D:\rok\app\bin\a.dll");
    let item = Item::new(r"D:\rok\app\bin".into(), 1, RuleKind::Bin);
    let o = opts(r"D:\rok");
    let deleter = RecordingDeleter::new();

    let outcomes = clean_with(
        &manifest_of(&item),
        std::slice::from_ref(&item.id),
        Mode::Permanent,
        &o,
        &fs,
        &deleter,
        &MemoryJournal::default(),
    );

    assert_eq!(
        outcomes,
        vec![DeleteOutcome {
            id: item.id.clone(),
            ok: true,
            err: None,
        }]
    );
    assert_eq!(
        deleter.calls.lock().unwrap().as_slice(),
        &[(item.id.clone(), Mode::Permanent)]
    );
}
