//! Cleanup engine and journal built on top of the scan contract.
//!
//! `clean_with` re-validates every id **immediately before** asking a [`Deleter`] to remove it:
//! no raw path from the UI is ever trusted. This module performs no deletion on its own — the
//! only real deletion code lives in [`PermanentDeleter`], which the tests here never invoke.
//!
//! The engine receives an [`FsView`] so all safety checks (target path and rule kind/guard) run
//! through the same injected view the scan used. Tests pass an in-memory view; production callers
//! can use the [`clean`] convenience wrapper, which injects [`RealFs`].

use crate::model::{FsView, Item, Manifest, Mode, RealFs, ScanOptions};
use crate::scan::is_recent;
use crate::{new_run_id, now_unix_secs, path_contains, validate_item_kind, validate_target_with, REPARSE_POINT};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Error returned by a [`Deleter`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelErr(pub String);

impl std::fmt::Display for DelErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for DelErr {}

/// Strategy that actually removes (or recycles) one matched directory.
pub trait Deleter: Send + Sync {
    fn delete(&self, item: &Item, mode: Mode) -> Result<(), DelErr>;
}

/// Result of one attempted deletion, emitted in the same order as the requested ids.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteOutcome {
    pub id: String,
    pub ok: bool,
    pub err: Option<String>,
}

/// One durable progress snapshot written by a [`Journal`].
///
/// NOTE FOR THE APP: when reading `cleaner_result.json` on startup, if `phase == "cleaning"`
/// report "lần trước bị ngắt". NEVER auto-resume: a fresh consent is required before deleting
/// anything left over from an interrupted run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanRecord {
    pub run_id: String,
    /// `"cleaning"` for per-item snapshots, `"done"` for the final snapshot.
    pub phase: String,
    pub mode: Mode,
    pub outcomes: Vec<DeleteOutcome>,
    pub finished_at: u64,
}

pub const PHASE_CLEANING: &str = "cleaning";
pub const PHASE_DONE: &str = "done";

/// Durable sink for progress snapshots.
pub trait Journal {
    fn record(&self, r: &CleanRecord) -> io::Result<()>;
}

/// JSON journal written atomically: `<path>.<pid>.tmp` then rename.
pub struct FileJournal {
    pub path: PathBuf,
}

impl Journal for FileJournal {
    fn record(&self, r: &CleanRecord) -> io::Result<()> {
        let text = serde_json::to_string_pretty(r)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let tmp = PathBuf::from(format!(
            "{}.{}.tmp",
            self.path.display(),
            std::process::id()
        ));
        atomic_write(&self.path, &tmp, &text)
    }
}

