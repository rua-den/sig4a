//! Tests for the cleanup engine. Every deleter here is a `RecordingDeleter`: no test in this
//! file calls `remove_file`, `remove_dir_all` or any other real deletion API.

use cu_cleaner::{
    clean_with, rule_matches_name, CleanRecord, DeleteOutcome, FileJournal, Item, Journal,
    Manifest, Mode, RealFs, RecordingDeleter, RuleKind, ScanOptions, PHASE_CLEANING, PHASE_DONE,
};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// `TempDir::keep()` (formerly `into_path()`) keeps the directory instead of deleting it on Drop:
/// this suite never removes anything from disk.
fn temp_root() -> PathBuf {
    tempfile::tempdir().unwrap().keep()
}

fn rule_dir(root: &Path, parts: &[&str]) -> PathBuf {
    let mut path = root.to_path_buf();
    for part in parts {
        path.push(part);
    }
    fs::create_dir_all(&path).unwrap();
    path
}

fn item_at(path: &Path, kind: RuleKind) -> Item {
    Item::new(path.to_string_lossy().into_owned(), 1, kind)
}

/// Put a guard file inside `dir` (the parent of an artefact) so the stricter
/// `validate_item_kind` re-check still accepts items that were valid at scan time.
fn guard_in(dir: &Path, name: &str) {
    fs::write(dir.join(name), b"x").unwrap();
}

fn manifest_for(items: Vec<Item>) -> Manifest {
    let mut manifest = Manifest::new();
    manifest.items = items;
    manifest
}

fn opts(root: PathBuf) -> ScanOptions {
    ScanOptions {
        roots: vec![root],
        forbidden: Vec::new(),
        skip_recent_days: 0,
        now_secs: 0,
        max_items: 100,
        max_dirs: 1000,
    }
}

#[derive(Default)]
struct MemoryJournal {
    records: Mutex<Vec<CleanRecord>>,
}

impl Journal for MemoryJournal {
    fn record(&self, r: &CleanRecord) -> io::Result<()> {
        self.records.lock().unwrap().push(r.clone());
        Ok(())
    }
}

struct FailingJournal;

impl Journal for FailingJournal {
    fn record(&self, _r: &CleanRecord) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "journal unavailable",
        ))
    }
}

#[cfg(windows)]
fn symlink_dir(target: &Path, link: &Path) -> io::Result<()> {
    std::os::windows::fs::symlink_dir(target, link)
}

