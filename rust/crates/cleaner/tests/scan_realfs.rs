//! Real-filesystem test. Creates a throw-away tree under the OS temp folder and deliberately leaks
//! it via `TempDir::into_path()` — this suite never removes a file or directory.

use cu_cleaner::{
    is_blocked_drive_type, now_unix_secs, scan, strip_verbatim, to_long_path, validate_target,
    RealFs, RuleKind, ScanOptions,
};
use std::fs;

#[test]
#[allow(deprecated)]
fn real_fs_counts_bytes_behind_a_path_longer_than_260_chars() {
    let dir = tempfile::tempdir().expect("temp dir");
    // `into_path` disables the automatic cleanup: nothing in this test ever deletes anything.
    let root = strip_verbatim(&fs::canonicalize(dir.into_path()).expect("canonicalize temp root"));

    let proj = root.join("proj");
    let bin = proj.join("bin");
    fs::create_dir_all(to_long_path(&bin)).expect("create fixture dirs");
    fs::write(to_long_path(&proj.join("p.csproj")), b"<Project />").expect("write project marker");

    let mut deep = bin.clone();
    for i in 0..9 {
        deep = deep.join(format!("seg{i:02}_{}", "x".repeat(28)));
        fs::create_dir(to_long_path(&deep)).expect("create deep dir");
    }
    let payload = deep.join("payload.bin");
    fs::write(to_long_path(&payload), vec![0x5a_u8; 123]).expect("write payload");
    assert!(
        payload.to_string_lossy().len() > 260,
        "fixture path must be long: {}",
        payload.to_string_lossy().len()
    );

    let opts = ScanOptions {
        roots: vec![root],
        forbidden: Vec::new(),
        skip_recent_days: 0,
        now_secs: now_unix_secs(),
        max_items: 100,
        max_dirs: 100_000,
    };
    let manifest = scan(&opts, &RealFs);

    assert!(manifest.errors.is_empty(), "{:?}", manifest.errors);
    assert_eq!(manifest.items.len(), 1, "{:?}", manifest.items);
    assert_eq!(manifest.items[0].kind, RuleKind::Bin);
    assert_eq!(manifest.items[0].size, 123);
    assert_eq!(manifest.total, 123);
    assert!(
        !manifest.items[0].path.starts_with(r"\\?\"),
        "display path must be normal"
    );
}

#[test]
fn blocked_drive_types_include_mapped_network_drives() {
    // 1 = NO_ROOT, 2 = REMOVABLE, 4 = REMOTE (mapped network), 5 = CD-ROM.
    for ty in [1, 2, 4, 5] {
        assert!(is_blocked_drive_type(ty), "type {ty} must be blocked");
    }
    for ty in [0, 3, 6] {
        assert!(!is_blocked_drive_type(ty), "type {ty} must be allowed");
    }
}

#[test]
#[allow(deprecated)]
fn validate_target_rejects_forbidden_and_out_of_root_items() {
    let dir = tempfile::tempdir().expect("temp dir");
    let root = strip_verbatim(&fs::canonicalize(dir.into_path()).expect("canonicalize temp root"));
    let proj = root.join("proj");
    let bin = proj.join("bin");
    fs::create_dir_all(&bin).expect("create fixture dirs");
    fs::write(proj.join("p.csproj"), b"<Project />").expect("write project marker");
    fs::write(bin.join("f.dll"), vec![0x11_u8; 7]).expect("write payload");

    let opts = ScanOptions {
        roots: vec![root.clone()],
        forbidden: Vec::new(),
        skip_recent_days: 0,
        now_secs: now_unix_secs(),
        max_items: 100,
        max_dirs: 10_000,
    };
    let manifest = scan(&opts, &RealFs);
    assert_eq!(manifest.items.len(), 1, "{:?}", manifest.errors);
    assert!(
        validate_target(&manifest.items[0], &opts).is_ok(),
        "freshly scanned item must revalidate"
    );

    let mut blocked = opts.clone();
    blocked.forbidden = vec![bin.clone()];
    let err = validate_target(&manifest.items[0], &blocked).expect_err("forbidden item");
    assert!(err.contains("forbidden"), "{err}");

    let mut other = opts.clone();
    other.roots = vec![root.join("somewhere_else")];
    let err = validate_target(&manifest.items[0], &other).expect_err("item outside roots");
    assert!(err.contains("not inside any configured root"), "{err}");
}
