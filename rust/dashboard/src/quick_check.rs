//! Settings Quick Check: native view authorization, bounded config evidence, and fixed fixture scoring.
use crate::quick_check_transport as transport;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    fs::{File, OpenOptions},
    io::Read,
    path::{Component, Path, PathBuf, Prefix},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant, SystemTime},
};
use transport::{Operation, Output};

pub const PREFIX: &str = "sig4a-qc:";
const MAX_ENVELOPE: usize = 256;
const MAX_JS_ID: u64 = (1 << 53) - 1;
const MAX_CONFIG: u64 = 128 * 1024;
const MAX_PREVIEW: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Enter,
    Exit,
    Work(Operation),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    id: u64,
    m: String,
    a: Vec<String>,
}
pub struct Call {
    pub id: u64,
    pub document: String,
    pub scope: Option<String>,
    pub action: Action,
}

fn token(text: &str) -> bool {
    text.len() == 32
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub fn parse(raw: &str) -> Option<Call> {
    let raw = raw.strip_prefix(PREFIX)?;
    if raw.len() > MAX_ENVELOPE {
        return None;
    }
    let wire: Envelope = serde_json::from_str(raw).ok()?;
    if wire.id == 0 || wire.id > MAX_JS_ID {
        return None;
    }
    let action = match wire.m.as_str() {
        "qc_enter" => Action::Enter,
        "qc_exit" => Action::Exit,
        "qc_refresh" => Action::Work(Operation::Refresh),
        "qc_translation" => Action::Work(Operation::Translation),
        "qc_grammar" => Action::Work(Operation::Grammar),
        _ => return None,
    };
    let (document, scope) = match (action, wire.a.as_slice()) {
        (Action::Enter, [document]) if token(document) => (document, None),
        (Action::Exit | Action::Work(_), [document, scope]) if token(document) && token(scope) => {
            (document, Some(scope.clone()))
        }
        _ => return None,
    };
    Some(Call {
        id: wire.id,
        document: document.clone(),
        scope,
        action,
    })
}

/// Call from the real native navigation callbacks, before queuing a Tao event.
#[derive(Clone, Default)]
pub struct Gate(Arc<(AtomicU64, Mutex<Option<Arc<AtomicBool>>>)>);

impl Gate {
    pub fn epoch(&self) -> u64 {
        self.0 .0.load(Ordering::Acquire)
    }
    pub fn invalidate(&self) {
        let mut current = self
            .0
             .1
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(active) = current.take() {
            active.store(false, Ordering::Release);
        }
        self.0 .0.fetch_add(1, Ordering::AcqRel);
    }
    fn arm(&self, epoch: u64) -> Arc<AtomicBool> {
        let mut current = self
            .0
             .1
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let active = Arc::new(AtomicBool::new(self.epoch() == epoch));
        if active.load(Ordering::Relaxed) {
            if let Some(old) = current.replace(active.clone()) {
                old.store(false, Ordering::Release);
            }
        }
        active
    }
}

#[derive(Default)]
pub struct View {
    scope: Option<(u64, String, String)>, // native epoch, document witness, opaque issued scope
    next: u64,
    contract: Option<(u64, Arc<AtomicBool>)>,
}

impl View {
    pub fn invalidate(&mut self) {
        self.cancel_work();
        self.scope = None;
    }
    pub fn cancel_work(&mut self) {
        if let Some((_, active)) = self.contract.take() {
            active.store(false, Ordering::Release);
        }
    }
    pub fn enter(&mut self, epoch: u64, document: String) -> Option<String> {
        self.invalidate();
        self.next = self.next.checked_add(1)?;
        let scope = format!("{epoch:016x}{:016x}", self.next);
        self.scope = Some((epoch, document, scope.clone()));
        Some(scope)
    }
    pub fn matches(&self, epoch: u64, document: &str, scope: Option<&str>) -> bool {
        self.scope.as_ref().is_some_and(|(expected, doc, token)| {
            *expected == epoch && doc == document && Some(token.as_str()) == scope
        })
    }
    pub fn admit(&mut self, id: u64, gate: &Gate, epoch: u64) -> Arc<AtomicBool> {
        self.cancel_work();
        let active = gate.arm(epoch);
        self.contract = Some((id, active.clone()));
        active
    }
    pub fn finish(&mut self, id: u64, epoch: u64, document: &str, scope: Option<&str>) -> bool {
        if !self.matches(epoch, document, scope)
            || !self
                .contract
                .as_ref()
                .is_some_and(|(current, token)| *current == id && token.load(Ordering::Acquire))
        {
            return false;
        }
        self.cancel_work();
        true
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Selection {
    eligible: bool,
    model: String,
    modified: Option<SystemTime>,
    size: u64,
}

/// Check the lexical drive before asking Windows to open anything, then pin
/// each local non-reparse ancestor before walking below it. There is no UNC,
/// mapped drive, device path, junction-following or symlink canonicalization.
fn safe_config_open(path: &Path) -> Result<(File, Vec<File>), &'static str> {
    use std::os::windows::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt},
    };
    use windows_sys::Win32::Storage::FileSystem::{
        GetDriveTypeW, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
    };
    let error = "config_unavailable";
    let mut parts = path.components();
    let Some(Component::Prefix(prefix)) = parts.next() else {
        return Err(error);
    };
    if !matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_))
        || !matches!(parts.next(), Some(Component::RootDir))
    {
        return Err(error);
    }
    let mut cursor = PathBuf::from(prefix.as_os_str());
    cursor.push(std::path::MAIN_SEPARATOR_STR);
    let root_w: Vec<_> = cursor
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    if unsafe { GetDriveTypeW(root_w.as_ptr()) } != 3 {
        return Err(error);
    } // DRIVE_FIXED
    let mut parts = parts.peekable();
    let mut ancestors = Vec::new();
    while let Some(Component::Normal(name)) = parts.next() {
        // Reject ADS/device-like names even on a normal component.
        if name.encode_wide().any(|c| c == b':' as u16) {
            return Err(error);
        }
        cursor.push(name);
        if parts.peek().is_none() {
            let handle = OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
                .open(&cursor)
                .map_err(|_| error)?;
            return Ok((handle, ancestors));
        }
        let pin = OpenOptions::new()
            .access_mode(0)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&cursor)
            .map_err(|_| error)?;
        let meta = pin.metadata().map_err(|_| error)?;
        if !meta.is_dir() || meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(error);
        }
        ancestors.push(pin);
    }
    Err(error)
}