#[cfg(not(windows))]
fn symlink_dir(target: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

/// A dangling directory symlink is a real reparse point whose canonicalize fails, so
/// `validate_target` sees the reparse attribute instead of its (nonexistent) target.
fn dangling_symlink(root: &Path, parts: &[&str]) -> Option<PathBuf> {
    let link = rule_dir_parent(root, parts);
    let missing = root.join("__target_that_does_not_exist__");
    match symlink_dir(&missing, &link) {
        Ok(()) => Some(link),
        Err(e) => {
            eprintln!("skip reparse check: cannot create symlink ({e})");
            None
        }
    }
}

fn rule_dir_parent(root: &Path, parts: &[&str]) -> PathBuf {
    let mut path = root.to_path_buf();
    for part in parts {
        path.push(part);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    path
}

#[test]
fn selects_only_requested_ids_in_the_given_order() {
    let root = temp_root();
    let a = rule_dir(&root, &["app1", "bin"]);
    let b = rule_dir(&root, &["app2", "obj"]);
    let c = rule_dir(&root, &["app3", ".vs"]);
    guard_in(a.parent().unwrap(), "app1.csproj");
    guard_in(b.parent().unwrap(), "app2.csproj");
    let ia = item_at(&a, RuleKind::Bin);
    let ib = item_at(&b, RuleKind::Obj);
    let ic = item_at(&c, RuleKind::Vs);
    let manifest = manifest_for(vec![ia.clone(), ib.clone(), ic.clone()]);
    let deleter = RecordingDeleter::new();
    let journal = MemoryJournal::default();
    let ids = vec![ib.id.clone(), ia.id.clone()];

    let outcomes = clean_with(
        &manifest,
        &ids,
        Mode::Recycle,
        &opts(root),
        &RealFs,
        &deleter,
        &journal,
    );

    assert_eq!(outcomes.len(), 2);
    assert!(outcomes.iter().all(|o| o.ok && o.err.is_none()));
    let calls = deleter.calls.lock().unwrap().clone();
    assert_eq!(
        calls,
        vec![
            (ib.id.clone(), Mode::Recycle),
            (ia.id.clone(), Mode::Recycle),
        ]
    );
}

#[test]
fn failure_on_a_middle_id_does_not_stop_the_loop() {
    let root = temp_root();
    let a = rule_dir(&root, &["app1", "bin"]);
    let b = rule_dir(&root, &["app2", "obj"]);
    let c = rule_dir(&root, &["app3", "node_modules"]);
    guard_in(a.parent().unwrap(), "app1.csproj");
    guard_in(b.parent().unwrap(), "app2.csproj");
    guard_in(c.parent().unwrap(), "package.json");
    let ia = item_at(&a, RuleKind::Bin);
    let ib = item_at(&b, RuleKind::Obj);
    let ic = item_at(&c, RuleKind::NodeModules);
    let manifest = manifest_for(vec![ia.clone(), ib.clone(), ic.clone()]);
    let deleter = RecordingDeleter {
        calls: Arc::new(Mutex::new(Vec::new())),
        fail_on: Some(ib.id.clone()),
    };
    let journal = MemoryJournal::default();
    let ids = vec![ia.id.clone(), ib.id.clone(), ic.id.clone()];

    let outcomes = clean_with(
        &manifest,
        &ids,
        Mode::Permanent,
        &opts(root),
        &RealFs,
        &deleter,
        &journal,
    );

    assert_eq!(deleter.calls.lock().unwrap().len(), 3);
    assert!(outcomes[0].ok && outcomes[2].ok);
    assert_eq!(outcomes[1].id, ib.id);
    assert!(!outcomes[1].ok);
    assert!(outcomes[1]
        .err
        .as_deref()
        .is_some_and(|e| e.contains("simulated failure")));
}

#[test]
fn journal_gets_initial_snapshot_per_item_snapshots_and_final_done() {
    let root = temp_root();
    let a = rule_dir(&root, &["app1", "bin"]);
    let b = rule_dir(&root, &["app2", "TestResults"]);
    guard_in(a.parent().unwrap(), "app1.csproj");
    guard_in(b.parent().unwrap(), "app2.csproj");
    let ia = item_at(&a, RuleKind::Bin);
    let ib = item_at(&b, RuleKind::TestResults);
    let manifest = manifest_for(vec![ia.clone(), ib.clone()]);
    let deleter = RecordingDeleter::new();
    let journal = MemoryJournal::default();
    let ids = vec![ia.id.clone(), ib.id.clone()];

    clean_with(
        &manifest,
        &ids,
        Mode::Permanent,
        &opts(root),
        &RealFs,
        &deleter,
        &journal,
    );

    let records = journal.records.lock().unwrap().clone();
    assert_eq!(records.len(), 4);
    assert_eq!(records.iter().map(|r| r.phase.as_str()).collect::<Vec<_>>(), vec![PHASE_CLEANING, PHASE_CLEANING, PHASE_CLEANING, PHASE_DONE]);
    assert_eq!(records.iter().map(|r| r.outcomes.len()).collect::<Vec<_>>(), vec![0, 1, 2, 2]);
    assert_eq!(records[0].run_id, records[3].run_id);
    assert_eq!(records[0].mode, Mode::Permanent);
}

#[test]
fn unknown_outside_forbidden_and_reparse_ids_are_rejected_without_deleting() {
    let root = temp_root();
    let outside_root = temp_root();
    let good = rule_dir(&root, &["app", "bin"]);
    let outside = rule_dir(&outside_root, &["app", "obj"]);
    let forbidden = rule_dir(&root, &["secret", "node_modules"]);
    let reparse = dangling_symlink(&root, &["link", "TestResults"]);

    let mut items = vec![
        item_at(&good, RuleKind::Bin),
        item_at(&outside, RuleKind::Obj),
        item_at(&forbidden, RuleKind::NodeModules),
    ];
    if let Some(link) = &reparse {
        items.push(item_at(link, RuleKind::TestResults));
    }
    let manifest = manifest_for(items);

    let mut opts = opts(root.clone());
    opts.forbidden = vec![rule_dir(&root, &["secret"])];
    let mut ids = vec![
        "id-not-in-manifest".to_string(),
        outside.to_string_lossy().into_owned(),
        forbidden.to_string_lossy().into_owned(),
    ];
    if let Some(link) = &reparse {
        ids.push(link.to_string_lossy().into_owned());
    }

    let deleter = RecordingDeleter::new();
    let journal = MemoryJournal::default();
    let outcomes = clean_with(
        &manifest,
        &ids,
        Mode::Permanent,
        &opts,
        &RealFs,
        &deleter,
        &journal,
    );

    assert_eq!(outcomes.len(), ids.len());
    assert!(outcomes.iter().all(|o| !o.ok && o.err.is_some()));
    assert!(outcomes[0]
        .err
        .as_deref()
        .is_some_and(|e| e.contains("not in the manifest")));
    assert!(deleter.calls.lock().unwrap().is_empty());
}

#[test]
fn rule_that_no_longer_matches_the_name_is_rejected() {
    let root = temp_root();
    let path = rule_dir(&root, &["app", "bin"]);
    // The path is still named "bin" but the manifest claims a different rule.
    let item = Item::new(path.to_string_lossy().into_owned(), 1, RuleKind::Obj);
    let manifest = manifest_for(vec![item.clone()]);
    let deleter = RecordingDeleter::new();
    let journal = MemoryJournal::default();

    let outcomes = clean_with(
        &manifest,
        std::slice::from_ref(&item.id),
        Mode::Permanent,
        &opts(root),
        &RealFs,
        &deleter,
        &journal,
    );

    assert_eq!(outcomes.len(), 1);
    assert!(!outcomes[0].ok);
    assert!(outcomes[0]
        .err
        .as_deref()
        .is_some_and(|e| e.contains("rule")));
    assert!(deleter.calls.lock().unwrap().is_empty());
}

#[test]
fn initial_journal_failure_prevents_all_removals() {
    let root = temp_root();
    let a = rule_dir(&root, &["app1", "bin"]);
    let b = rule_dir(&root, &["app2", "obj"]);
    let c = rule_dir(&root, &["app3", ".vs"]);
    guard_in(a.parent().unwrap(), "app1.csproj");
    guard_in(b.parent().unwrap(), "app2.csproj");
    let ia = item_at(&a, RuleKind::Bin);
    let ib = item_at(&b, RuleKind::Obj);
    let ic = item_at(&c, RuleKind::Vs);
    let manifest = manifest_for(vec![ia.clone(), ib.clone(), ic.clone()]);
    let deleter = RecordingDeleter::new();
    let ids = vec![ia.id.clone(), ib.id.clone(), ic.id.clone()];

    let outcomes = clean_with(
        &manifest,
        &ids,
        Mode::Permanent,
        &opts(root),
        &RealFs,
        &deleter,
        &FailingJournal,
    );

    assert!(deleter.calls.lock().unwrap().is_empty());
    assert_eq!(outcomes.len(), 1);
    let last = outcomes.last().unwrap();
    assert_eq!(last.id, "");
    assert!(!last.ok);
    assert_eq!(last.err.as_deref(), Some("không ghi được journal"));
}

#[test]
fn file_journal_writes_readable_json() {
    let path = temp_root().join("cleaner_result.json");
    let record = CleanRecord {
        run_id: "run-42".into(),
        phase: PHASE_CLEANING.into(),
        mode: Mode::Recycle,
        outcomes: vec![DeleteOutcome {
            id: "x".into(),
            ok: true,
            err: None,
        }],
        finished_at: 123,
    };

    FileJournal { path: path.clone() }.record(&record).unwrap();
    let text = fs::read_to_string(&path).unwrap();
    let back: CleanRecord = serde_json::from_str(&text).unwrap();

    assert_eq!(back.phase, PHASE_CLEANING);
    assert_eq!(back.run_id, "run-42");
    assert_eq!(back.mode, Mode::Recycle);
    assert_eq!(back.outcomes, record.outcomes);
    assert_eq!(back.finished_at, 123);
}

#[test]
fn mode_serializes_as_lowercase() {
    assert_eq!(
        serde_json::to_string(&Mode::Permanent).unwrap(),
        "\"permanent\""
    );
    assert_eq!(
        serde_json::to_string(&Mode::Recycle).unwrap(),
        "\"recycle\""
    );
    assert_eq!(
        serde_json::from_str::<Mode>("\"permanent\"").unwrap(),
        Mode::Permanent
    );
    assert_eq!(
        serde_json::from_str::<Mode>("\"recycle\"").unwrap(),
        Mode::Recycle
    );
}

#[test]
fn rule_name_helper_matches_all_five_families() {
    assert!(rule_matches_name(RuleKind::Bin, Path::new("BIN")));
    assert!(rule_matches_name(RuleKind::Obj, Path::new("obj")));
    assert!(rule_matches_name(RuleKind::Vs, Path::new(".vs")));
    assert!(rule_matches_name(
        RuleKind::NodeModules,
        Path::new("node_modules")
    ));
    assert!(rule_matches_name(
        RuleKind::TestResults,
        Path::new("TestResults")
    ));
    assert!(!rule_matches_name(RuleKind::Bin, Path::new("obj")));
}
