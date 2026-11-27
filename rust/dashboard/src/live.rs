//! Supervised, local-only helper for the optional Windows Live Translate experiment.
use serde_json::{json, Value};
use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::os::windows::{io::{AsRawHandle, FromRawHandle, OwnedHandle}, process::CommandExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}, mpsc::{self, Receiver, SyncSender, TryRecvError}};
use std::time::{Duration, Instant};
use windows_sys::Win32::{
    Foundation::{GetLastError, ERROR_ALREADY_EXISTS, INVALID_HANDLE_VALUE},
    System::{Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Thread32First, Thread32Next, THREADENTRY32, TH32CS_SNAPTHREAD},
        JobObjects::{AssignProcessToJobObject, CreateJobObjectW, SetInformationJobObject, JobObjectExtendedLimitInformation, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE},
        Threading::{CreateMutexW, OpenThread, ResumeThread, CREATE_NO_WINDOW, CREATE_SUSPENDED, THREAD_SUSPEND_RESUME}},
};

const LINE_MAX: usize = 16_384;
const SESSION_MAX: u64 = (1 << 53) - 1;
const STOP_TIMEOUT: Duration = Duration::from_secs(2);
const START_TIMEOUT: Duration = Duration::from_secs(35);
const LIST_TIMEOUT: Duration = Duration::from_secs(6);
const QUEUE: usize = 8;

#[derive(Clone, PartialEq, Eq)]
struct Device {
    id: String,
    name: String,
    source: String,
    is_default: bool,
}
impl Device {
    fn value(&self) -> Value {
        json!({"id":self.id,"name":self.name,"source":self.source,"is_default":self.is_default})
    }
}

enum Message {
    Devices(Vec<Device>),
    Status(u64, bool),
    Caption(u64, String, Option<String>, Option<u64>),
    Error(u64, &'static str),
}
fn bounded(value: &Value, max: usize) -> Option<&str> {
    let s = value.as_str()?;
    (!s.is_empty() && s.chars().take(max + 1).count() <= max && !s.chars().any(|c| c.is_control() && c != '\n' && c != '\t')).then_some(s)
}
fn source(value: &Value) -> Option<&str> {
    value.as_str().filter(|s| matches!(*s, "playback" | "mic"))
}
fn keys(value: &Value, required: &[&str], optional: &[&str]) -> bool {
    value.as_object().is_some_and(|o| {
        required.iter().all(|k| o.contains_key(*k))
            && o.keys().all(|k| required.contains(&k.as_str()) || optional.contains(&k.as_str()))
    })
}
fn session(value: &Value, zero: bool) -> Option<u64> {
    value.as_u64().filter(|n| (*n > 0 || zero) && *n <= SESSION_MAX)
}
fn safe_code(value: &Value) -> Option<&'static str> {
    const CODES: &[&str] = &["backend_missing","model_missing","translation_unavailable","device_missing","device_lost","permission_denied","busy","timeout","protocol_error","internal"];
    value.as_str().and_then(|s| CODES.iter().find(|c| **c == s).copied())
}
fn parse(bytes: &[u8]) -> Option<Message> {
    let v: Value = serde_json::from_slice(bytes).ok()?;
    if v["v"].as_u64()? != 1 { return None; }
    match v["type"].as_str()? {
        "devices" if keys(&v, &["v","type","devices"], &[]) => {
            let all = v["devices"].as_array()?;
            if all.len() > 32 { return None; }
            let mut out: Vec<Device> = Vec::with_capacity(all.len());
            for d in all {
                if !keys(d, &["id","name","source","is_default"], &[]) { return None; }
                let id = bounded(&d["id"], 512)?;
                let name = bounded(&d["name"], 128)?;
                let source = source(&d["source"])?;
                if out.iter().any(|old| old.id == id && old.source == source) { return None; }
                out.push(Device {id:id.into(),name:name.into(),source:source.into(),is_default:d["is_default"].as_bool()?});
            }
            Some(Message::Devices(out))
        }
        "status" if keys(&v, &["v","type","session","state"], &[]) =>
            Some(Message::Status(session(&v["session"], false)?, match v["state"].as_str()? {
                "started" => true, "stopped" => false, _ => return None,
            })),
        "caption" if keys(&v, &["v","type","session","en","vi"], &["seq"]) => {
            let n = session(&v["session"], false)?;
            let en = bounded(&v["en"], 2048)?.into();
            let vi = if v["vi"].is_null() { None } else { Some(bounded(&v["vi"], 2048)?.into()) };
            let seq = if let Some(seq) = v.get("seq") { Some(session(seq, false)?) } else { None };
            Some(Message::Caption(n, en, vi, seq))
        }
        "error" if keys(&v, &["v","type","session","code"], &[]) =>
            Some(Message::Error(session(&v["session"], true)?, safe_code(&v["code"])?)),
        _ => None,
    }
}

