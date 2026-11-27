//! Supervised, local-only helper for the optional Windows Live Translate experiment.
use serde_json::{json, Value};
use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::os::windows::{io::{AsRawHandle, FromRawHandle, OwnedHandle}, process::CommandExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Condvar, Mutex, atomic::{AtomicBool, Ordering}, mpsc::{self, Receiver, SyncSender, TryRecvError}};
use std::time::{Duration, Instant};
use windows_sys::Win32::{
    Foundation::{GetLastError, ERROR_ALREADY_EXISTS, INVALID_HANDLE_VALUE},
    System::{Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Thread32First, Thread32Next, THREADENTRY32, TH32CS_SNAPTHREAD},
        JobObjects::{AssignProcessToJobObject, CreateJobObjectW, SetInformationJobObject, TerminateJobObject, JobObjectExtendedLimitInformation, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE},
        Threading::{CreateMutexW, OpenThread, ResumeThread, CREATE_NO_WINDOW, CREATE_SUSPENDED, THREAD_SUSPEND_RESUME}},
};

const LINE_MAX: usize = 16_384;
const SESSION_MAX: u64 = (1 << 53) - 1;
const STOP_TIMEOUT: Duration = Duration::from_secs(2);
const START_TIMEOUT: Duration = Duration::from_secs(35);
const CHECK_START_TIMEOUT: Duration = Duration::from_secs(6);
const CHECK_WINDOW: Duration = Duration::from_secs(5);
const CHECK_CLEANUP: Duration = Duration::from_millis(500);
const CHECK_SAMPLE_MAX_UTF16: usize = 512;
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
    CheckStarted(u64),
    CheckStopped(u64, bool),
    CheckMeter(u64, u8, bool),
    CheckSample(u64, String),
    CheckModel(u64, bool),
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
        "check_status" => match v["state"].as_str()? {
            "started" if keys(&v, &["v","type","session","state"], &[]) =>
                Some(Message::CheckStarted(session(&v["session"], false)?)),
            "stopped" if keys(&v, &["v","type","session","state","completed"], &[]) =>
                Some(Message::CheckStopped(session(&v["session"], false)?, v["completed"].as_bool()?)),
            _ => None,
        },
        "check_meter" if keys(&v, &["v","type","session","level","signal"], &[]) => {
            let level = v["level"].as_u64().filter(|n| *n <= 100)? as u8;
            Some(Message::CheckMeter(session(&v["session"], false)?, level, v["signal"].as_bool()?))
        }
        "check_sample" if keys(&v, &["v","type","session","en"], &[]) => {
            let en = bounded(&v["en"], CHECK_SAMPLE_MAX_UTF16)?;
            if en.encode_utf16().take(CHECK_SAMPLE_MAX_UTF16 + 1).count() > CHECK_SAMPLE_MAX_UTF16 {
                return None;
            }
            Some(Message::CheckSample(session(&v["session"], false)?, en.into()))
        }
        "check_model" if keys(&v, &["v","type","session","state"], &[]) => {
            let ready = match v["state"].as_str()? { "ready" => true, "unavailable" => false, _ => return None };
            Some(Message::CheckModel(session(&v["session"], false)?, ready))
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

pub(crate) fn colocated(current_exe: &Path) -> Option<PathBuf> {
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


#[derive(Default)]
struct Watch {
    deadline: Option<(Instant, &'static str)>,
    check: Option<CheckWatch>,
    exit: bool,
    tripped: Option<&'static str>,
}
#[derive(Clone, Copy)]
struct CheckWatch {
    generation: u64,
    started: bool,
    hard_deadline: Instant,
}
impl Watch {
    fn expiry(&self) -> Option<(Instant, &'static str)> {
        match (self.check, self.deadline) {
            (Some(check), Some(soft)) if soft.0 < check.hard_deadline => Some(soft),
            (Some(check), _) => Some((check.hard_deadline, "timeout")),
            (None, soft) => soft,
        }
    }

    /// Timestamp the first matching native Started on the protocol reader, even
    /// if WebView/Tao is blocked. Duplicates, stopped checks and expired setup
    /// cannot add time to the hard deadline.
    fn check_started(&mut self, generation: u64, now: Instant) {
        let Some(check) = &mut self.check else { return; };
        if self.tripped.is_none() && check.generation == generation && !check.started && now < check.hard_deadline {
            check.started = true;
            check.hard_deadline = now + CHECK_WINDOW + CHECK_CLEANUP;
        }
    }
}

type WatchState = Arc<(Mutex<Watch>, Condvar)>;

fn monitor(job: OwnedHandle, state: WatchState) {
    let (lock, wake) = &*state;
    let mut guard = lock.lock().unwrap();
    loop {
        if guard.exit { return; }
        match guard.expiry() {
            None => guard = wake.wait(guard).unwrap(),
            Some((end, code)) if Instant::now() >= end => {
                guard.tripped = Some(code);
                drop(guard);
                // Independent of Tao/WebView progress; every process in this Job is terminated.
                unsafe { TerminateJobObject(job.as_raw_handle() as _, 1); }
                return;
            }
            Some((end, _)) => {
                let remaining = end.saturating_duration_since(Instant::now());
                guard = wake.wait_timeout(guard, remaining).unwrap().0;
            }
        }
    }
}

struct Helper {
    // A local-session singleton also covers two dashboards raced before their HWNDs exist.
    _singleton: OwnedHandle,
    job: Option<OwnedHandle>,
    child: Child,
    send: SyncSender<Value>,
    receive: Receiver<(Message, Instant)>,
    broken: Arc<AtomicBool>,
    watch: WatchState,
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
        let monitor_job = job.try_clone().map_err(|_| "internal")?;
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
        let watch = Arc::new((Mutex::new(Watch::default()), Condvar::new()));
        let read_watch = watch.clone();
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
                let now = Instant::now();
                if let Message::CheckStarted(id) = &message {
                    read_watch.0.lock().unwrap().check_started(*id, now);
                    read_watch.1.notify_one();
                }
                // Fail on a flooding helper instead of accumulating unbounded events.
                if events.try_send((message, now)).is_err() { break; }
            }
            read_failed.store(true, Ordering::Release);
        });
        let watcher = watch.clone();
        std::thread::spawn(move || monitor(monitor_job, watcher));
        Ok(Self {_singleton:singleton, job:Some(job), child, send, receive, broken, watch})
    }

    fn arm(&self, deadline: Option<(Instant, &'static str)>) {
        let (lock, wake) = &*self.watch;
        lock.lock().unwrap().deadline = deadline;
        wake.notify_one();
    }

    fn begin_check(&self, generation: u64, hard_deadline: Instant) {
        let mut watch = self.watch.0.lock().unwrap();
        watch.check = Some(CheckWatch {generation, started:false, hard_deadline});
        drop(watch);
        self.watch.1.notify_one();
    }

    fn end_check(&self) {
        self.watch.0.lock().unwrap().check = None;
        self.watch.1.notify_one();
    }

    fn tripped(&self) -> Option<&'static str> {
        self.watch.0.lock().unwrap().tripped
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
        // Both handles live only in the dashboard. Explicit termination also
        // covers an independent watchdog that still owns the duplicate.
        if let Some(job) = self.job.take() {
            unsafe { TerminateJobObject(job.as_raw_handle() as _, 1); }
        }
        self.watch.0.lock().unwrap().exit = true;
        self.watch.1.notify_one();
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
#[derive(Default)]
struct AudioCheck {
    level: u8,
    signal: bool,
    model: Option<bool>,
    sample: Option<String>,
    outcome: Option<&'static str>,
}
impl AudioCheck {
    fn value(&self, phase: Phase) -> Value {
        json!({"phase": phase.label(), "level":self.level,"signal":self.signal,
            "model":self.model,"sample":self.sample,"outcome":self.outcome})
    }

    fn finish(&mut self) {
        self.level = 0;
        if !self.signal { self.sample = None; }
        self.outcome = Some(if !self.signal { "silent" }
            else if self.sample.is_some() { "sample" }
            else { match self.model {
                Some(false) => "model_unavailable",
                None => "model_not_ready",
                Some(true) => "audible_no_sample",
            } });
    }
}

pub struct Live {
    helper: Option<Helper>,
    devices: Vec<Device>,
    listing: Option<Instant>,
    phase: Phase,
    pause: bool,
    deadline: Option<Instant>,
    renderer_deadline: Option<Instant>,
    generation: u64,
    translate: bool,
    caption: Caption,
    checking: bool,
    check: Option<AudioCheck>,
    code: Option<&'static str>,
}

impl Default for Live {
    fn default() -> Self {
        Self {helper:None,devices:Vec::new(),listing:None,phase:Phase::Idle,pause:false,deadline:None,
            renderer_deadline:None,generation:0,translate:false,caption:Caption::default(),checking:false,check:None,code:None}
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
        let helper = self.helper.as_ref().unwrap();
        match self.phase {
            Phase::Starting => {
                // Arm before the first Start byte can reach the suspended/Job-bound helper.
                let Some(lease) = self.renderer_deadline else { self.fail("view_lost"); return; };
                if self.checking {
                    helper.begin_check(self.generation, self.deadline.expect("bounded check setup"));
                }
                helper.arm(Some((lease, "view_lost")));
            }
            Phase::Stopping => {
                let stop = self.deadline.expect("Stopping phase has bounded ACK time");
                let lease = self.renderer_deadline.unwrap_or(stop);
                if lease <= stop { helper.arm(Some((lease, "view_lost"))); }
                else { helper.arm(Some((stop, "timeout"))); }
            }
            _ => {}
        }
        if helper.send.try_send(message).is_err() { self.fail("protocol_error"); }
    }

    fn fail(&mut self, code: &'static str) {
        if let Some(mut helper) = self.helper.take() { helper.shutdown(Duration::ZERO); }
        self.listing = None;
        self.phase = Phase::Idle;
        self.deadline = None;
        self.caption = Caption::default();
        if self.checking {
            // Stop/dismiss has already cleared the only presentable check. Still reap
            // the helper, but don't resurrect a result for the formerly selected source.
            if self.check.is_some() && code != "view_lost" {
                self.check = Some(AudioCheck {outcome:Some(match code {
                    "backend_missing" => "backend_missing",
                    "device_missing" | "device_lost" | "permission_denied" => "unavailable",
                    _ => "failed",
                }),..AudioCheck::default()});
            } else {
                self.check = None;
            }
            self.checking = false;
            self.code = None;
        } else {
            self.code = Some(code);
        }
    }

    pub fn renew_view(&mut self, deadline: Instant) {
        self.renderer_deadline = Some(deadline);
        if matches!(self.phase, Phase::Starting | Phase::Listening) {
            if let Some(helper) = &self.helper { helper.arm(Some((deadline, "view_lost"))); }
        }
    }

    pub fn view_lost(&mut self) {
        self.renderer_deadline = None;
        self.check = None;
        if matches!(self.phase, Phase::Starting | Phase::Listening) {
            let code = self.helper.as_ref().and_then(Helper::tripped).unwrap_or("view_lost");
            self.fail(code);
        }
    }

    pub fn close(&mut self) {
        if let Some(mut helper) = self.helper.take() { helper.shutdown(Duration::from_millis(300)); }
        self.caption = Caption::default();
        self.check = None;
    }

    fn refresh(&mut self) {
        if self.phase.busy() || self.listing.is_some() { return; }
        self.caption = Caption::default();
        self.code = None;
        self.listing = Some(Instant::now() + LIST_TIMEOUT);
        self.send(json!({"v":1,"type":"list"}), true);
    }

    fn check_start(&mut self, value: &Value) {
        // A check is a separate operation. Never consume a paused regular session.
        if self.phase != Phase::Idle || self.listing.is_some() { return; }
        self.caption = Caption::default();
        self.check = None;
        self.code = None;
        let selection = (|| {
            if !keys(value, &["source","device_id"], &[]) { return None; }
            let source = source(&value["source"])?;
            Some((source, self.selected_device(source, &value["device_id"])?))
        })();
        let Some((source, device_id)) = selection else {
            self.check = Some(AudioCheck {outcome:Some("unavailable"),..AudioCheck::default()});
            return;
        };
        if self.generation >= SESSION_MAX { self.fail("protocol_error"); return; }
        self.generation += 1;
        self.translate = false;
        self.checking = true;
        self.check = Some(AudioCheck::default());
        self.phase = Phase::Starting;
        self.deadline = Some(Instant::now() + CHECK_START_TIMEOUT);
        self.send(json!({"v":1,"type":"check_start","session":self.generation,"source":source,"device_id":device_id}), true);
    }

    fn check_clear(&mut self) {
        self.check = None;
        if self.checking { self.stop(false); }
    }

    fn start(&mut self, value: &Value) {
        if self.phase.busy() || self.listing.is_some() { return; }
        self.check = None;
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
        let device = self.selected_device(source, &value["device_id"])?;
        Some((source.to_owned(), device, translate))
    }

    fn selected_device(&self, source: &str, value: &Value) -> Option<Option<String>> {
        if value.is_null() {
            if source != "playback" { return None; }
            Some(None)
        } else {
            let id = bounded(value, 512)?;
            if !self.devices.iter().any(|d| d.id == id && d.source == source) { return None; }
            Some(Some(id.to_owned()))
        }
    }

    pub fn stop(&mut self, pause: bool) {
        self.caption = Caption::default();
        self.check = None;
        match self.phase {
            Phase::Starting | Phase::Listening => {
                self.phase = Phase::Stopping;
                self.pause = pause && !self.checking;
                self.deadline = Some(Instant::now() + STOP_TIMEOUT);
                let method = if self.checking {"check_stop"} else {"stop"};
                self.send(json!({"v":1,"type":method,"session":self.generation}), false);
            }
            Phase::Stopping => {
                if !pause { self.pause = false; }
            }
            Phase::Paused if !pause => self.phase = Phase::Idle,
            _ => {}
        }
    }

    #[cfg(test)]
    fn event(&mut self, msg: Message) -> Result<(), &'static str> {
        self.event_at(msg, Instant::now())
    }

    fn event_at(&mut self, msg: Message, received: Instant) -> Result<(), &'static str> {
        match msg {
            Message::CheckStarted(n) if self.checking && n == self.generation => {
                match self.phase {
                    Phase::Starting => {
                        self.phase = Phase::Listening;
                        self.deadline = Some(received + CHECK_WINDOW + CHECK_CLEANUP);
                    }
                    Phase::Stopping => {}
                    _ => return Err("protocol_error"),
                }
            }
            Message::CheckStopped(n, completed) if self.checking && n == self.generation => {
                match self.phase {
                    Phase::Listening if completed => {
                        if let Some(check) = &mut self.check { check.finish(); }
                    }
                    Phase::Starting | Phase::Listening | Phase::Stopping if !completed || self.phase == Phase::Stopping => {
                        self.check = None;
                    }
                    _ => return Err("protocol_error"),
                }
                self.checking = false;
                self.phase = Phase::Idle;
                self.deadline = None;
                if let Some(helper) = &self.helper { helper.end_check(); helper.arm(None); }
            }
            Message::CheckMeter(n, level, signal)
                if self.checking && n == self.generation && self.phase == Phase::Listening =>
            {
                let check = self.check.as_mut().ok_or("protocol_error")?;
                check.level = level;
                check.signal |= signal;
            }
            Message::CheckSample(n, sample)
                if self.checking && n == self.generation && self.phase == Phase::Listening =>
            {
                let check = self.check.as_mut().ok_or("protocol_error")?;
                if check.sample.is_none() { check.sample = Some(sample); }
            }
            Message::CheckModel(n, ready) if self.checking && n == self.generation && self.phase == Phase::Listening => {
                let check = self.check.as_mut().ok_or("protocol_error")?;
                check.model = Some(ready);
            }
            Message::Devices(devices) if self.listing.take().is_some() => self.devices = devices,
            Message::Devices(_) => return Err("protocol_error"),
            Message::Status(n, started) if n == self.generation && !self.checking => {
                match (self.phase, started) {
                    (Phase::Starting, true) => { self.phase = Phase::Listening; self.deadline = None; }
                    (Phase::Stopping, false) => {
                        self.phase = if self.pause { Phase::Paused } else { Phase::Idle };
                        self.deadline = None;
                        if let Some(helper) = &self.helper { helper.arm(None); }
                    }
                    // An in-flight start acknowledgement must not undo Stop.
                    (Phase::Stopping, true) | (Phase::Idle | Phase::Paused, false) => {}
                    _ => return Err("protocol_error"),
                }
            }
            Message::Caption(n, en, vi, seq) if n == self.generation && self.phase == Phase::Listening && !self.checking => {
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
                if code == "translation_unavailable" && !self.checking && self.phase != Phase::Stopping {
                    self.translate = false;
                    self.caption.vi = None;
                    self.code = Some(code);
                } else if code != "translation_unavailable" || self.checking {
                    // The contract releases first; terminate defensively and never reopen.
                    return Err(code);
                }
            }
            Message::Status(..) | Message::Caption(..) | Message::Error(..) |
            Message::CheckStarted(..) | Message::CheckStopped(..) | Message::CheckMeter(..) |
            Message::CheckSample(..) | Message::CheckModel(..) => {}
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
            if let Some(code) = helper.tripped() { self.fail(code); break; }
            if helper.broken.load(Ordering::Acquire) { self.fail("protocol_error"); break; }
            match helper.receive.try_recv() {
                Ok((message, received)) => {
                    if let Err(code) = self.event_at(message, received) { self.fail(code); break; }
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
            "live_pause" if !self.checking => self.stop(true),
            "live_check_start" => self.check_start(arg),
            "live_check_clear" | "live_check_stop" => self.check_clear(),
            _ => {}
        }
        json!({"phase":self.phase.label(),"listing":self.listing.is_some(),"code":self.code,
            "checking":self.checking,"check":self.check.as_ref().map(|c| c.value(self.phase)),
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

    #[test]
    fn audio_check_wire_is_separate_strict_and_bounded() {
        assert!(matches!(parse(br#"{"v":1,"type":"check_status","session":2,"state":"started"}"#), Some(Message::CheckStarted(2))));
        assert!(matches!(parse(br#"{"v":1,"type":"check_status","session":2,"state":"stopped","completed":true}"#), Some(Message::CheckStopped(2, true))));
        assert!(matches!(parse(br#"{"v":1,"type":"check_meter","session":2,"level":100,"signal":true}"#), Some(Message::CheckMeter(2,100,true))));
        assert!(matches!(parse(br#"{"v":1,"type":"check_model","session":2,"state":"unavailable"}"#), Some(Message::CheckModel(2,false))));
        for line in [
            r#"{"v":1,"type":"check_status","session":2,"state":"stopped"}"#,
            r#"{"v":1,"type":"check_status","session":2,"state":"started","completed":true}"#,
            r#"{"v":1,"type":"check_meter","session":2,"level":101,"signal":false}"#,
            r#"{"v":1,"type":"check_meter","session":2,"level":3,"signal":0}"#,
            r#"{"v":1,"type":"check_sample","session":0,"en":"text"}"#,
            r#"{"v":1,"type":"check_sample","session":2,"en":"text","vi":"test"}"#,
            r#"{"v":1,"type":"check_model","session":2,"state":"running"}"#,
        ] { assert!(parse(line.as_bytes()).is_none(), "{line}"); }
        for (en, accepted) in [("x".repeat(512), true), ("x".repeat(513), false),
                              ("𐐀".repeat(256), true), (format!("{}x", "𐐀".repeat(256)), false)] {
            let line = json!({"v":1,"type":"check_sample","session":2,"en":en});
            assert_eq!(parse(line.to_string().as_bytes()).is_some(), accepted);
        }
    }

    #[test]
    fn check_watchdog_never_renews_hard_bound_from_heartbeat_or_repeat_started() {
        let now = Instant::now();
        let mut watch = Watch {
            deadline:Some((now + Duration::from_secs(3),"view_lost")),
            check:Some(CheckWatch{generation:7,started:false,hard_deadline:now + CHECK_START_TIMEOUT}),
            ..Watch::default()
        };
        watch.check_started(6, now + Duration::from_secs(1));
        assert_eq!(watch.check.unwrap().hard_deadline, now + CHECK_START_TIMEOUT);
        watch.check_started(7, now + Duration::from_secs(1));
        let hard = now + Duration::from_secs(1) + CHECK_WINDOW + CHECK_CLEANUP;
        assert_eq!(watch.check.unwrap().hard_deadline, hard);
        watch.check_started(7, now + Duration::from_secs(2));
        watch.deadline = Some((now + Duration::from_secs(12),"view_lost"));
        assert_eq!(watch.expiry(), Some((hard,"timeout")));
        watch.deadline = Some((now + Duration::from_secs(4),"view_lost"));
        assert_eq!(watch.expiry(), Some((now + Duration::from_secs(4),"view_lost")));
    }

    #[test]
    fn check_and_regular_events_never_cross_or_refill_stopped_test() {
        let mut live = Live {generation:10,checking:true,check:Some(AudioCheck::default()),phase:Phase::Starting,..Live::default()};
        live.event(Message::Status(10,true)).unwrap();
        assert_eq!(live.phase, Phase::Starting);
        live.event(Message::CheckStarted(10)).unwrap();
        live.event(Message::CheckModel(10,true)).unwrap();
        live.event(Message::CheckMeter(9,99,true)).unwrap();
        assert_eq!(live.check.as_ref().unwrap().level, 0);
        live.event(Message::CheckMeter(10,55,true)).unwrap();
        live.event(Message::CheckMeter(10,0,false)).unwrap();
        live.event(Message::CheckSample(10,"first".into())).unwrap();
        live.event(Message::CheckSample(10,"second".into())).unwrap();
        live.event(Message::Caption(10,"regular".into(),Some("VI".into()),None)).unwrap();
        assert!(live.caption.en.is_empty());
        live.start(&json!({"source":"playback","device_id":null,"translate":false,"consent":false}));
        assert_eq!(live.generation,10);
        live.event(Message::CheckStopped(10,true)).unwrap();
        assert_eq!(live.check.as_ref().unwrap().outcome, Some("sample"));
        assert_eq!(live.check.as_ref().unwrap().sample.as_deref(), Some("first"));
        live.check_clear();
        live.event(Message::CheckSample(10,"late check result".into())).unwrap();
        assert!(live.check.is_none());
    }

    #[test]
    fn audible_no_sample_distinguishes_ready_missing_and_startup_pending() {
        for (model, outcome) in [(None, "model_not_ready"), (Some(false), "model_unavailable"),
                                 (Some(true), "audible_no_sample")] {
            let mut check = AudioCheck {signal:true,model,..AudioCheck::default()};
            check.finish();
            assert_eq!(check.outcome, Some(outcome));
        }
    }

    #[test]
    fn audio_check_cannot_replace_paused_regular_session() {
        let mut live = Live {generation:11,phase:Phase::Paused,translate:true,..Live::default()};
        let state = live.call("live_check_start", &json!({"source":"playback","device_id":null}));
        live.call("live_check_stop", &Value::Null);
        assert_eq!(state["phase"], "paused");
        assert_eq!(live.phase, Phase::Paused);
        assert_eq!(live.generation, 11);
        assert!(live.translate && !live.checking && live.check.is_none() && !live.active_worker());
    }

    #[test]
    fn cancelled_audio_check_never_publishes_stale_timeout_or_hidden_results() {
        for dismiss in ["live_check_clear", "live_check_stop", "live_stop"] {
            let mut live = Live {generation:3,checking:true,phase:Phase::Stopping,
                check:Some(AudioCheck {signal:true,sample:Some("formerly selected".into()),..AudioCheck::default()}),
                deadline:Some(Instant::now()),..Live::default()};
            live.call(dismiss, &Value::Null);
            live.tick(Instant::now()); // Failed ACK still closes the Job, but cannot publish stale feedback.
            let state = live.call("live_state", &Value::Null);
            assert_eq!(state["phase"], "idle");
            assert!(state["code"].is_null() && state["check"].is_null());
            assert!(!live.checking);
        }

        let mut live = Live {generation:4,checking:true,phase:Phase::Listening,
            check:Some(AudioCheck::default()),..Live::default()};
        live.view_lost();
        live.event(Message::CheckSample(4,"late hidden sample".into())).unwrap();
        assert_eq!(live.phase, Phase::Idle);
        assert!(live.check.is_none() && live.code.is_none());
    }

}
