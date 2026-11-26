//! The data/actions half of `dashboard.Api` (no window access): state, save, flags, hooks, widget settings.
//! Semantics follow the Python code: `save` is `cfg.update(v)` + the interval floor + autostart, nothing else.

#[cfg(not(any(debug_assertions, feature = "e2e")))]
use cu_cleaner::PermanentDeleter;
#[cfg(any(debug_assertions, feature = "e2e"))]
use cu_cleaner::RecordingDeleter;
use cu_cleaner::{
    clean, find_duplicates, path_contains, scan, CleanRecord, DeleteOutcome, Deleter, DupsOptions,
    DupsReport, FileJournal, Manifest, Mode, RealFs, ScanOptions, DEFAULT_MAX_BYTES_HASH,
    DEFAULT_MAX_FILES, PHASE_CLEANING,
};
use cu_core::cache::load_cache;
use cu_core::config::{self, Config};
use cu_core::fmt::effective_interval;
use cu_core::hookinstall::{self, HookProfile};
use cu_core::paths::Paths;
use cu_core::pyval::{py_int, truthy};
use cu_core::MIN_INTERVAL;
use serde_json::{json, Value};
use std::fs;
use std::path::{Component, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Vòng đời của một phiên dọn: chỉ `Done` mới cho phép `cleaner_clean`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CleanPhase {
    Idle,
    Scanning,
    Done,
    Cleaning { run_id: String },
}

/// Trạng thái RAM của cleaner. `Journal` không `Send + Sync` nên được tạo bên trong thread worker,
/// không bao giờ chia sẻ qua `Arc`.
#[derive(Debug)]
pub struct CleanerState {
    pub phase: CleanPhase,
    pub manifest: Option<Manifest>,
    pub opts: Option<ScanOptions>,
    pub outcomes: Vec<DeleteOutcome>,
    pub last_run: Option<CleanRecord>,
    pub interrupted: bool,
    pub errors: Vec<String>,
    /// milliseconds — chỉ dùng để tính `elapsed_ms` cho UI.
    pub started_at: u64,
}

impl Default for CleanerState {
    fn default() -> Self {
        Self {
            phase: CleanPhase::Idle,
            manifest: None,
            opts: None,
            outcomes: Vec::new(),
            last_run: None,
            interrupted: false,
            errors: Vec::new(),
            started_at: now_millis(),
        }
    }
}

/// Vòng đời của một phiên tìm file trùng (detection-only: không có bước xoá).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DupsPhase {
    Idle,
    Scanning,
    Done,
}

impl DupsPhase {
    fn as_str(self) -> &'static str {
        match self {
            DupsPhase::Idle => "idle",
            DupsPhase::Scanning => "scanning",
            DupsPhase::Done => "done",
        }
    }
}

/// Trạng thái RAM của riêng phần tìm file trùng. Tách khỏi `CleanerState` để poll không bị block
/// bởi các khoá/journal của luồng dọn.
#[derive(Debug)]
pub struct DupsState {
    pub phase: DupsPhase,
    pub report: Option<DupsReport>,
    pub errors: Vec<String>,
    pub started_at: u64,
}

