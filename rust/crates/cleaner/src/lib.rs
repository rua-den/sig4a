//! `cu-cleaner` — read-only scanner for build artefacts.
//!
//! This crate deliberately has no GUI or `cu-core` dependency. The scan phase only walks a
//! filesystem view, matches five artefact rules and returns a [`Manifest`]. The cleanup phase
//! ([`clean_with`]) re-validates every id and delegates destructive work to a [`Deleter`]; it
//! never deletes anything by itself.
//!
//! ```no_run
//! use cu_cleaner::{scan, RealFs, ScanOptions};
//! use std::path::PathBuf;
//!
//! let opts = ScanOptions {
//!     roots: vec![PathBuf::from(r"C:\work\myproj")],
//!     forbidden: vec![PathBuf::from(r"C:\Users\me")],
//!     skip_recent_days: 7,
//!     now_secs: cu_cleaner::now_unix_secs(),
//!     max_items: 5000,
//!     max_dirs: 200_000,
//! };
//! let manifest = scan(&opts, &RealFs);
//! println!("{} matches, {} bytes", manifest.matched, manifest.total);
//! ```

mod delete;
mod dups;
mod fs;
mod model;
mod recycle;
mod scan;

pub use delete::{
    clean, clean_with, CleanRecord, DelErr, DeleteOutcome, Deleter, FileJournal, Journal,
    PermanentDeleter, RecordingDeleter, PHASE_CLEANING, PHASE_DONE,
};
pub use dups::{
    find_duplicates, DupFile, DupGroup, DupsOptions, DupsReport, FileId, DEFAULT_MAX_BYTES_HASH,
    DEFAULT_MAX_FILES, DEFAULT_MIN_SIZE,
};
pub use fs::{is_blocked_drive_type, path_contains, strip_verbatim, to_long_path, REPARSE_POINT};
pub use model::{
    EntryInfo, FsView, HeavyEntry, Item, Manifest, Mode, RealFs, RuleKind, ScanOptions,
};
pub use recycle::{
    build_double_nul_buffer, ShellDeleter, ShellFn, ShellOp, ShellStatus, FOF_ALLOWUNDO,
    FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, RECYCLE_FLAGS,
};
pub use scan::{rule_matches_name, scan, validate_item_kind};

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds since the Unix epoch — used for `created_at`.
pub fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Unique-enough run identifier from nanoseconds + pid, without pulling in a UUID dependency.
pub fn new_run_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{nanos:x}-{}", std::process::id())
}

/// Re-validate one manifest item against the current filesystem **immediately before** any
/// destructive action. Shared by the scan and cleanup phases so Agent C does not copy safety logic:
/// absolute path, not UNC / drive root / reparse point / removable or mapped drive, outside every
/// forbidden subtree, and inside at least one configured root. This function never writes anything.
///
/// Uses the real filesystem view. Tests and callers that need to inject a mock view (e.g. to
/// simulate an item turning into a reparse point right before deletion) should call
/// [`validate_target_with`] instead.
pub fn validate_target(item: &Item, opts: &ScanOptions) -> Result<(), String> {
    validate_target_with(item, opts, &RealFs)
}

/// Injectable counterpart of [`validate_target`]. All safety checks are identical; only the
/// [`FsView`] used for the `attrs` probe is supplied by the caller. Never writes anything.
pub fn validate_target_with(
    item: &Item,
    opts: &ScanOptions,
    fs: &dyn FsView,
) -> Result<(), String> {
    use crate::fs::{canonical_for, is_removable, is_root_dir, is_unc};

    if item.id != item.path {
        return Err(format!("item id/path mismatch: '{}'", item.id));
    }
    let path = PathBuf::from(&item.path);
    if !path.is_absolute() {
        return Err(format!("target '{}' is not absolute", item.path));
    }
    if is_unc(&path) {
        return Err(format!("target '{}' is a UNC path", item.path));
    }
    let canonical = std::fs::canonicalize(&path)
        .ok()
        .map(|p| strip_verbatim(&p));
    let target = canonical.clone().unwrap_or_else(|| path.clone());
    if is_root_dir(&target) {
        return Err(format!("target '{}' is a drive/share root", item.path));
    }
    let Some(attrs) = fs.attrs(&target) else {
        return Err(format!("target '{}' is missing or unreadable", item.path));
    };
    if attrs & REPARSE_POINT != 0 {
        return Err(format!("target '{}' became a reparse point", item.path));
    }
    // Probe the real drive type only for the real filesystem: a mock's fake `D:\` path may
    // canonicalise to a real drive on the test machine, which must not affect its result.
    if fs.is_real_fs() && canonical.is_some() && is_removable(&target) {
        return Err(format!("target '{}' is on a blocked drive type", item.path));
    }
    for forbidden in &opts.forbidden {
        let forbidden = canonical_for(forbidden);
        if path_contains(&forbidden, &target) {
            return Err(format!(
                "target '{}' lies inside forbidden path '{}'",
                item.path,
                forbidden.display()
            ));
        }
    }
    let inside_root = opts
        .roots
        .iter()
        .any(|root| path_contains(&canonical_for(root), &target));
    if !inside_root {
        return Err(format!(
            "target '{}' is not inside any configured root",
            item.path
        ));
    }
    Ok(())
}