// Never accumulate beyond LINE_MAX, even if a child sends a line without a terminator.
fn read_line(reader: &mut impl BufRead) -> io::Result<Option<Vec<u8>>> {
    let mut line = Vec::new();
    loop {
        let buf = reader.fill_buf()?;
        if buf.is_empty() {
            return if line.is_empty() { Ok(None) } else { Err(io::ErrorKind::InvalidData.into()) };
        }
        let newline = buf.iter().position(|b| *b == b'\n');
        let used = newline.unwrap_or(buf.len());
        if line.len() + used > LINE_MAX { return Err(io::ErrorKind::InvalidData.into()); }
        line.extend_from_slice(&buf[..used]);
        reader.consume(used + usize::from(newline.is_some()));
        if newline.is_some() { return Ok(Some(line)); }
    }
}

fn colocated(current_exe: &Path) -> Option<PathBuf> {
    let root = current_exe.canonicalize().ok()?;
    let dir = root.parent()?;
    let folder = dir.join("live-translate");
    let path = folder.join("Sig4a.LiveTranslate.exe");
    if folder.canonicalize().ok()? != folder || path.canonicalize().ok()? != path || !fs::metadata(&path).ok()?.is_file() {
        return None;
    }
    Some(path)
}

fn job() -> io::Result<OwnedHandle> {
    unsafe {
        let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if h.is_null() { return Err(io::Error::last_os_error()); }
        let owner = OwnedHandle::from_raw_handle(h as _);
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if SetInformationJobObject(h, JobObjectExtendedLimitInformation, (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(), std::mem::size_of_val(&info) as u32) == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(owner)
    }
}

// Spawn is suspended. A failure at any step leaves the helper unable to execute any
// instructions; it is killed/reaped before returning an actionable setup error.
fn assign_and_resume(child: &Child, job: &OwnedHandle) -> io::Result<()> {
    unsafe {
        if AssignProcessToJobObject(job.as_raw_handle() as _, child.as_raw_handle() as _) == 0 {
            return Err(io::Error::last_os_error());
        }
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snap == INVALID_HANDLE_VALUE { return Err(io::Error::last_os_error()); }
        let _snap_owner = OwnedHandle::from_raw_handle(snap as _);
        let mut entry: THREADENTRY32 = std::mem::zeroed();
        entry.dwSize = std::mem::size_of_val(&entry) as u32;
        let mut next = Thread32First(snap, &mut entry);
        let mut thread = None;
        while next != 0 {
            if entry.th32OwnerProcessID == child.id() {
                if thread.is_some() { return Err(io::ErrorKind::InvalidData.into()); }
                thread = Some(entry.th32ThreadID);
            }
            entry.dwSize = std::mem::size_of_val(&entry) as u32;
            next = Thread32Next(snap, &mut entry);
        }
        let h = OpenThread(THREAD_SUSPEND_RESUME, 0, thread.ok_or(io::ErrorKind::NotFound)?);
        if h.is_null() { return Err(io::Error::last_os_error()); }
        let _thread_owner = OwnedHandle::from_raw_handle(h as _);
        if ResumeThread(h) != 1 { return Err(io::ErrorKind::Other.into()); }
        Ok(())
    }
}

struct Helper {
    // A local-session singleton also covers two dashboards raced before their HWNDs exist.
    _singleton: OwnedHandle,
    job: Option<OwnedHandle>,
    child: Child,
    send: SyncSender<Value>,
    receive: Receiver<Message>,
    broken: Arc<AtomicBool>,
}
impl Helper {
    fn open() -> Result<Self, &'static str> {
        let exe = colocated(&std::env::current_exe().map_err(|_| "backend_missing")?).ok_or("backend_missing")?;
        let singleton = unsafe {
            let name: Vec<u16> = "Local\\SIG4A.LiveTranslate.Helper.v1".encode_utf16().chain([0]).collect();
            let h = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
            let existed = GetLastError() == ERROR_ALREADY_EXISTS;
            if h.is_null() { return Err("internal"); }
            let owner = OwnedHandle::from_raw_handle(h as _);
            if existed { return Err("busy"); }
            owner
        };
        let job = job().map_err(|_| "internal")?;
        let mut child = Command::new(&exe)
            .current_dir(exe.parent().ok_or("backend_missing")?)
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED)
            .spawn().map_err(|_| "backend_missing")?;
        if assign_and_resume(&child, &job).is_err() {
            let _ = child.kill();
            let _ = child.wait();
            return Err("internal");
        }
        let mut stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let (send, commands) = mpsc::sync_channel::<Value>(QUEUE);
        let (events, receive) = mpsc::sync_channel(QUEUE);
        let broken = Arc::new(AtomicBool::new(false));
        let write_failed = broken.clone();
        let read_failed = broken.clone();
        std::thread::spawn(move || {
            while let Ok(value) = commands.recv() {
                let data = value.to_string();
                if data.len() > LINE_MAX || stdin.write_all(data.as_bytes()).and_then(|_| stdin.write_all(b"\n")).and_then(|_| stdin.flush()).is_err() {
                    write_failed.store(true, Ordering::Release);
                    return;
                }
            }
        });
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            while let Ok(Some(line)) = read_line(&mut reader) {
                let Some(message) = parse(&line) else { break; };
                // Fail on a flooding helper instead of accumulating unbounded events.
                if events.try_send(message).is_err() { break; }
            }
            read_failed.store(true, Ordering::Release);
        });
        Ok(Self {_singleton:singleton, job:Some(job), child, send, receive, broken})
    }

    fn shutdown(&mut self, grace: Duration) {
        if self.job.is_none() { return; }
        if !grace.is_zero() {
            let _ = self.send.try_send(json!({"v":1,"type":"quit"}));
            let end = Instant::now() + grace;
            while Instant::now() < end {
                if matches!(self.child.try_wait(), Ok(Some(_))) { break; }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        // Closing the sole non-inheritable job handle kills the whole process tree.
        drop(self.job.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Drop for Helper {
    fn drop(&mut self) { self.shutdown(Duration::ZERO); }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase { Idle, Starting, Listening, Stopping, Paused }
impl Phase {
    fn label(self) -> &'static str {
        match self { Self::Idle=>"idle",Self::Starting=>"starting",Self::Listening=>"listening",Self::Stopping=>"stopping",Self::Paused=>"paused" }
    }
    fn busy(self) -> bool { matches!(self, Self::Starting | Self::Listening | Self::Stopping) }
}

#[derive(Default)]
struct Caption {
    en: String,
    vi: Option<String>,
    seq: Option<u64>,
}
pub struct Live {
    helper: Option<Helper>,
    devices: Vec<Device>,
    listing: Option<Instant>,
    phase: Phase,
    pause: bool,
    deadline: Option<Instant>,
    generation: u64,
    translate: bool,
    caption: Caption,
    code: Option<&'static str>,
}

impl Default for Live {
    fn default() -> Self {
        Self {helper:None,devices:Vec::new(),listing:None,phase:Phase::Idle,pause:false,deadline:None,
            generation:0,translate:false,caption:Caption::default(),code:None}
    }
}

impl Live {
    pub fn active_worker(&self) -> bool { self.helper.is_some() }

    fn send(&mut self, message: Value, launch: bool) {
        if self.helper.is_none() {
            if !launch { self.fail("protocol_error"); return; }
            match Helper::open() {
                Ok(helper) => self.helper = Some(helper),
                Err(code) => { self.fail(code); return; }
            }
        }
        if self.helper.as_ref().unwrap().send.try_send(message).is_err() { self.fail("protocol_error"); }
    }

    fn fail(&mut self, code: &'static str) {
        if let Some(mut helper) = self.helper.take() { helper.shutdown(Duration::ZERO); }
        self.listing = None;
        self.phase = Phase::Idle;
        self.deadline = None;
        self.caption = Caption::default();
        self.code = Some(code);
    }

    pub fn close(&mut self) {
        if let Some(mut helper) = self.helper.take() { helper.shutdown(Duration::from_millis(300)); }
        self.caption = Caption::default();
    }

    fn refresh(&mut self) {
        if self.phase.busy() || self.listing.is_some() { return; }
        self.caption = Caption::default();
        self.code = None;
        self.listing = Some(Instant::now() + LIST_TIMEOUT);
        self.send(json!({"v":1,"type":"list"}), true);
    }

    fn start(&mut self, value: &Value) {
        if self.phase.busy() || self.listing.is_some() { return; }
        self.caption = Caption::default();
        self.code = None;
        if value["translate"].as_bool() == Some(true) && value["consent"].as_bool() != Some(true) {
            self.code = Some("consent_required");
            return;
        }
        let Some((source, device_id, translate)) = self.start_args(value) else {
            self.code = Some("device_missing");
            return;
        };
        if self.generation >= SESSION_MAX { self.fail("protocol_error"); return; }
        self.generation += 1;
        self.translate = translate;
        self.phase = Phase::Starting;
        self.deadline = Some(Instant::now() + START_TIMEOUT);
        self.send(json!({"v":1,"type":"start","session":self.generation,"source":source,"device_id":device_id,"translate":translate}), true);
    }

    fn start_args(&self, value: &Value) -> Option<(String, Option<String>, bool)> {
        if !keys(value, &["source","device_id","translate","consent"], &[]) { return None; }
        let source = source(&value["source"])?;
        let translate = value["translate"].as_bool()?;
        if translate && value["consent"].as_bool() != Some(true) { return None; }
        let device = if value["device_id"].is_null() {
            if source != "playback" { return None; }
            None
        } else {
            let id = bounded(&value["device_id"], 512)?;
            if !self.devices.iter().any(|d| d.id == id && d.source == source) { return None; }
            Some(id.to_owned())
        };
        Some((source.to_owned(), device, translate))
    }

    pub fn stop(&mut self, pause: bool) {
        self.caption = Caption::default();
        match self.phase {
            Phase::Starting | Phase::Listening => {
                self.phase = Phase::Stopping;
                self.pause = pause;
                self.deadline = Some(Instant::now() + STOP_TIMEOUT);
                self.send(json!({"v":1,"type":"stop","session":self.generation}), false);
            }
            Phase::Stopping => {
                if !pause { self.pause = false; }
            }
            Phase::Paused if !pause => self.phase = Phase::Idle,
            _ => {}
        }
    }

    fn event(&mut self, msg: Message) -> Result<(), &'static str> {
        match msg {
            Message::Devices(devices) if self.listing.take().is_some() => self.devices = devices,
            Message::Devices(_) => return Err("protocol_error"),
            Message::Status(n, started) if n == self.generation => {
                match (self.phase, started) {
                    (Phase::Starting, true) => { self.phase = Phase::Listening; self.deadline = None; }
                    (Phase::Stopping, false) => { self.phase = if self.pause {Phase::Paused} else {Phase::Idle}; self.deadline = None; }
                    // An in-flight start acknowledgement must not undo Stop.
                    (Phase::Stopping, true) | (Phase::Idle | Phase::Paused, false) => {}
                    _ => return Err("protocol_error"),
                }
            }
            Message::Caption(n, en, vi, seq) if n == self.generation && self.phase == Phase::Listening => {
                match (seq, vi) {
                    // Translations never create a new line or change its English text.
                    // A delayed translation is rendered only for the exact current row.
                    (Some(seq), Some(vi)) => {
                        if self.translate && self.caption.seq == Some(seq) && self.caption.en == en {
                            self.caption.vi = Some(vi);
                        }
                    }
                    (Some(seq), None) => {
                        if self.caption.seq.is_none_or(|old| seq > old) {
                            self.caption = Caption {en,vi:None,seq:Some(seq)};
                        }
                    }
                    // An old helper can still provide English, but uncorrelated Vietnamese
                    // is never accepted or retroactively attached.
                    (None, _) if self.caption.seq.is_none() => self.caption = Caption {en,vi:None,seq:None},
                    _ => {}
                }
            }
            Message::Error(n, code) if n == 0 && self.listing.take().is_some() => self.code = Some(code),
            Message::Error(n, code) if n == self.generation && n > 0 && self.phase.busy() => {
                if code == "translation_unavailable" && self.phase != Phase::Stopping {
                    self.translate = false;
                    self.caption.vi = None;
                    self.code = Some(code);
                } else if code != "translation_unavailable" {
                    // The contract releases first; terminate defensively and never reopen.
                    return Err(code);
                }
            }
            Message::Status(..) | Message::Caption(..) | Message::Error(..) => {}
        }
        Ok(())
    }

    pub fn tick(&mut self, now: Instant) {
        if self.deadline.is_some_and(|d| now >= d) || self.listing.is_some_and(|d| now >= d) {
            self.fail("timeout");
            return;
        }
        for _ in 0..QUEUE {
            let Some(helper) = self.helper.as_ref() else { break; };
            if helper.broken.load(Ordering::Acquire) { self.fail("protocol_error"); break; }
            match helper.receive.try_recv() {
                Ok(message) => {
                    if let Err(code) = self.event(message) { self.fail(code); break; }
                }
                Err(TryRecvError::Disconnected) => { self.fail("protocol_error"); break; }
                Err(TryRecvError::Empty) => break,
            }
        }
    }

    pub fn call(&mut self, method: &str, arg: &Value) -> Value {
        match method {
            "live_list" => self.refresh(),
            "live_start" => self.start(arg),
            "live_stop" => self.stop(false),
            "live_pause" => self.stop(true),
            _ => {}
        }
        json!({"phase":self.phase.label(),"listing":self.listing.is_some(),"code":self.code,
            "devices":self.devices.iter().map(Device::value).collect::<Vec<_>>(),
            "en":self.caption.en,"vi":self.caption.vi})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_is_bounded_and_allowlisted() {
        let mut oversized = io::Cursor::new(vec![b'x'; LINE_MAX + 1]);
        assert!(read_line(&mut oversized).is_err());
        assert!(parse(br#"{"v":1,"type":"caption","session":0,"en":"x","vi":null}"#).is_none());
        assert!(parse(br#"{"v":1,"type":"status","session":1,"state":"started","url":"https://example.com"}"#).is_none());
        assert!(parse(br#"{"v":1,"type":"error","session":1,"code":"arbitrary"}"#).is_none());
        assert!(parse(br#"{"v":1,"type":"caption","session":1,"en":"x","vi":null}"#).is_some());
    }

    #[test]
    fn opening_does_not_spawn_and_consent_and_mic_are_validated() {
        let mut live = Live::default();
        assert_eq!(live.call("live_state", &Value::Null)["phase"], "idle");
        assert!(!live.active_worker());
        assert!(live.start_args(&json!({"source":"mic","device_id":null,"translate":false,"consent":false})).is_none());
        assert!(live.start_args(&json!({"source":"playback","device_id":null,"translate":true,"consent":false})).is_none());
        assert!(live.start_args(&json!({"source":"playback","device_id":null,"translate":false,"consent":false})).is_some());
        assert!(colocated(&std::env::temp_dir().join("nonexistent-sig4a-dashboard.exe")).is_none());
    }

    #[test]
    fn stopped_and_old_session_events_never_repopulate_captions() {
        let mut live = Live { generation:2, phase:Phase::Stopping, pause:true, ..Live::default() };
        live.event(Message::Status(2, true)).unwrap();
        live.event(Message::Caption(2,"late during stop".into(),None,None)).unwrap();
        live.event(Message::Status(2, false)).unwrap();
        assert_eq!(live.phase, Phase::Paused);
        live.event(Message::Caption(2,"late after stop".into(),None,None)).unwrap();
        assert_eq!(live.caption.en, "");
        live.phase = Phase::Listening;
        live.generation = 3;
        live.event(Message::Caption(2,"old generation".into(),None,None)).unwrap();
        assert_eq!(live.caption.en, "");
        live.event(Message::Caption(3,"English".into(),Some("Việt".into()),None)).unwrap();
        assert_eq!(live.caption.en, "English");
        assert_eq!(live.caption.vi, None);
    }
    #[test]
    fn vietnamese_only_updates_the_exact_current_english_row() {
        let mut live = Live {generation:4,phase:Phase::Listening,translate:true,..Live::default()};
        live.event(Message::Caption(4,"future only".into(),Some("not authorized".into()),Some(5))).unwrap();
        assert_eq!(live.caption.en, "");
        live.event(Message::Caption(4,"first".into(),None,Some(1))).unwrap();
        live.event(Message::Caption(4,"different English".into(),Some("incorrect association".into()),Some(1))).unwrap();
        assert_eq!(live.caption.vi, None);
        live.event(Message::Caption(4,"first".into(),Some("một".into()),Some(1))).unwrap();
        assert_eq!(live.caption.vi.as_deref(), Some("một"));
        live.event(Message::Caption(4,"first".into(),None,Some(1))).unwrap();
        assert_eq!(live.caption.vi.as_deref(), Some("một"));
        live.event(Message::Caption(4,"second".into(),None,Some(2))).unwrap();
        live.event(Message::Caption(4,"first".into(),Some("old translation".into()),Some(1))).unwrap();
        live.event(Message::Caption(3,"second".into(),Some("old session".into()),Some(2))).unwrap();
        assert_eq!(live.caption.en, "second");
        assert_eq!(live.caption.vi, None);
    }

}