impl Default for DupsState {
    fn default() -> Self {
        Self {
            phase: DupsPhase::Idle,
            report: None,
            errors: Vec::new(),
            started_at: now_millis(),
        }
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Debug/e2e: không bao giờ chạm đĩa. Release: xoá vĩnh viễn hoặc đưa vào thùng rác.
/// `ShellDeleter` do crate cleaner (Agent F) cung cấp; bản debug không cần tên đó để biên dịch.
fn make_deleter(mode: Mode) -> Box<dyn Deleter> {
    #[cfg(any(debug_assertions, feature = "e2e"))]
    {
        let _ = mode;
        Box::new(RecordingDeleter::new())
    }
    #[cfg(not(any(debug_assertions, feature = "e2e")))]
    {
        match mode {
            Mode::Permanent => Box::new(PermanentDeleter),
            Mode::Recycle => Box::new(cu_cleaner::ShellDeleter::default()),
        }
    }
}

pub struct Core {
    pub paths: Paths,
    pub home: PathBuf,
    /// The tray exe (`claude-usage.exe` next to this one): what hooks and autostart must point at.
    pub tray_exe: PathBuf,
    /// Registry writes allowed (release build and no `CU_NO_AUTOSTART`).
    pub autostart_ok: bool,
    /// Trạng thái quét/dọn dùng chung với thread worker (journal nằm trong worker).
    pub cleaner: Arc<Mutex<CleanerState>>,
    /// Trạng thái tìm file trùng (chỉ phát hiện, không bao giờ xoá).
    pub dups: Arc<Mutex<DupsState>>,
}

impl Core {
    pub fn new(paths: Paths, home: PathBuf, tray_exe: PathBuf, autostart_ok: bool) -> Self {
        let last_run = fs::read_to_string(paths.data.join("cleaner_result.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<CleanRecord>(&t).ok());
        // Lần trước bị ngắt: báo cho UI, KHÔNG tự resume — cần sự đồng ý mới.
        let interrupted = last_run.as_ref().is_some_and(|r| r.phase == PHASE_CLEANING);
        Core {
            paths,
            home,
            tray_exe,
            autostart_ok,
            cleaner: Arc::new(Mutex::new(CleanerState {
                last_run,
                interrupted,
                ..CleanerState::default()
            })),
            dups: Arc::new(Mutex::new(DupsState::default())),
        }
    }

    pub fn cfg(&self) -> Config {
        config::load_config(&self.paths, &self.home)
    }

    fn save_cfg(&self, cfg: &Config) -> bool {
        config::save_config(&self.paths, cfg).is_ok()
    }

    /// `Api.state()`
    pub fn state(&self) -> Value {
        let cfg = self.cfg();
        let interval = effective_interval(&cfg);
        json!({"cfg": cfg, "cache": load_cache(&self.paths), "version": VERSION, "interval": interval})
    }

    /// `page()`: ui.html with the current state inlined so the numbers show the instant the window appears.
    pub fn page(&self, html: &str) -> String {
        let init = json!({"cfg": self.cfg(), "cache": load_cache(&self.paths), "version": VERSION})
            .to_string()
            .replace("</", "<\\/");
        html.replacen("/*__INIT__*/null", &init, 1)
    }

    pub fn flag(&self, path: &std::path::Path) -> Value {
        Value::Bool(fs::write(path, "1").is_ok())
    }

    fn hook_profiles(&self) -> Vec<HookProfile> {
        config::profiles(&self.cfg())
            .iter()
            .map(|p| HookProfile::new(p.name.clone(), &p.path))
            .collect()
    }

    pub fn hook_status(&self) -> Value {
        json!(hookinstall::status(&self.hook_profiles()))
    }

    pub fn hook_install(&self) -> Value {
        json!(hookinstall::install(&self.hook_profiles(), &self.tray_exe))
    }

    pub fn hook_uninstall(&self) -> Value {
        json!(hookinstall::uninstall(&self.hook_profiles()))
    }

    pub fn widget_unlock(&self) -> Value {
        let mut cfg = self.cfg();
        cfg.insert("widget_locked".into(), Value::Bool(false));
        Value::Bool(self.save_cfg(&cfg))
    }

    pub fn widget_reset(&self) -> Value {
        let mut cfg = self.cfg();
        for (k, v) in [
            ("widget_cx", Value::Null),
            ("widget_cy", Value::Null),
            ("widget_angle", json!(0.0)),
            ("widget_size", json!(36)),
            ("widget_layout", json!("auto")),
            ("widget_locked", Value::Bool(true)),
        ] {
            cfg.insert(k.into(), v);
        }
        Value::Bool(self.save_cfg(&cfg))
    }

    /// `Api.save(v)`: false on any failure (like the blanket `except` of the Python code).
    pub fn save(&self, v: &Value) -> Value {
        let Some(patch) = v.as_object() else {
            return Value::Bool(false);
        };
        let mut cfg = self.cfg();
        for (k, val) in patch {
            cfg.insert(k.clone(), val.clone());
        }
        let Some(iv) = cfg.get("interval_sec").and_then(py_int) else {
            return Value::Bool(false);
        };
        cfg.insert("interval_sec".into(), Value::from(iv.max(MIN_INTERVAL)));
        if !self.save_cfg(&cfg) {
            return Value::Bool(false);
        }
        if self.autostart_ok {
            crate::win::set_autostart(cfg.get("autostart").is_some_and(truthy), &self.tray_exe);
        }
        Value::Bool(true)
    }

    pub fn set_dash_rect(&self, r: Option<[i32; 4]>) {
        let mut cfg = self.cfg();
        cfg.insert("dash_rect".into(), r.map_or(Value::Null, |r| json!(r)));
        self.save_cfg(&cfg);
    }

    pub fn saved_rect(&self) -> Value {
        self.cfg().get("dash_rect").cloned().unwrap_or(Value::Null)
    }

    /// Cấu hình quét: roots/skip_days từ config, forbidden gồm DATA, HOME, thư mục chứa exe
    /// và các thư mục hệ thống. Không đụng đĩa ngoài việc đọc config.
    fn scan_options(&self, cfg: &Config) -> ScanOptions {
        let roots: Vec<PathBuf> = cfg
            .get("cleaner_roots")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(PathBuf::from)
                    .collect::<Vec<_>>()
            })
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| vec![PathBuf::from(r"D:\stu")]);
        let skip_recent_days = cfg
            .get("cleaner_skip_recent_days")
            .and_then(py_int)
            .unwrap_or(0)
            .max(0) as u64;
        let mut forbidden = vec![self.paths.data.clone(), self.home.clone()];
        if let Some(dir) = self.tray_exe.parent() {
            forbidden.push(dir.to_path_buf());
        }
        for sys in [
            r"C:\Windows",
            r"C:\Program Files",
            r"C:\Program Files (x86)",
            r"C:\ProgramData",
            r"C:\System Volume Information",
            r"C:\$Recycle.Bin",
        ] {
            forbidden.push(PathBuf::from(sys));
        }
        // Mỗi root nằm trên một ổ thì chặn luôn các thư mục hệ thống của CHÍNH ổ đó — cần thiết
        // khi người dùng chọn gốc ổ (D:\) làm root. Dùng Component của std (không so chuỗi naive)
        // và `path_contains` của cleaner để không thêm path đã có / nằm trong forbidden cũ.
        for root in &roots {
            let mut parts = root.components();
            let (Some(prefix), Some(root_dir)) = (parts.next(), parts.next()) else {
                continue;
            };
            if !matches!(prefix, Component::Prefix(_)) || !matches!(root_dir, Component::RootDir) {
                continue;
            }
            #[cfg(windows)]
            if !matches!(prefix, Component::Prefix(p) if matches!(p.kind(), std::path::Prefix::Disk(_))) {
                continue;
            }
            let drive: PathBuf = root.components().take(2).collect();
            for name in [
                "System Volume Information",
                "$Recycle.Bin",
                "Windows",
                "Program Files",
                "Program Files (x86)",
                "ProgramData",
                "Recovery",
                "$WinREAgent",
            ] {
                let p = drive.join(name);
                if !forbidden.iter().any(|f| path_contains(f, &p)) {
                    forbidden.push(p);
                }
            }
        }
        ScanOptions {
            roots,
            forbidden,
            skip_recent_days,
            now_secs: now_secs(),
            max_items: 5000,
            max_dirs: 200_000,
        }
    }

    /// Bắt đầu quét nền. Trả `{ok:true}` ngay; UI gọi `cleaner_status` để lấy kết quả.
    pub fn cleaner_scan(&self) -> Value {
        {
            let mut st = self.cleaner.lock().unwrap_or_else(|e| e.into_inner());
            if matches!(st.phase, CleanPhase::Scanning | CleanPhase::Cleaning { .. }) {
                return json!({"ok": false, "error": "đang chạy"});
            }
            st.phase = CleanPhase::Scanning;
            st.started_at = now_millis();
            st.errors.clear();
        }
        let opts = self.scan_options(&self.cfg());
        let cleaner = Arc::clone(&self.cleaner);
        std::thread::spawn(move || {
            let result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| scan(&opts, &RealFs)));
            let mut st = cleaner.lock().unwrap_or_else(|e| e.into_inner());
            match result {
                Ok(manifest) => {
                    st.manifest = Some(manifest);
                    st.opts = Some(opts);
                    st.phase = CleanPhase::Done;
                    st.outcomes.clear();
                    st.interrupted = false;
                }
                Err(_) => {
                    st.phase = CleanPhase::Idle;
                    st.errors.push("quét bị lỗi".into());
                }
            }
        });
        json!({"ok": true})
    }

    /// Chỉ đọc `Arc<Mutex<_>>` — KHÔNG gọi `self.cfg()` (tránh 150ms đọc đĩa mỗi lần poll).
    pub fn cleaner_status(&self) -> Value {
        let st = self.cleaner.lock().unwrap_or_else(|e| e.into_inner());
        let phase = match st.phase {
            CleanPhase::Idle => "idle",
            CleanPhase::Scanning => "scanning",
            CleanPhase::Done => "done",
            CleanPhase::Cleaning { .. } => "cleaning",
        };
        let (total, matched, scanned, items, top_files, top_folders) = match &st.manifest {
            Some(m) => (
                m.total,
                m.matched,
                m.scanned,
                m.items
                    .iter()
                    .take(12)
                    .map(|i| json!({"id": i.id, "path": i.path, "size": i.size, "kind": i.kind}))
                    .collect::<Vec<_>>(),
                m.top_files
                    .iter()
                    .map(|e| json!({"path": e.path, "size": e.size}))
                    .collect::<Vec<_>>(),
                m.top_folders
                    .iter()
                    .map(|e| json!({"path": e.path, "size": e.size}))
                    .collect::<Vec<_>>(),
            ),
            None => (0, 0, 0, Vec::new(), Vec::new(), Vec::new()),
        };
        let outcomes = st
            .outcomes
            .iter()
            .map(|o| json!({"id": o.id, "ok": o.ok, "err": o.err}))
            .collect::<Vec<_>>();
        let cleaned = st.outcomes.iter().filter(|o| o.ok).count();
        json!({
            "phase": phase,
            "items": items,
            "total": total,
            "matched": matched,
            "scanned": scanned,
            "cleaned": cleaned,
            "top_files": top_files,
            "top_folders": top_folders,
            "errors": &st.errors,
            "interrupted": st.interrupted,
            "outcomes": outcomes,
            "last_run": &st.last_run,
            "elapsed_ms": now_millis().saturating_sub(st.started_at),
        })
    }

    /// Dọn các id đã quét. Chỉ nhận khi `Done` + có manifest + không bị ngắt lần trước.
    /// `a[0]` là mảng id, `a[1]` là `"permanent"` / `"recycle"`.
    pub fn cleaner_clean(&self, ids: &[Value], mode: &str) -> Value {
        let mode = match mode {
            "permanent" => Mode::Permanent,
            "recycle" => Mode::Recycle,
            _ => return json!({"ok": false, "error": "mode không hợp lệ"}),
        };
        let ids: Vec<String> = ids
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect();
        let (manifest, opts, run_id) = {
            let st = self.cleaner.lock().unwrap_or_else(|e| e.into_inner());
            if !matches!(st.phase, CleanPhase::Done) || st.manifest.is_none() || st.opts.is_none() {
                return json!({"ok": false, "error": "chưa quét"});
            }
            if st.interrupted {
                return json!({"ok": false, "error": "lần trước bị ngắt"});
            }
            let m = st.manifest.clone().expect("manifest checked");
            let o = st.opts.clone().expect("opts checked");
            let rid = m.run_id.clone();
            (m, o, rid)
        };
        {
            let mut st = self.cleaner.lock().unwrap_or_else(|e| e.into_inner());
            st.phase = CleanPhase::Cleaning { run_id };
            st.started_at = now_millis();
            st.outcomes.clear();
        }
        let cleaner = Arc::clone(&self.cleaner);
        let journal_path = self.paths.data.join("cleaner_result.json");
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let deleter = make_deleter(mode);
                let journal = FileJournal {
                    path: journal_path.clone(),
                };
                clean(&manifest, &ids, mode, &opts, deleter.as_ref(), &journal)
            }));
            let mut st = cleaner.lock().unwrap_or_else(|e| e.into_inner());
            match result {
                Ok(outcomes) => {
                    st.outcomes = outcomes;
                    // Journal được ghi trong worker: làm mới last_run để UI không thấy kết quả cũ.
                    match fs::read_to_string(&journal_path)
                        .ok()
                        .and_then(|t| serde_json::from_str::<CleanRecord>(&t).ok())
                    {
                        Some(rec) => st.last_run = Some(rec),
                        None => st.errors.push("không đọc được journal kết quả".into()),
                    }
                }
                Err(_) => {
                    st.errors.push("dọn bị lỗi".into());
                    st.outcomes.push(DeleteOutcome {
                        id: String::new(),
                        ok: false,
                        err: Some("dọn ổ bị ngắt bất thường".into()),
                    });
                }
            }
            st.phase = CleanPhase::Done;
            // id cũ vô hiệu cho đến lần scan mới.
            st.manifest = None;
            st.opts = None;
        });
        json!({"ok": true})
    }

    /// Bắt đầu tìm file trùng nền. Detection-only: không có API xoá, không đụng vào `cleaner`.
    /// Dùng chung roots + forbidden với `scan_options` để cùng ranh giới an toàn với Quét.
    pub fn cleaner_dups_scan(&self) -> Value {
        {
            let mut st = self.dups.lock().unwrap_or_else(|e| e.into_inner());
            if st.phase == DupsPhase::Scanning {
                return json!({"ok": false, "error": "đang chạy"});
            }
            st.phase = DupsPhase::Scanning;
            st.started_at = now_millis();
            st.errors.clear();
            st.report = None;
        }
        let cfg = self.cfg();
        let so = self.scan_options(&cfg);
        // KB từ config (0 = mọi file >0 byte); clamp >= 0 rồi đổi sang byte. Chỉ đọc config ở đây,
        // cleaner_dups_status() vẫn không gọi cfg().
        let min_kb = cfg
            .get("cleaner_dups_min_kb")
            .and_then(py_int)
            .unwrap_or(0)
            .max(0) as u64;
        let opts = DupsOptions {
            roots: so.roots,
            forbidden: so.forbidden,
            min_size: min_kb.saturating_mul(1024),
            max_files: DEFAULT_MAX_FILES,
            max_bytes_hash: DEFAULT_MAX_BYTES_HASH,
            now_secs: now_secs(),
        };
        let dups = Arc::clone(&self.dups);
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                find_duplicates(&opts, &RealFs)
            }));
            let mut st = dups.lock().unwrap_or_else(|e| e.into_inner());
            match result {
                Ok(report) => {
                    st.report = Some(report);
                    st.phase = DupsPhase::Done;
                }
                Err(_) => {
                    st.phase = DupsPhase::Idle;
                    st.errors.push("tìm file trùng bị lỗi".into());
                }
            }
        });
        json!({"ok": true})
    }

    /// Chỉ đọc `Arc<Mutex<_>>` — KHÔNG gọi `self.cfg()` (mỗi lần poll phải rẻ).
    pub fn cleaner_dups_status(&self) -> Value {
        let st = self.dups.lock().unwrap_or_else(|e| e.into_inner());
        let (groups, scanned, skipped, bytes_hashed, truncated, mut errors) = match &st.report {
            Some(r) => (
                r.groups
                    .iter()
                    .take(8)
                    .map(|g| {
                        json!({
                            "hash": g.hash.chars().take(12).collect::<String>(),
                            "files": g.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
                            "size_each": g.size_each,
                            "wasted": g.wasted,
                        })
                    })
                    .collect::<Vec<_>>(),
                r.scanned,
                r.skipped,
                r.bytes_hashed,
                r.truncated,
                r.errors.clone(),
            ),
            None => (Vec::new(), 0, 0, 0, false, Vec::new()),
        };
        errors.extend(st.errors.iter().cloned());
        json!({
            "phase": st.phase.as_str(),
            "groups": groups,
            "scanned": scanned,
            "skipped": skipped,
            "bytes_hashed": bytes_hashed,
            "errors": errors,
            "truncated": truncated,
            "elapsed_ms": now_millis().saturating_sub(st.started_at),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn core(dir: &std::path::Path) -> Core {
        let home = dir.join("home");
        fs::create_dir_all(&home).unwrap();
        Core::new(
            Paths::with_root(dir.join("data")),
            home,
            dir.join("claude-usage.exe"),
            false,
        )
    }

    fn read(c: &Core) -> Value {
        serde_json::from_str(&fs::read_to_string(&c.paths.config).unwrap()).unwrap()
    }

    #[test]
    fn save_updates_merges_and_floors_interval() {
        let t = tempfile::tempdir().unwrap();
        let c = core(t.path());
        fs::write(&c.paths.config, r#"{"warn_pct": 70, "custom_key": 1}"#).unwrap();
        assert_eq!(
            c.save(&json!({"interval_sec": 10, "block_pct": 99, "new": "x"})),
            true
        );
        let f = read(&c);
        assert_eq!(f["interval_sec"], 90); // floor
        assert_eq!(f["block_pct"], 99);
        assert_eq!(f["warn_pct"], 70); // untouched keys survive
        assert_eq!(f["custom_key"], 1); // unknown keys survive
        assert_eq!(f["new"], "x"); // no whitelist, like Python
        assert_eq!(f["profiles"], json!([])); // the merged config is written whole
    }

    #[test]
    fn save_failures_return_false() {
        let t = tempfile::tempdir().unwrap();
        let c = core(t.path());
        assert_eq!(c.save(&json!("not a dict")), false);
        assert_eq!(c.save(&json!({"interval_sec": null})), false);
        assert_eq!(c.save(&json!({"interval_sec": "abc"})), false);
        assert_eq!(c.save(&json!({"interval_sec": "200"})), true); // int("200") works
        assert_eq!(read(&c)["interval_sec"], 200);
    }

    #[test]
    fn widget_actions_edit_config() {
        let t = tempfile::tempdir().unwrap();
        let c = core(t.path());
        c.save(&json!({"widget_locked": true, "widget_cx": 5, "widget_size": 50, "widget_angle": 30.0, "widget_layout": "h"}));
        c.widget_unlock();
        assert_eq!(read(&c)["widget_locked"], false);
        c.widget_reset();
        let f = read(&c);
        assert_eq!(f["widget_cx"], Value::Null);
        assert_eq!(f["widget_size"], 36);
        assert_eq!(f["widget_angle"], 0.0);
        assert_eq!(f["widget_layout"], "auto");
        assert_eq!(f["widget_locked"], true);
    }

    #[test]
    fn page_inlines_state_and_escapes_script_end() {
        let t = tempfile::tempdir().unwrap();
        let c = core(t.path());
        c.save(&json!({"aliases": {"a": "</script>"}}));
        let p = c.page("x=/*__INIT__*/null; y=/*__INIT__*/null;");
        assert!(p.starts_with("x={\"cfg\":"), "{p}");
        assert!(p.contains(&format!("\"version\":\"{VERSION}\"")), "{p}");
        assert!(!p.contains("</script>") && p.contains("<\\/script>"));
        assert!(p.ends_with("y=/*__INIT__*/null;")); // only the first marker
    }

    #[test]
    fn state_has_cache_and_interval() {
        let t = tempfile::tempdir().unwrap();
        let c = core(t.path());
        let s = c.state();
        assert_eq!(s["cache"], Value::Null);
        assert_eq!(s["interval"], 180);
        assert_eq!(s["version"], VERSION);
        fs::write(&c.paths.cache, r#"{"profiles": {}}"#).unwrap();
        assert_eq!(c.state()["cache"], json!({"profiles": {}}));
    }

    #[test]
    fn hooks_install_status_uninstall_in_sandbox() {
        let t = tempfile::tempdir().unwrap();
        let c = core(t.path());
        let prof = t.path().join("prof");
        fs::create_dir_all(&prof).unwrap();
        fs::write(prof.join("settings.json"), "{}").unwrap();
        c.save(&json!({"profiles": [{"name": "p1", "path": prof.to_string_lossy()}]}));
        assert_eq!(c.hook_status(), json!([["p1", false]]));
        assert_eq!(c.hook_install(), json!([["p1", "đã cài"]]));
        assert_eq!(c.hook_status(), json!([["p1", true]]));
        let s = fs::read_to_string(prof.join("settings.json")).unwrap();
        assert!(s.contains("claude-usage.exe") && s.contains(" hook"));
        assert_eq!(c.hook_uninstall(), json!([["p1", "đã gỡ"]]));
        assert_eq!(c.hook_status(), json!([["p1", false]]));
    }

    // Các test dưới đây dùng `keep()`: không xoá cả thư mục tạm do chính test tạo ra.
    fn sandbox() -> PathBuf {
        tempfile::tempdir().unwrap().keep()
    }

    fn wait_done(c: &Core) -> Value {
        let t = std::time::Instant::now();
        loop {
            let s = c.cleaner_status();
            if s["phase"] != "scanning" && s["phase"] != "cleaning" {
                return s;
            }
            assert!(
                t.elapsed() < std::time::Duration::from_secs(10),
                "timeout: {s}"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    fn wait_dups_done(c: &Core) -> Value {
        let t = std::time::Instant::now();
        loop {
            let s = c.cleaner_dups_status();
            if s["phase"] != "scanning" {
                return s;
            }
            assert!(
                t.elapsed() < std::time::Duration::from_secs(20),
                "timeout: {s}"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    /// Cây giả: `app/package.json` + `app/node_modules/payload.bin` → đúng rule NodeModules.
    fn fake_tree() -> (PathBuf, String, u64) {
        let root = sandbox();
        let app = root.join("app");
        let nm = app.join("node_modules");
        fs::create_dir_all(&nm).unwrap();
        fs::write(app.join("package.json"), "{}").unwrap();
        fs::write(nm.join("payload.bin"), vec![7u8; 4096]).unwrap();
        (root, nm.to_string_lossy().to_string(), 4096)
    }

    fn write_scan_cfg(c: &Core, roots: &[PathBuf]) {
        fs::write(
            &c.paths.config,
            json!({
                "profiles": [{"name": "p", "path": "C:\\none"}],
                "interval_sec": 180,
                "cleaner_roots": roots,
                "cleaner_skip_recent_days": 0
            })
            .to_string(),
        )
        .unwrap();
    }

    #[test]
    fn cleaner_scan_then_clean_uses_recording_deleter() {
        let c = core(&sandbox());
        let (root, _fake_id, size) = fake_tree();
        write_scan_cfg(&c, &[root]);

        assert_eq!(
            c.cleaner_clean(&[json!("bat-ky")], "permanent")["ok"],
            false
        ); // chưa quét
        assert_eq!(c.cleaner_scan(), json!({"ok": true}));
        let s = wait_done(&c);
        assert_eq!(s["phase"], "done");
        assert_eq!(s["matched"], 1);
        assert_eq!(s["total"], size);
        // scanner canonicalise root (có thể trả về tên 8.3) → lấy id đúng từ manifest.
        let id = s["items"][0]["id"].as_str().unwrap().to_string();
        assert_eq!(s["items"][0]["kind"], json!("NodeModules"));
        assert!(s["scanned"].as_u64().unwrap_or(0) > 0);
        assert_eq!(s["cleaned"], 0);
        assert_eq!(s["interrupted"], false);

        // id lạ: engine từ chối vì không nằm trong manifest.
        assert_eq!(
            c.cleaner_clean(&[json!("id-khong-ton-tai")], "permanent"),
            json!({"ok": true})
        );
        let s = wait_done(&c);
        assert_eq!(s["outcomes"][0]["ok"], false);
        assert_eq!(s["cleaned"], 0);
        assert_eq!(s["last_run"]["phase"], "done"); // last_run đã được làm mới
        assert!(s["outcomes"][0]["err"]
            .as_str()
            .unwrap()
            .contains("not in the manifest"));
        assert_eq!(s["items"], json!([])); // manifest cũ đã vô hiệu

        // Quét lại rồi dọn id thật: debug build dùng RecordingDeleter, không đụng đĩa.
        assert_eq!(c.cleaner_scan()["ok"], true);
        let s = wait_done(&c);
        assert_eq!(s["phase"], "done");
        assert_eq!(c.cleaner_clean(&[json!(id.clone())], "recycle")["ok"], true);
        let s = wait_done(&c);
        assert_eq!(s["outcomes"][0]["id"], json!(id));
        assert_eq!(s["outcomes"][0]["ok"], true);
        assert_eq!(s["cleaned"], 1);
        assert_eq!(s["scanned"], 0); // manifest đã bị xoá sau clean
        assert_eq!(s["last_run"]["phase"], "done");
        assert!(std::path::Path::new(&id).is_dir()); // vẫn còn nguyên — không có xoá thật
        let rec: CleanRecord = serde_json::from_str(
            &fs::read_to_string(c.paths.data.join("cleaner_result.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(rec.phase, "done");
        assert!(rec.outcomes[0].ok);
        assert_eq!(c.cleaner_clean(&[json!(id)], "khong-hop-le")["ok"], false);
    }

    #[test]
    fn cleaner_status_never_reads_config() {
        let c = core(&sandbox());
        // Config hỏng: cfg() mất ~125ms/lần (6 lần thử, 5 lần sleep 25ms).
        fs::write(&c.paths.config, "{not json").unwrap();
        let t = std::time::Instant::now();
        let mut s = c.cleaner_status();
        for _ in 0..2 {
            s = c.cleaner_status();
        }
        assert_eq!(s["scanned"], 0);
        assert_eq!(s["cleaned"], 0);
        assert!(
            t.elapsed() < std::time::Duration::from_millis(300),
            "status có vẻ đã gọi cfg(): {:?}",
            t.elapsed()
        );
    }

    #[test]
    fn cleaner_status_reports_heavy_charts_from_manifest() {
        let c = core(&sandbox());
        // Chưa quét: hai mảng mới rỗng.
        let s = c.cleaner_status();
        assert_eq!(s["top_files"], json!([]));
        assert_eq!(s["top_folders"], json!([]));

        {
            let mut st = c.cleaner.lock().unwrap_or_else(|e| e.into_inner());
            let mut m = Manifest::new();
            m.top_files = vec![
                cu_cleaner::HeavyEntry {
                    path: r"D:\app\bin\a.dll".into(),
                    size: 4096,
                },
                cu_cleaner::HeavyEntry {
                    path: r"D:\app\obj\b.dll".into(),
                    size: 2048,
                },
            ];
            m.top_folders = vec![cu_cleaner::HeavyEntry {
                path: r"D:\app".into(),
                size: 8192,
            }];
            st.manifest = Some(m);
        }
        let s = c.cleaner_status();
        assert_eq!(
            s["top_files"],
            json!([
                {"path": r"D:\app\bin\a.dll", "size": 4096},
                {"path": r"D:\app\obj\b.dll", "size": 2048}
            ])
        );
        assert_eq!(s["top_folders"], json!([{"path": r"D:\app", "size": 8192}]));
    }

    #[test]
    fn cleaner_interrupted_journal_is_reported_and_never_resumed() {
        let dir = sandbox();
        let data = dir.join("data");
        fs::create_dir_all(&data).unwrap();
        let (root, id, _) = fake_tree();
        let rec = CleanRecord {
            run_id: "run-cu".into(),
            phase: PHASE_CLEANING.into(),
            mode: Mode::Recycle,
            outcomes: Vec::new(),
            finished_at: now_secs(),
        };
        let original = serde_json::to_string(&rec).unwrap();
        fs::write(data.join("cleaner_result.json"), &original).unwrap();

        let c = core(&dir);
        let s = c.cleaner_status();
        assert_eq!(s["interrupted"], true);
        assert_eq!(s["last_run"]["phase"], "cleaning");
        write_scan_cfg(&c, &[root]);
        assert_eq!(c.cleaner_scan()["ok"], true);
        let s = wait_done(&c);
        assert_eq!(s["phase"], "done");
        // Quét mới = đồng ý mới → cờ ngắt được xoá trong RAM.
        assert_eq!(s["interrupted"], false);
        // Giả lập cờ ngắt còn tồn tại: clean phải bị chặn, không tự resume.
        c.cleaner.lock().unwrap().interrupted = true;
        assert_eq!(
            c.cleaner_clean(&[json!(id)], "permanent"),
            json!({"ok": false, "error": "lần trước bị ngắt"})
        );
        assert_eq!(
            fs::read_to_string(data.join("cleaner_result.json")).unwrap(),
            original
        ); // không có lần dọn nào chạy → journal không đổi
    }

    #[test]
    fn scan_options_adds_forbidden_for_every_root_drive() {
        let c = core(&sandbox());
        let cfg: Config = serde_json::from_value(json!({
            "cleaner_roots": [r"D:\", r"C:\work"],
            "cleaner_skip_recent_days": 2
        }))
        .unwrap();
        let so = c.scan_options(&cfg);
        assert_eq!(so.skip_recent_days, 2);
        #[cfg(windows)]
        {
            for name in [
                "System Volume Information",
                "$Recycle.Bin",
                "Windows",
                "Program Files",
                "Program Files (x86)",
                "ProgramData",
                "Recovery",
                "$WinREAgent",
            ] {
                for drive in [r"D:\", r"C:\"] {
                    let p = PathBuf::from(drive).join(name);
                    assert!(
                        so.forbidden.iter().any(|f| path_contains(f, &p)),
                        "thiếu forbidden {name} cho {drive}"
                    );
                }
            }
            // C:\Windows đã có trong danh sách cố định: không được nhân đôi.
            let win = PathBuf::from(r"C:\Windows");
            assert_eq!(
                so.forbidden.iter().filter(|f| path_contains(f, &win)).count(),
                1
            );
        }
    }

    #[test]
    fn cleaner_dups_scan_is_detection_only() {
        let dir = sandbox();
        let tree = sandbox(); // cây giả phải nằm NGOÀI exe dir của Core (exe dir là forbidden)
        let root = tree.join("faketree");
        let d = root.join("dups");
        fs::create_dir_all(&d).unwrap();
        let a = d.join("a.dat");
        let b = d.join("b.dat");
        let bytes = vec![b'x'; 1 << 20];
        fs::write(&a, &bytes).unwrap();
        fs::write(&b, &bytes).unwrap();
        let c = core(&dir);
        write_scan_cfg(&c, &[root]);

        assert_eq!(c.cleaner_dups_scan()["ok"], true);
        let s = wait_dups_done(&c);
        assert_eq!(s["phase"], "done");
        assert!(s["groups"].as_array().is_some_and(|g| !g.is_empty()));
        let g = &s["groups"][0];
        assert!(g["wasted"].as_u64().unwrap_or(0) > 0);
        assert_eq!(g["size_each"], 1 << 20);
        let paths: Vec<&str> = g["files"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert!(paths.len() >= 2, "{paths:?}");
        assert!(paths.iter().all(|p| p.ends_with("a.dat") || p.ends_with("b.dat")), "{paths:?}");
        assert_eq!(s["errors"], json!([]));
        // Không có API xoá cho duplicate: file vẫn nguyên trên đĩa.
        assert!(a.exists() && b.exists());
    }

    #[test]
    fn cleaner_dups_status_shape_status_never_reads_config() {
        let c = core(&sandbox());
        fs::write(&c.paths.config, "{not json").unwrap();
        let t = std::time::Instant::now();
        let empty = c.cleaner_dups_status();
        assert_eq!(empty["phase"], "idle");
        assert_eq!(empty["groups"], json!([]));
        assert!(t.elapsed() < std::time::Duration::from_millis(300));

        let mut report = DupsReport::default();
        for i in 0..10u32 {
            report.groups.push(cu_cleaner::DupGroup {
                hash: format!("{i:064x}"),
                files: vec![cu_cleaner::DupFile {
                    path: format!("D:\\f{i}\\a.dat"),
                    identity: None,
                }],
                size_each: 2_097_152,
                wasted: 2_097_152 * (i as u64 + 1),
            });
        }
        report.scanned = 11;
        report.skipped = 3;
        report.bytes_hashed = 42;
        report.truncated = true;
        {
            let mut st = c.dups.lock().unwrap_or_else(|e| e.into_inner());
            st.report = Some(report);
            st.phase = DupsPhase::Done;
        }
        let s = c.cleaner_dups_status();
        assert_eq!(s["phase"], "done");
        assert_eq!(s["groups"].as_array().unwrap().len(), 8); // tối đa 8 group
        assert_eq!(s["groups"][0]["hash"].as_str().unwrap().len(), 12);
        assert_eq!(s["groups"][0]["files"][0], json!("D:\\f0\\a.dat"));
        assert!(s["groups"][0].get("identity").is_none()); // bỏ identity khi serialize
        assert_eq!(s["scanned"], 11);
        assert_eq!(s["skipped"], 3);
        assert_eq!(s["bytes_hashed"], 42);
        assert_eq!(s["truncated"], true);
        assert!(s["elapsed_ms"].is_number());
    }
}
