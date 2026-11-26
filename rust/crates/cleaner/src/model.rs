//! Public data contract of the scan phase.
//!
//! Everything here is intentionally free of GUI / `cu-core` dependencies: other agents can link this
//! crate from the app, a CLI, or tests without pulling in the rest of the port.

use crate::dups::FileId;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

/// What a later (out-of-scope here) cleanup pass should do with the matched items.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Permanent,
    Recycle,
}

/// The five artefact families recognised by the scanner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RuleKind {
    Bin,
    Obj,
    Vs,
    NodeModules,
    TestResults,
}

/// One matched directory, collapsed into a single entry. `id` is always the display path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub id: String,
    pub path: String,
    pub size: u64,
    pub kind: RuleKind,
}

impl Item {
    /// Build an item, enforcing the `id == path` invariant of the contract.
    pub fn new(path: String, size: u64, kind: RuleKind) -> Self {
        Self {
            id: path.clone(),
            path,
            size,
            kind,
        }
    }
}

/// One heavy file or folder for the dashboard's size chart. `path` is a normal display path
/// (never prefixed with `\\?\`), `size` is bytes. At most eight of each kind are kept.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HeavyEntry {
    pub path: String,
    pub size: u64,
}

/// Result of one scan. Serialisable so a later phase can persist it as a manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub run_id: String,
    pub created_at: u64,
    pub roots: Vec<String>,
    pub items: Vec<Item>,
    pub total: u64,
    pub matched: usize,
    pub scanned: usize,
    pub skipped: usize,
    pub errors: Vec<String>,
    /// Heaviest files, at most 8. Defaulted so older journals/manifests still deserialize.
    #[serde(default)]
    pub top_files: Vec<HeavyEntry>,
    /// Heaviest matched-item parent folders, at most 8. Defaulted for older JSON.
    #[serde(default)]
    pub top_folders: Vec<HeavyEntry>,
}

impl Manifest {
    pub fn new() -> Self {
        Self {
            run_id: crate::new_run_id(),
            created_at: crate::now_unix_secs(),
            roots: Vec::new(),
            items: Vec::new(),
            total: 0,
            matched: 0,
            scanned: 0,
            skipped: 0,
            errors: Vec::new(),
            top_files: Vec::new(),
            top_folders: Vec::new(),
        }
    }
}

impl Default for Manifest {
    fn default() -> Self {
        Self::new()
    }
}

/// One directory entry as seen through [`FsView`]. `attrs` carries the raw Windows file attributes
/// so callers can detect reparse points without relying on `is_symlink()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryInfo {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub attrs: u32,
    pub len: u64,
    pub modified: Option<u64>,
}

/// Minimal filesystem abstraction: the scanner only ever reads through this trait.
pub trait FsView: Send + Sync {
    fn entries(&self, dir: &Path) -> io::Result<Vec<EntryInfo>>;
    fn attrs(&self, path: &Path) -> Option<u32>;

    /// Stable physical identity (volume + index) used to collapse hard links. Defaults to `None`
    /// so existing in-memory test views keep compiling; [`RealFs`] overrides it with real metadata.
    fn identity(&self, _path: &Path) -> Option<FileId> {
        None
    }

    /// Full bytes of a file, used by the detection-only duplicate finder. The default reads the
    /// real filesystem (long-path aware); in-memory views override it to serve virtual content.
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        std::fs::read(crate::fs::to_long_path(path))
    }

    /// True only for the real filesystem. Win32 drive-type probes (`is_removable`) must be skipped
    /// for in-memory views: a fake `D:\` path can canonicalise to a real drive on the test machine
    /// and make results depend on that machine's drives. Defaults to `false`; [`RealFs`] overrides.
    fn is_real_fs(&self) -> bool {
        false
    }
}

/// The real, read-only filesystem view (long-path aware on Windows).
pub struct RealFs;

/// Scan configuration. `forbidden` is supplied by the caller (DATA, HOME, executable directory and
/// system folders) — the scanner never guesses those on its own.
#[derive(Debug, Clone)]
pub struct ScanOptions {
    pub roots: Vec<PathBuf>,
    pub forbidden: Vec<PathBuf>,
    pub skip_recent_days: u64,
    /// Caller-supplied "now" in seconds since epoch, so `skip_recent_days` is deterministic in tests.
    pub now_secs: u64,
    pub max_items: usize,
    pub max_dirs: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_manifest_json_without_chart_fields_deserialises_to_empty() {
        // Manifest từ journal/JSON cũ (trước khi có biểu đồ) vẫn phải đọc được.
        let old = r#"{
            "run_id":"r1","created_at":42,"roots":["D:\\r"],"items":[],
            "total":0,"matched":0,"scanned":3,"skipped":1,"errors":[]
        }"#;
        let m: Manifest = serde_json::from_str(old).expect("old manifest still deserialises");
        assert!(m.top_files.is_empty());
        assert!(m.top_folders.is_empty());
        assert_eq!(m.scanned, 3);

        // Round-trip: field mới được ghi ra và đọc lại đúng.
        let mut with_charts = m;
        with_charts.top_files = vec![HeavyEntry {
            path: r"D:\r\bin\a.dll".into(),
            size: 4096,
        }];
        with_charts.top_folders = vec![HeavyEntry {
            path: r"D:\r\proj".into(),
            size: 8192,
        }];
        let json = serde_json::to_string(&with_charts).expect("serialize");
        let back: Manifest = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, with_charts);
    }
}