fn saved_selection(path: &Path) -> Result<Selection, &'static str> {
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
    let (handle, _pins) = safe_config_open(path)?;
    let meta = handle.metadata().map_err(|_| "config_unavailable")?;
    if !meta.is_file()
        || meta.len() > MAX_CONFIG
        || meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    {
        return Err("config_unavailable");
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    handle
        .take(MAX_CONFIG + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "config_unavailable")?;
    if bytes.len() as u64 != meta.len() {
        return Err("config_unavailable");
    }
    let config: Value = serde_json::from_slice(&bytes).map_err(|_| "config_unavailable")?;
    if !config.is_object() {
        return Err("config_unavailable");
    }
    let provider = config
        .get("local_ai_backend")
        .map_or(Some("ollama"), Value::as_str);
    let endpoint = config
        .get("local_ai_endpoint")
        .map_or(Some("http://127.0.0.1:11434"), Value::as_str);
    let manual = config
        .get("local_ai_managed")
        .map_or(Some(false), Value::as_bool)
        == Some(false);
    let eligible = manual
        && provider == Some("ollama")
        && endpoint.is_some_and(|s| {
            matches!(
                s.trim(),
                "http://127.0.0.1:11434" | "http://localhost:11434"
            )
        });
    let model = config["local_ai_model"]
        .as_str()
        .filter(|s| s.len() <= 128 && !s.chars().any(|c| c.is_control() || c.is_whitespace()))
        .unwrap_or_default();
    let modified = Some(meta.modified().map_err(|_| "config_unavailable")?);
    Ok(Selection {
        eligible,
        model: model.into(),
        modified,
        size: meta.len(),
    })
}