/// Write `tmp`, then replace `dest` with a short retry on Windows sharing violations.
/// Deliberately never calls `remove_file`: a stale tmp is preferable to an accidental delete.
fn atomic_write(dest: &Path, tmp: &Path, text: &str) -> io::Result<()> {
    fs::write(tmp, text)?;
    let mut attempt = 0;
    loop {
        match fs::rename(tmp, dest) {
            Ok(()) => return Ok(()),
            Err(e) => {
                if attempt >= 10 || !is_retryable(&e) {
                    return Err(e);
                }
                attempt += 1;
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
}

/// ERROR_ACCESS_DENIED / ERROR_SHARING_VIOLATION / ERROR_LOCK_VIOLATION, or PermissionDenied.
fn is_retryable(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::PermissionDenied || matches!(e.raw_os_error(), Some(5 | 32 | 33))
}

/// In-memory deleter: records `(id, mode)` and **never touches the disk**. Used by every test.
pub struct RecordingDeleter {
    pub calls: Arc<Mutex<Vec<(String, Mode)>>>,
    /// When set, `delete` fails for this id so callers can test error handling.
    pub fail_on: Option<String>,
}

impl RecordingDeleter {
    pub fn new() -> Self {
        Self {
            calls: Arc::new(Mutex::new(Vec::new())),
            fail_on: None,
        }
    }
}

impl Default for RecordingDeleter {
    fn default() -> Self {
        Self::new()
    }
}

impl Deleter for RecordingDeleter {
    fn delete(&self, item: &Item, mode: Mode) -> Result<(), DelErr> {
        self.calls
            .lock()
            .map_err(|e| DelErr(e.to_string()))?
            .push((item.id.clone(), mode));
        if self.fail_on.as_deref() == Some(item.id.as_str()) {
            return Err(DelErr(format!("simulated failure for '{}'", item.id)));
        }
        Ok(())
    }
}

/// Real, long-path-aware permanent deletion. Never called by the tests in this crate.
pub struct PermanentDeleter;

impl Deleter for PermanentDeleter {
    fn delete(&self, item: &Item, _mode: Mode) -> Result<(), DelErr> {
        let path = PathBuf::from(&item.path);
        let long = crate::to_long_path(&path);
        let result = if path.is_dir() {
            fs::remove_dir_all(&long)
        } else {
            fs::remove_file(&long)
        };
        result.map_err(|e| DelErr(format!("failed to delete '{}': {e}", item.path)))
    }
}

fn journal_error_outcome() -> DeleteOutcome {
    DeleteOutcome {
        id: String::new(),
        ok: false,
        err: Some("không ghi được journal".to_string()),
    }
}

fn snapshot(
    journal: &dyn Journal,
    run_id: &str,
    mode: Mode,
    outcomes: &[DeleteOutcome],
    phase: &str,
) -> bool {
    let record = CleanRecord {
        run_id: run_id.to_string(),
        phase: phase.to_string(),
        mode,
        outcomes: outcomes.to_vec(),
        finished_at: now_unix_secs(),
    };
    journal.record(&record).is_ok()
}

/// Recheck the current target tree against the review's age filter. Unknown
/// timestamps, unreadable directories and a partial traversal cannot authorize deletion.
fn revalidate_age(item: &Item, opts: &ScanOptions, fs: &dyn FsView) -> Result<(), String> {
    let age_unknown = || format!("cannot verify build age of target '{}'; rescan before deleting", item.path);
    let check = |modified: Option<u64>| {
        let modified = modified.ok_or_else(age_unknown)?;
        if is_recent(Some(modified), opts.skip_recent_days, opts.now_secs) {
            return Err(format!("target '{}' contains a recent build; rescan before deleting", item.path));
        }
        Ok(())
    };
    let path = Path::new(&item.path);
    let parent = path.parent().ok_or_else(age_unknown)?;
    let entry = fs.strict_entries(parent).map_err(|_| age_unknown())?.into_iter()
        .find(|entry| path_contains(path, &entry.path) && path_contains(&entry.path, path))
        .ok_or_else(age_unknown)?;
    if !entry.is_dir || entry.attrs & REPARSE_POINT != 0 {
        return Err(age_unknown());
    }
    check(entry.modified)?;
    let mut pending = vec![path.to_path_buf()];
    let mut examined = 0usize;
    while let Some(dir) = pending.pop() {
        if examined >= opts.max_dirs || crate::has_reparse_component(&dir, fs) {
            return Err(age_unknown());
        }
        examined += 1;
        for entry in fs.strict_entries(&dir).map_err(|_| age_unknown())? {
            check(entry.modified)?;
            // Never follow symbolic links or junctions to prove this target is old.
            if entry.is_dir && entry.attrs & REPARSE_POINT == 0 {
                pending.push(entry.path);
            }
        }
    }
    Ok(())
}

/// Re-validate one manifest item right before deletion. `validate_target_with` covers roots,
/// forbidden subtrees, reparse points and blocked drives; `validate_item_kind` additionally checks
/// the rule name **and** its guard (project marker / `package.json` / `.trx`) through the same
/// injected [`FsView`] the scan used. Both checks go through `fs` so a mock can simulate changes
/// that happen between the scan and the deletion.
fn revalidate(item: &Item, opts: &ScanOptions, fs: &dyn FsView) -> Result<(), String> {
    validate_target_with(item, opts, fs)?;
    if !validate_item_kind(item, fs) {
        return Err(format!(
            "rule {:?} no longer matches item '{}' (name or guard changed)",
            item.kind, item.path
        ));
    }
    if opts.skip_recent_days > 0 {
        revalidate_age(item, opts, fs)?;
        // Traversal may take time. Re-check the original selected path and protected
        // roots now, immediately before handing it to the deleter.
        validate_target_with(item, opts, fs)?;
    }
    Ok(())
}

/// Delete the manifest items whose `id` appears in `ids`, in that exact order.
///
/// Every id is re-validated (id in manifest -> `validate_target_with` -> `validate_item_kind`)
/// through the injected `fs` before the deleter is called; a rejected id yields an error outcome
/// and the loop continues. A snapshot is written before the first delete, after **every** id, and with
/// `phase = "done"`. If a journal write fails, remaining items are skipped and a single
/// `{ id: "", err: "không ghi được journal" }` outcome is appended.
pub fn clean_with(
    manifest: &Manifest,
    ids: &[String],
    mode: Mode,
    opts: &ScanOptions,
    fs: &dyn FsView,
    deleter: &dyn Deleter,
    journal: &dyn Journal,
) -> Vec<DeleteOutcome> {
    let run_id = new_run_id();
    let mut outcomes: Vec<DeleteOutcome> = Vec::with_capacity(ids.len());
    if !snapshot(journal, &run_id, mode, &outcomes, PHASE_CLEANING) {
        return vec![journal_error_outcome()];
    }

    for id in ids {
        let outcome = match manifest
            .items
            .iter()
            .find(|item| item.id.as_str() == id.as_str())
        {
            None => DeleteOutcome {
                id: id.clone(),
                ok: false,
                err: Some(format!("id '{id}' is not in the manifest")),
            },
            Some(item) => match revalidate(item, opts, fs) {
                Err(err) => DeleteOutcome {
                    id: id.clone(),
                    ok: false,
                    err: Some(err),
                },
                Ok(()) => match deleter.delete(item, mode) {
                    Ok(()) => DeleteOutcome {
                        id: id.clone(),
                        ok: true,
                        err: None,
                    },
                    Err(e) => DeleteOutcome {
                        id: id.clone(),
                        ok: false,
                        err: Some(e.0),
                    },
                },
            },
        };
        outcomes.push(outcome);
        if !snapshot(journal, &run_id, mode, &outcomes, PHASE_CLEANING) {
            outcomes.push(journal_error_outcome());
            return outcomes;
        }
    }

    if !snapshot(journal, &run_id, mode, &outcomes, PHASE_DONE) {
        outcomes.push(journal_error_outcome());
    }
    outcomes
}

/// Convenience wrapper around [`clean_with`] that injects the real filesystem view. Callers that
/// need to mock filesystem changes between scan and deletion should call `clean_with` directly.
pub fn clean(
    manifest: &Manifest,
    ids: &[String],
    mode: Mode,
    opts: &ScanOptions,
    deleter: &dyn Deleter,
    journal: &dyn Journal,
) -> Vec<DeleteOutcome> {
    clean_with(manifest, ids, mode, opts, &RealFs, deleter, journal)
}
