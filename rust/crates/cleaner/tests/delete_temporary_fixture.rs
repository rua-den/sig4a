//! Real permanent deletion is restricted to a new TempDir fixture. Recycle flags
//! are covered separately with an injected shell, not the runner's desktop bin.
use cu_cleaner::{clean, scan, FileJournal, Mode, PermanentDeleter, RealFs, ScanOptions, PHASE_DONE};
use std::fs;

#[test]
fn real_permanent_delete_removes_only_the_selected_artifact_in_tempdir() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("scope");
    let selected = root.join("selected/node_modules");
    let retained = root.join("retained/node_modules");
    for item in [&selected, &retained] {
        fs::create_dir_all(item).unwrap();
        fs::write(item.parent().unwrap().join("package.json"), "{}").unwrap();
        fs::write(item.join("fixture.bin"), b"temporary fixture").unwrap();
    }
    let source = root.join("selected/ordinary.rs");
    fs::write(&source, b"ordinary source").unwrap();
    let opts = ScanOptions {
        roots: vec![root],
        forbidden: Vec::new(),
        skip_recent_days: 0,
        now_secs: cu_cleaner::now_unix_secs(),
        max_items: 10,
        max_dirs: 100,
    };
    let manifest = scan(&opts, &RealFs);
    assert_eq!(manifest.items.len(), 2, "{manifest:?}");
    let target = manifest.items.iter().find(|item| item.path.replace('\\', "/").ends_with("/selected/node_modules")).unwrap();
    assert!(selected.starts_with(tmp.path()) && selected.is_dir());
    let journal = FileJournal { path: tmp.path().join("cleaner-journal.json") };
    let out = clean(&manifest, &[target.id.clone()], Mode::Permanent, &opts, &PermanentDeleter, &journal);
    assert!(out[0].ok, "{out:?}");
    assert!(!selected.exists());
    assert_eq!(fs::read(retained.join("fixture.bin")).unwrap(), b"temporary fixture");
    assert_eq!(fs::read(source).unwrap(), b"ordinary source");
    let run: cu_cleaner::CleanRecord = serde_json::from_slice(&fs::read(&journal.path).unwrap()).unwrap();
    assert_eq!(run.phase, PHASE_DONE);
    assert_eq!(run.outcomes.len(), 1);
}