fn safe_preview(text: &str) -> Option<&str> {
    let text = text.trim();
    if text.is_empty()
        || text.encode_utf16().take(MAX_PREVIEW + 1).count() > MAX_PREVIEW
        || text
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        None
    } else {
        Some(text)
    }
}

fn matches_fixture(operation: Operation, preview: &str) -> bool {
    let normalized = preview.trim().trim_end_matches('.').trim();
    // Strip a single optional final full stop only. Reject multiline commentary.
    if preview.ends_with("..") || normalized.contains(['\r', '\n']) {
        return false;
    }
    let candidate = normalized
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    match operation {
        Operation::Translation => matches!(
            candidate.as_str(),
            "robot có hai chiếc hộp màu xanh dương"
                | "con rô-bốt có 2 cái hộp màu xanh lam"
                | "con robot có hai chiếc hộp màu xanh dương"
                | "con robot có hai cái hộp màu xanh dương"
        ),
        Operation::Grammar => matches!(
            candidate.as_str(),
            "every morning the robot carries two blue boxes"
                | "every morning, the robot carries two blue boxes"
        ),
        Operation::Refresh => false,
    }
}

pub fn execute(operation: Operation, config: &Path, active: &AtomicBool) -> Value {
    let start = Instant::now();
    let deadline = start
        + if operation == Operation::Refresh {
            Duration::from_secs(5)
        } else {
            Duration::from_secs(30)
        };
    let elapsed = || start.elapsed().as_millis().min(u128::from(u32::MAX)) as u32;
    let allowed = || active.load(Ordering::Acquire);
    if !allowed() {
        return Value::Null;
    }
    let before = match saved_selection(config) {
        Ok(current) => current,
        Err(code) => return json!({"code":code,"ms":elapsed()}),
    };
    let verify_config = || {
        if !allowed() {
            return Err("cancelled");
        }
        if Instant::now() >= deadline {
            return Err("timeout");
        }
        if saved_selection(config).ok().as_ref() != Some(&before) {
            return Err("configuration_changed");
        }
        if Instant::now() >= deadline {
            return Err("timeout");
        }
        Ok(())
    };
    let result = transport::run_checked(
        operation,
        &before.model,
        before.eligible,
        active,
        deadline,
        &verify_config,
    );
    if !allowed() {
        return Value::Null;
    }
    if let Err(code) = verify_config() {
        return json!({"code":code,"ms":elapsed()});
    }
    match result {
        Ok(Output::Inventory { listed }) => {
            json!({"code":"inventory","listed":listed,"ms":elapsed()})
        }
        Ok(Output::Sample(text)) => {
            let Some(preview) = safe_preview(&text) else {
                return json!({"code":"invalid_response","ms":elapsed()});
            };
            json!({"code":if matches_fixture(operation, preview) {"match"} else {"mismatch"},"preview":preview,"ms":elapsed()})
        }
        Err(code) => json!({"code":code,"ms":elapsed()}),
    }
}

/// Debug --e2e UI rendering proof only. No HTTP, device, configuration or provider trust.
#[cfg(debug_assertions)]
pub(crate) fn scripted_fixture(operation: Operation, active: &AtomicBool) -> Value {
    std::thread::sleep(std::time::Duration::from_millis(550));
    if !active.load(Ordering::Acquire) {
        return Value::Null;
    }
    match operation {
        Operation::Refresh => json!({"code":"inventory","listed":true,"helper":false,"ms":550}),
        Operation::Translation => json!({
            "code":"match","preview":"Robot có hai chiếc hộp màu xanh dương.","ms":550
        }),
        Operation::Grammar => json!({
            "code":"mismatch","preview":"<b>Every morning the robot carry two blue boxes.</b>","ms":550
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const TOKEN: &str = "321f0000000000000000000000000000";
    #[test]
    fn native_schema_rejects_oversize_types_and_extra_inputs() {
        let valid =
            format!(r#"sig4a-qc:{{"id":4,"m":"qc_translation","a":["{TOKEN}","{TOKEN}"]}}"#);
        let call = parse(&valid).expect("valid scope status");
        assert_eq!(call.id, 4);
        assert_eq!(call.document, TOKEN);
        assert_eq!(call.scope.as_deref(), Some(TOKEN));
        assert_eq!(call.action, Action::Work(Operation::Translation));
        for bad in [
            valid.replacen(PREFIX, "", 1),
            valid.replace("4,", "0,"),
            valid.replace("4,", "\"4\","),
            valid.replace("4,", "4,\"id\":4,"),
            valid.replace("4,", "4,\"text\":\"external\","),
            valid.replace(
                &format!("\"{TOKEN}\""),
                &format!("\"{TOKEN}\",\"provided prompt\""),
            ),
            valid.replace(&format!("\"{TOKEN}\""), "{}"),
            valid.replace(TOKEN, &"a".repeat(300)),
            valid.replace("qc_translation", "qc_terminal"),
        ] {
            assert!(parse(&bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn navigation_cancels_worker_and_authorizes_no_old_result() {
        let mut view = View::default();
        let gate = Gate::default();
        let original = gate.epoch();
        let scope = view.enter(original, TOKEN.into()).unwrap();
        let active = view.admit(2, &gate, original);
        assert!(!view.finish(3, original, TOKEN, Some(&scope)));
        // Synchronous callback invalidates even before the event loop handles navigation.
        gate.invalidate();
        assert!(!active.load(Ordering::Acquire));
        assert!(!view.finish(2, original, TOKEN, Some(&scope)));
        let current = gate.epoch();
        let new_scope = view.enter(current, "fresh".into()).unwrap();
        view.admit(2, &gate, current);
        assert!(!view.finish(2, original, TOKEN, Some(&scope)));
        assert!(!view.finish(2, current, TOKEN, Some(&new_scope)));
        assert!(view.finish(2, current, "fresh", Some(&new_scope)));
        assert!(!view.finish(2, current, "fresh", Some(&new_scope)));
    }

    #[test]
    fn lexical_unc_device_and_relative_config_paths_refuse_before_opening() {
        for path in [
            r"\\sig4a.invalid\profiles\config.json",
            r"\\?\UNC\sig4a.invalid\profiles\config.json",
            r"\\.\NUL",
            r"relative\config.json",
            r"C:config.json",
        ] {
            assert!(matches!(
                safe_config_open(Path::new(path)),
                Err("config_unavailable")
            ));
        }
    }

    #[test]
    fn fixture_checks_never_score_wrong_meaning_or_bad_preview() {
        assert!(matches_fixture(
            Operation::Grammar,
            "Every morning, the robot carries two blue boxes."
        ));
        assert!(matches_fixture(
            Operation::Translation,
            "Robot có hai chiếc hộp màu xanh dương."
        ));
        assert!(!matches_fixture(
            Operation::Grammar,
            "Every morning the robot carry two blue boxes."
        ));
        assert!(!matches_fixture(
            Operation::Translation,
            "The robot has two blue boxes."
        ));
        assert!(!matches_fixture(
            Operation::Translation,
            "Robot có hai chiếc hộp màu đỏ."
        ));
        assert!(!matches_fixture(
            Operation::Translation,
            "Robot có hai chiếc hộp màu xanh dương.\nGiải thích"
        ));
        assert!(!matches_fixture(
            Operation::Translation,
            "Robot có hai chiếc hộp màu xanh dương..."
        ));
        assert!(safe_preview(&"🐕".repeat(257)).is_none());
    }
}
