//! Explicit, one-shot English model setup through the colocated Live Translate helper.
//! This module does not inspect model files on page visits and never starts audio.
use serde_json::{json, Value};
use std::{
    io::{self, BufRead, BufReader, Write},
    os::windows::{
        io::{AsHandle, AsRawHandle, FromRawHandle, OwnedHandle},
        process::CommandExt,
    },
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{GetLastError, ERROR_ALREADY_EXISTS, INVALID_HANDLE_VALUE, WAIT_OBJECT_0},
    Security::Cryptography::{BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG},
    System::{
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
        },
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        },
        Threading::{
            CreateMutexW, OpenThread, ResumeThread, TerminateProcess, WaitForSingleObject,
            CREATE_NO_WINDOW, CREATE_SUSPENDED, THREAD_SUSPEND_RESUME,
        },
    },
};

const MAX_LINE: usize = 16 * 1024;
const TOTAL: u64 = 310_414_022;
const IDLE_TIMEOUT: Duration = Duration::from_secs(20 * 60);
const CANCEL_TIMEOUT: Duration = Duration::from_secs(2);
const QUEUE: usize = 8;
const SESSION_LIMIT: u64 = 9_007_199_254_740_991;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Event {
    session: u64,
    state: &'static str,
    received: Option<u64>,
    code: Option<&'static str>,
}
enum ReaderEvent {
    Event(Event),
    Invalid,
    CleanEof,
}

fn unique_top_level_keys(bytes: &[u8]) -> bool {
    let mut i = 0;
    while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
        i += 1;
    }
    if bytes.get(i) != Some(&b'{') {
        return false;
    }
    let mut object_depth = 0i32;
    let mut array_depth = 0i32;
    let mut keys = std::collections::HashSet::<String>::new();
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                let start = i;
                i += 1;
                let mut escaped = false;
                while i < bytes.len() {
                    let b = bytes[i];
                    i += 1;
                    if escaped {
                        escaped = false;
                        continue;
                    }
                    if b == b'\\' {
                        escaped = true;
                        continue;
                    }
                    if b == b'"' {
                        break;
                    }
                }
                if i > bytes.len() || bytes.get(i.saturating_sub(1)) != Some(&b'"') {
                    return false;
                }
                if object_depth == 1 && array_depth == 0 {
                    let mut after = i;
                    while bytes.get(after).is_some_and(u8::is_ascii_whitespace) {
                        after += 1;
                    }
                    if bytes.get(after) == Some(&b':') {
                        let Ok(key) = serde_json::from_slice::<String>(&bytes[start..i]) else {
                            return false;
                        };
                        if !keys.insert(key) {
                            return false;
                        }
                    }
                }
                continue;
            }
            b'{' => object_depth += 1,
            b'}' => object_depth -= 1,
            b'[' => array_depth += 1,
            b']' => array_depth -= 1,
            _ => {}
        }
        if object_depth < 0 || array_depth < 0 {
            return false;
        }
        i += 1;
    }
    object_depth == 0 && array_depth == 0
}

fn setup_transition(action: &str, stage: u8, state: &str) -> Option<u8> {
    if state == "failed" {
        return Some(stage);
    }
    if state == "cancelled" {
        // The helper emits committing before the last cancellation checkpoint and
        // sends cancelled only once rollback/cleanup has completed.
        return (stage <= 6).then_some(stage);
    }
    match (action, stage, state) {
        ("verify", 0, "checking_files") => Some(1),
        ("verify", 1, "ready") => Some(2),
        ("install", 0, "checking_files") => Some(1),
        ("install", 1, "downloading") => Some(2),
        ("install", 1, "ready") => Some(7), // Existing assets passed all pinned hashes.
        ("install", 2, "downloading") => Some(2),
        ("install", 2, "checking_archive") => Some(3),
        ("install", 3, "extracting") => Some(4),
        ("install", 4, "checking_files") => Some(5),
        ("install", 5, "committing") => Some(6),
        ("install", 6, "ready") => Some(7),
        _ => None,
    }
}

fn fresh_challenge() -> Option<String> {
    let mut bytes = [0u8; 16];
    let status = unsafe {
        BCryptGenRandom(
            std::ptr::null_mut(),
            bytes.as_mut_ptr(),
            bytes.len() as u32,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    if status < 0 {
        return None;
    }
    Some(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn parse_line(bytes: &[u8], expected: u64) -> Option<Event> {
    if !unique_top_level_keys(bytes) {
        return None;
    }
    let v: Value = serde_json::from_slice(bytes).ok()?;
    if v["v"].as_u64()? != 1
        || v["type"].as_str()? != "model_setup"
        || v["session"].as_u64()? != expected
    {
        return None;
    }
    let state = match v["state"].as_str()? {
        "downloading" => "downloading",
        "checking_archive" => "checking_archive",
        "extracting" => "extracting",
        "checking_files" => "checking_files",
        "committing" => "committing",
        "ready" => "ready",
        "cancelled" => "cancelled",
        "failed" => "failed",
        _ => return None,
    };
    let code = if state == "failed" {
        let code = v["code"].as_str()?;
        Some(match code {
            "busy" => "busy",
            "not_installed" => "not_installed",
            "needs_repair" => "needs_repair",
            "access_denied" => "access_denied",
            "insufficient_space" => "insufficient_space",
            "network_unavailable" => "network_unavailable",
            "timeout" => "timeout",
            "digest_mismatch" => "digest_mismatch",
            "archive_invalid" => "archive_invalid",
            "invalid_path" => "invalid_path",
            "cleanup_incomplete" => "cleanup_incomplete",
            "installer_internal" => "installer_internal",
            _ => return None,
        })
    } else {
        None
    };
    let received = if state == "downloading" {
        if v["total"].as_u64()? != TOTAL {
            return None;
        }
        let n = v["received"].as_u64()?;
        if n > TOTAL {
            return None;
        }
        Some(n)
    } else {
        None
    };
    let expected_keys: &[&str] = if state == "downloading" {
        &["v", "type", "session", "state", "received", "total"]
    } else if state == "failed" {
        &["v", "type", "session", "state", "code"]
    } else {
        &["v", "type", "session", "state"]
    };
    let obj = v.as_object()?;
    if obj.len() != expected_keys.len() || obj.keys().any(|k| !expected_keys.contains(&k.as_str()))
    {
        return None;
    }
    Some(Event {
        session: expected,
        state,
        received,
        code,
    })
}

fn read_line(reader: &mut impl BufRead) -> io::Result<Option<Vec<u8>>> {
    let mut line = Vec::new();
    loop {
        let buf = reader.fill_buf()?;
        if buf.is_empty() {
            return if line.is_empty() {
                Ok(None)
            } else {
                Err(io::ErrorKind::InvalidData.into())
            };
        }
        let newline = buf.iter().position(|b| *b == b'\n');
        let used = newline.unwrap_or(buf.len());
        if line.len() + used > MAX_LINE {
            return Err(io::ErrorKind::InvalidData.into());
        }
        line.extend_from_slice(&buf[..used]);
        reader.consume(used + usize::from(newline.is_some()));
        if newline.is_some() {
            return Ok(Some(line));
        }
    }
}

fn job() -> io::Result<OwnedHandle> {
    unsafe {
        let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if h.is_null() {
            return Err(io::Error::last_os_error());
        }
        let owner = OwnedHandle::from_raw_handle(h as _);
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if SetInformationJobObject(
            h,
            JobObjectExtendedLimitInformation,
            (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            std::mem::size_of_val(&info) as u32,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(owner)
    }
}
fn assign_and_resume(child: &Child, job: &OwnedHandle) -> io::Result<()> {
    unsafe {
        if AssignProcessToJobObject(job.as_raw_handle() as _, child.as_raw_handle() as _) == 0 {
            return Err(io::Error::last_os_error());
        }
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snap == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let _snapshot = OwnedHandle::from_raw_handle(snap as _);
        let mut e: THREADENTRY32 = std::mem::zeroed();
        e.dwSize = std::mem::size_of_val(&e) as u32;
        let mut n = Thread32First(snap, &mut e);
        let mut found = None;
        while n != 0 {
            if e.th32OwnerProcessID == child.id() {
                if found.is_some() {
                    return Err(io::ErrorKind::InvalidData.into());
                }
                found = Some(e.th32ThreadID);
            }
            e.dwSize = std::mem::size_of_val(&e) as u32;
            n = Thread32Next(snap, &mut e);
        }
        let h = OpenThread(
            THREAD_SUSPEND_RESUME,
            0,
            found.ok_or(io::ErrorKind::NotFound)?,
        );
        if h.is_null() {
            return Err(io::Error::last_os_error());
        }
        let _thread = OwnedHandle::from_raw_handle(h as _);
        if ResumeThread(h) != 1 {
            return Err(io::ErrorKind::Other.into());
        }
        Ok(())
    }
}

struct Worker {
    _lock: OwnedHandle,
    child: Child,
    job: OwnedHandle,
    send: SyncSender<Value>,
    events: Receiver<ReaderEvent>,
    started: Instant,
    last_event: Instant,
    cancel_ms: Arc<AtomicU64>,
    start_revoked: Arc<Mutex<bool>>,
    tripped: Arc<Mutex<Option<&'static str>>>,
    view_deadline: Arc<Mutex<Instant>>,
    monitor_done: Arc<AtomicBool>,
    terminal: Option<(Event, Instant)>,
    cancel_at: Option<Instant>,
    action: &'static str,
    stage: u8,
    process_exit: Option<Option<i32>>,
    process_exit_seen_at: Option<Instant>,
    clean_eof: bool,
    session: u64,
    received: u64,
    state: &'static str,
    code: Option<&'static str>,
    done: bool,
}

fn write_start_if_permitted(
    stdin: &mut impl Write,
    frame: &[u8],
    original_deadline: Instant,
    revoked: &Mutex<bool>,
    visible: impl FnOnce() -> bool,
) -> Result<(), &'static str> {
    // A later renewable watchdog heartbeat cannot authorize the **first** write.
    // The pipe is new and empty: issue one short write including the newline;
    // a failed/partial write never gets a continuation or a retry.
    let revoked = revoked.lock().unwrap();
    if *revoked || !visible() || Instant::now() >= original_deadline {
        return Err("cleanup_incomplete");
    }
    match stdin.write(frame) {
        Ok(written) if written == frame.len() => Ok(()),
        _ => Err("installer_internal"),
    }
}

impl Worker {
    fn open(
        session: u64,
        action: &str,
        initial_view_deadline: Instant,
        hwnd: isize,
    ) -> Result<Self, &'static str> {
        let action = match action {
            "install" => "install",
            "verify" => "verify",
            _ => return Err("installer_internal"),
        };
        if Instant::now() >= initial_view_deadline || !crate::win::visible_live_window(hwnd) {
            return Err("cleanup_incomplete");
        }
        let helper = crate::model_setup_path::SetupHelper::open(
            &std::env::current_exe().map_err(|_| "backend_missing")?,
        )
        .map_err(|error| error.code())?;
        let exe = &helper.executable;
        // This is only a UI-install-session guard. The managed helper owns the shared
        // package lock that arbitrates install, verify, capture, and Audio Check.
        let lock = unsafe {
            let name: Vec<u16> = "Local\\SIG4A.LiveTranslate.ModelSetup.UI.v1"
                .encode_utf16()
                .chain([0])
                .collect();
            let h = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
            let existed = GetLastError() == ERROR_ALREADY_EXISTS;
            if h.is_null() {
                return Err("installer_internal");
            }
            let owner = OwnedHandle::from_raw_handle(h as _);
            if existed {
                return Err("busy");
            }
            owner
        };
        let job = job().map_err(|_| "installer_internal")?;
        helper.confirm().map_err(|error| error.code())?;
        if Instant::now() >= initial_view_deadline || !crate::win::visible_live_window(hwnd) {
            return Err("cleanup_incomplete");
        }
        let mut child = Command::new(exe)
            .arg("--english-model-setup")
            .current_dir(exe.parent().ok_or("backend_missing")?)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED)
            .spawn()
            .map_err(|_| "backend_missing")?;
        // If process creation itself was slow, the suspended process never starts.
        if Instant::now() >= initial_view_deadline || !crate::win::visible_live_window(hwnd) {
            let _ = child.kill();
            let _ = child.wait();
            return Err("cleanup_incomplete");
        }
        if let Err(error) = helper.confirm() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error.code());
        }
        if Instant::now() >= initial_view_deadline || !crate::win::visible_live_window(hwnd) {
            let _ = child.kill();
            let _ = child.wait();
            return Err("cleanup_incomplete");
        }
        if assign_and_resume(&child, &job).is_err() {
            let _ = child.kill();
            let _ = child.wait();
            return Err("installer_internal");
        }
        drop(helper);
        let mut stdin = child.stdin.take().ok_or("installer_internal")?;
        let stdout = child.stdout.take().ok_or("installer_internal")?;
        let (send, commands) = mpsc::sync_channel::<Value>(2);
        let (event_tx, events) = mpsc::sync_channel::<ReaderEvent>(QUEUE);
        let started = Instant::now();
        let last_event_ms = Arc::new(AtomicU64::new(0));
        let cancel_ms = Arc::new(AtomicU64::new(0));
        let start_revoked = Arc::new(Mutex::new(false));
        let tripped = Arc::new(Mutex::new(None));
        let view_deadline = Arc::new(Mutex::new(initial_view_deadline));
        let monitor_done = Arc::new(AtomicBool::new(false));
        let monitor_job = job.try_clone().map_err(|_| "installer_internal")?;
        let monitor_process = child
            .as_handle()
            .try_clone_to_owned()
            .map_err(|_| "installer_internal")?;
        let monitor_last = last_event_ms.clone();
        let monitor_cancel = cancel_ms.clone();
        let monitor_trip = tripped.clone();
        let monitor_view_deadline = view_deadline.clone();
        let monitor_stop = monitor_done.clone();
        std::thread::spawn(move || loop {
            if monitor_stop.load(Ordering::Acquire) {
                return;
            }
            let elapsed = started.elapsed();
            let now_ms = elapsed.as_millis().min(u64::MAX as u128) as u64;
            let cancel = monitor_cancel.load(Ordering::Acquire);
            if unsafe { WaitForSingleObject(monitor_process.as_raw_handle() as _, 0) }
                == WAIT_OBJECT_0
            {
                return;
            }
            let code = if !crate::win::visible_live_window(hwnd)
                || Instant::now() >= *monitor_view_deadline.lock().unwrap()
            {
                Some("cleanup_incomplete")
            } else if elapsed >= IDLE_TIMEOUT {
                Some("timeout")
            } else if cancel != 0
                && now_ms.saturating_sub(cancel) >= CANCEL_TIMEOUT.as_millis() as u64
            {
                Some("cleanup_incomplete")
            } else if now_ms.saturating_sub(monitor_last.load(Ordering::Acquire))
                >= Duration::from_secs(120).as_millis() as u64
            {
                Some("timeout")
            } else {
                None
            };
            if let Some(code) = code {
                monitor_trip.lock().unwrap().get_or_insert(code);
                unsafe {
                    if TerminateJobObject(monitor_job.as_raw_handle() as _, 1) == 0 {
                        TerminateProcess(monitor_process.as_raw_handle() as _, 1);
                    }
                }
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        });
        let writer_cancel = start_revoked.clone();
        let writer_trip = tripped.clone();
        let writer_job = job.try_clone().map_err(|_| "installer_internal")?;
        let writer_process = child
            .as_handle()
            .try_clone_to_owned()
            .map_err(|_| "installer_internal")?;
        let frame = json!({"v":1,"type":"model_setup_start","session":session,"action":action})
            .to_string()
            + "\n";
        std::thread::spawn(move || {
            if let Err(code) = write_start_if_permitted(
                &mut stdin,
                frame.as_bytes(),
                initial_view_deadline,
                &writer_cancel,
                || crate::win::visible_live_window(hwnd) && writer_trip.lock().unwrap().is_none(),
            ) {
                writer_trip.lock().unwrap().get_or_insert(code);
                unsafe {
                    if TerminateJobObject(writer_job.as_raw_handle() as _, 1) == 0 {
                        TerminateProcess(writer_process.as_raw_handle() as _, 1);
                    }
                }
                return;
            }
            while let Ok(value) = commands.recv() {
                let s = value.to_string();
                if stdin
                    .write_all(s.as_bytes())
                    .and_then(|_| stdin.write_all(b"\n"))
                    .and_then(|_| stdin.flush())
                    .is_err()
                {
                    break;
                }
            }
        });
        let reader_started = started;
        let reader_last_event = last_event_ms.clone();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                match read_line(&mut reader) {
                    Ok(Some(line)) => match parse_line(&line, session) {
                        Some(event) => {
                            reader_last_event.store(
                                reader_started.elapsed().as_millis().min(u64::MAX as u128) as u64,
                                Ordering::Release,
                            );
                            if event_tx.send(ReaderEvent::Event(event)).is_err() {
                                break;
                            }
                        }
                        None => {
                            let _ = event_tx.send(ReaderEvent::Invalid);
                            break;
                        }
                    },
                    Ok(None) => {
                        let _ = event_tx.send(ReaderEvent::CleanEof);
                        break;
                    }
                    Err(_) => {
                        let _ = event_tx.send(ReaderEvent::Invalid);
                        break;
                    }
                }
            }
        });
        Ok(Self {
            _lock: lock,
            child,
            job,
            send,
            events,
            started,
            last_event: Instant::now(),
            cancel_ms,
            start_revoked,
            tripped,
            view_deadline,
            monitor_done,
            terminal: None,
            cancel_at: None,
            action,
            stage: 0,
            process_exit: None,
            process_exit_seen_at: None,
            clean_eof: false,
            session,
            received: 0,
            state: "starting",
            code: None,
            done: false,
        })
    }
    fn cancel(&mut self) {
        if self.done || self.cancel_at.is_some() {
            return;
        }
        *self.start_revoked.lock().unwrap() = true;
        self.cancel_at = Some(Instant::now());
        self.cancel_ms.store(
            self.started.elapsed().as_millis().min(u64::MAX as u128) as u64 + 1,
            Ordering::Release,
        );
        let _ = self
            .send
            .try_send(json!({"v":1,"type":"model_setup_cancel","session":self.session}));
        self.state = "cancelling";
    }
    fn kill(&mut self) {
        if self.done {
            return;
        }
        unsafe {
            if TerminateJobObject(self.job.as_raw_handle() as _, 1) == 0 {
                let _ = self.child.kill();
            }
        }
        let _ = self.child.wait();
        self.monitor_done.store(true, Ordering::Release);
        self.done = true;
        self.state = "failed";
        self.code = Some("cleanup_incomplete");
    }
    fn tick(&mut self) {
        if self.done {
            return;
        }
        let tripped = { *self.tripped.lock().unwrap() };
        if let Some(code) = tripped {
            self.fail(code);
            return;
        }
        for _ in 0..QUEUE {
            match self.events.try_recv() {
                Ok(ReaderEvent::Event(e)) if e.session == self.session => {
                    let terminal = matches!(e.state, "ready" | "cancelled" | "failed");
                    if self.terminal.is_some() || self.clean_eof {
                        self.fail("installer_internal");
                        break;
                    }
                    let Some(stage) = setup_transition(self.action, self.stage, e.state) else {
                        self.fail("installer_internal");
                        break;
                    };
                    if e.state == "downloading" && e.received.is_some_and(|n| n < self.received) {
                        self.fail("installer_internal");
                        break;
                    }
                    self.stage = stage;
                    self.last_event = Instant::now();
                    self.received = e.received.unwrap_or(self.received);
                    if terminal {
                        self.terminal = Some((e, Instant::now()));
                    } else if self.cancel_at.is_none() {
                        // Progress still advances the protocol FSM while the visible
                        // status stays Cancelling; an atomic commit may win the race.
                        self.state = e.state;
                    }
                }
                Ok(ReaderEvent::Event(_)) => {
                    self.fail("installer_internal");
                    break;
                }
                Ok(ReaderEvent::CleanEof) if self.terminal.is_some() && !self.clean_eof => {
                    self.clean_eof = true;
                }
                Ok(ReaderEvent::Invalid) | Ok(ReaderEvent::CleanEof) => {
                    self.fail("installer_internal");
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) if self.clean_eof => break,
                Err(TryRecvError::Disconnected) => {
                    self.fail("installer_internal");
                    break;
                }
            }
        }
        if self.done {
            return;
        }
        if self.started.elapsed() > IDLE_TIMEOUT
            || self.last_event.elapsed() > Duration::from_secs(120)
        {
            self.fail("timeout");
            return;
        }
        if self
            .cancel_at
            .is_some_and(|at| at.elapsed() > CANCEL_TIMEOUT)
        {
            self.fail("cleanup_incomplete");
            return;
        }
        match self.child.try_wait() {
            Ok(Some(status)) => {
                self.monitor_done.store(true, Ordering::Release);
                self.process_exit = Some(status.code());
                self.process_exit_seen_at.get_or_insert_with(Instant::now);
            }
            Err(_) => {
                self.fail("installer_internal");
                return;
            }
            _ => {}
        }
        let tripped = { *self.tripped.lock().unwrap() };
        if let Some(code) = tripped {
            self.fail(code);
            return;
        }
        if let Some(exit) = self.process_exit {
            if !self.clean_eof {
                if self
                    .process_exit_seen_at
                    .is_some_and(|at| at.elapsed() > Duration::from_millis(500))
                {
                    self.fail("installer_internal");
                }
                return;
            }
            match self.terminal.take() {
                Some((event, _)) if exit == Some(0) && event.state == "ready" => {
                    self.state = event.state;
                    self.code = event.code;
                    self.received = event.received.unwrap_or(self.received);
                    self.done = true;
                }
                Some((event, _)) if exit == Some(0) && event.state == "cancelled" => {
                    self.state = "cancelled";
                    self.code = None;
                    self.done = true;
                }
                Some((event, _)) if exit == Some(1) && event.state == "failed" => {
                    self.state = "failed";
                    self.code = event.code.or(Some("installer_internal"));
                    self.done = true;
                }
                Some((event, _)) if event.state == "cancelled" => self.fail("cleanup_incomplete"),
                Some((_, _)) => self.fail("installer_internal"),
                None if self
                    .process_exit_seen_at
                    .is_some_and(|at| at.elapsed() > Duration::from_millis(500)) =>
                {
                    self.fail("installer_internal")
                }
                None => {}
            }
        } else if self
            .terminal
            .as_ref()
            .is_some_and(|(_, at)| at.elapsed() > CANCEL_TIMEOUT)
        {
            self.fail("installer_internal");
        }
    }
    fn fail(&mut self, code: &'static str) {
        if self.done {
            return;
        }
        unsafe {
            if TerminateJobObject(self.job.as_raw_handle() as _, 1) == 0 {
                let _ = self.child.kill();
            }
        }
        let _ = self.child.wait();
        self.monitor_done.store(true, Ordering::Release);
        self.done = true;
        self.state = "failed";
        self.code = Some(code);
    }
    fn value(&self) -> Value {
        let mut v = json!({"active":!self.done,"session":self.session,"state":self.state,"received":self.received,"total":TOTAL,"code":self.code});
        if self.state != "downloading" {
            v.as_object_mut().unwrap().remove("received");
            v.as_object_mut().unwrap().remove("total");
        }
        v
    }
}
pub struct ModelSetup {
    worker: Option<Worker>,
    next: u64,
    pending: Option<(u64, String, String, Instant, Instant)>,
    last: Value,
}
impl Default for ModelSetup {
    fn default() -> Self {
        Self {
            worker: None,
            next: 0,
            pending: None,
            last: json!({"active":false,"state":"unchecked"}),
        }
    }
}
impl ModelSetup {
    fn expire_pending(&mut self, now: Instant) {
        if self
            .pending
            .as_ref()
            .is_some_and(|(_, _, _, _, expires)| now >= *expires)
        {
            if let Some((session, _, _, _, _)) = self.pending.take() {
                self.last = json!({"active":false,"session":session,"state":"failed","code":"stale_confirmation"});
            }
        }
    }
    pub fn state(&mut self) -> Value {
        self.expire_pending(Instant::now());
        if let Some(w) = self.worker.as_mut() {
            w.tick();
            self.last = w.value();
            if w.done {
                self.worker = None;
            }
        }
        self.last.clone()
    }
    /// Arm install only after explicit native confirmation. The returned nonce is bound to one page and expires.
    pub fn confirm_install(
        &mut self,
        page: &str,
        confirmed: bool,
        now: Instant,
    ) -> Option<(u64, String)> {
        self.expire_pending(now);
        if self.worker.is_some() || self.pending.is_some() {
            return None;
        }
        if !confirmed {
            self.last = json!({"active":false,"state":"declined"});
            return None;
        }
        let challenge = fresh_challenge()?;
        self.next = if self.next >= SESSION_LIMIT {
            1
        } else {
            self.next + 1
        };
        self.pending = Some((
            self.next,
            page.to_owned(),
            challenge.clone(),
            now,
            now + Duration::from_secs(10),
        ));
        self.last = json!({"active":false,"session":self.next,"state":"awaiting_heartbeat"});
        Some((self.next, challenge))
    }
    pub fn awaiting_heartbeat(&mut self, page: &str, now: Instant) -> bool {
        self.expire_pending(now);
        self.pending
            .as_ref()
            .is_some_and(|(_, owner, _, issued, expires)| {
                owner == page && now > *issued && now < *expires
            })
    }
    pub fn heartbeat(
        &mut self,
        page: &str,
        challenge: &str,
        now: Instant,
        view_deadline: Instant,
        hwnd: isize,
    ) -> bool {
        self.expire_pending(now);
        let valid = self
            .pending
            .as_ref()
            .is_some_and(|(_, owner, expected, issued, expires)| {
                owner == page && expected == challenge && now > *issued && now < *expires
            });
        if !valid || self.worker.is_some() {
            return false;
        }
        let Some((session, _, _, _, _)) = self.pending.take() else {
            return false;
        };
        self.start(session, "install", view_deadline, hwnd);
        true
    }
    pub fn reject_pending(&mut self, page: &str, challenge: &str, now: Instant) {
        self.expire_pending(now);
        if self
            .pending
            .as_ref()
            .is_some_and(|(_, owner, expected, _, _)| owner == page && expected == challenge)
        {
            let (session, _, _, _, _) = self.pending.take().expect("matching post-modal grant");
            self.last = json!({"active":false,"session":session,"state":"failed","code":"busy"});
        }
    }
    pub fn verify(&mut self, view_deadline: Instant, hwnd: isize) -> Option<u64> {
        self.expire_pending(Instant::now());
        if self.worker.is_some() || self.pending.is_some() {
            return None;
        }
        self.next = if self.next >= SESSION_LIMIT {
            1
        } else {
            self.next + 1
        };
        let session = self.next;
        self.start(session, "verify", view_deadline, hwnd);
        Some(session)
    }
    fn start(&mut self, session: u64, action: &str, view_deadline: Instant, hwnd: isize) {
        match Worker::open(session, action, view_deadline, hwnd) {
            Ok(w) => {
                self.last = w.value();
                self.worker = Some(w);
            }
            Err(code) => {
                self.last = json!({"active":false,"session":session,"state":"failed","code":code})
            }
        }
    }
    pub fn cancel(&mut self, session: u64) {
        if self
            .pending
            .as_ref()
            .is_some_and(|(pending, _, _, _, _)| *pending == session)
        {
            self.pending = None;
            self.last = json!({"active":false,"session":session,"state":"cancelled"});
            return;
        }
        if let Some(w) = self.worker.as_mut() {
            if session == w.session && !w.done {
                w.cancel();
                self.last = w.value();
            }
        }
    }
    pub fn renew_view(&mut self, deadline: Instant) {
        if let Some(worker) = self.worker.as_ref() {
            *worker.view_deadline.lock().unwrap() = deadline;
        }
    }
    pub fn view_lost(&mut self) {
        self.pending = None;
        if let Some(mut w) = self.worker.take() {
            w.cancel();
            w.kill();
        }
        self.last = json!({"active":false,"state":"unchecked"});
    }
    pub fn busy(&mut self) -> bool {
        self.state()["active"].as_bool() == Some(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn start_writer_cannot_send_an_expired_cancelled_or_hidden_grant_to_the_win32_pipe() {
        const START: &[u8] =
            b"{\"v\":1,\"type\":\"model_setup_start\",\"session\":1,\"action\":\"install\"}\n";
        for case in [
            "stalled-spawn",
            "stalled-writer",
            "cancelled",
            "hidden",
            "valid",
        ] {
            // std::io::pipe uses a native Windows one-way pipe. No process, network or assets.
            let (mut reader, mut writer) = io::pipe().unwrap();
            let future = Instant::now() + Duration::from_secs(5);
            let expired = Instant::now() - Duration::from_millis(1);
            let revoked = Mutex::new(case == "cancelled");
            let result = if case == "stalled-writer" {
                // Fall through the visibility check only once the immutable
                // permit expires, as if this writer were held by the scheduler.
                let deadline = Instant::now() + Duration::from_millis(20);
                write_start_if_permitted(&mut writer, START, deadline, &revoked, || {
                    while Instant::now() < deadline {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    true
                })
            } else {
                let deadline = if case == "stalled-spawn" {
                    expired
                } else {
                    future
                };
                write_start_if_permitted(&mut writer, START, deadline, &revoked, || {
                    case != "hidden"
                })
            };
            drop(writer);
            let mut actual = Vec::new();
            reader.read_to_end(&mut actual).unwrap();
            if case == "valid" {
                assert_eq!(result, Ok(()));
                assert_eq!(actual, START);
            } else {
                assert_eq!(result, Err("cleanup_incomplete"), "{case}");
                assert!(actual.is_empty(), "{case}");
            }
        }
    }
    #[test]
    fn protocol_requires_exact_fields_and_pinned_progress() {
        assert_eq!(parse_line(br#"{"v":1,"type":"model_setup","session":2,"state":"downloading","received":3,"total":310414022}"#,2).unwrap().received,Some(3));
        for raw in [
            br#"{"v":1,"type":"model_setup","session":2,"state":"downloading","received":310414023,"total":310414022}"#.as_slice(),
            br#"{"v":1,"type":"model_setup","session":2,"state":"ready","extra":true}"#,
            br#"{"v":1,"type":"model_setup","session":1,"state":"ready"}"#,
            br#"{"v":1,"type":"model_setup","session":2,"state":"ready","state":"failed","code":"busy"}"#,
            br#"{"v":1,"type":"model_setup","session":2,"state":"ready","\u0073tate":"failed","code":"busy"}"#,
        ] { assert!(parse_line(raw,2).is_none()); }
    }
    #[test]
    fn ipc_distinguishes_clean_eof_from_a_truncated_or_oversized_frame() {
        let mut empty = io::Cursor::new(Vec::<u8>::new());
        assert_eq!(read_line(&mut empty).unwrap(), None);
        let mut partial = io::Cursor::new(b"{\"v\":1}".to_vec());
        assert_eq!(
            read_line(&mut partial).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        let mut too_long = io::Cursor::new(vec![b'x'; MAX_LINE + 1]);
        assert_eq!(
            read_line(&mut too_long).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }
    #[test]
    fn action_fsm_matches_real_precheck_then_download_and_verify_is_offline() {
        assert_eq!(setup_transition("install", 0, "checking_files"), Some(1));
        assert_eq!(setup_transition("install", 1, "downloading"), Some(2));
        assert_eq!(setup_transition("install", 2, "checking_archive"), Some(3));
        assert_eq!(setup_transition("install", 3, "extracting"), Some(4));
        assert_eq!(setup_transition("install", 4, "checking_files"), Some(5));
        assert_eq!(setup_transition("install", 5, "committing"), Some(6));
        assert_eq!(setup_transition("install", 6, "ready"), Some(7));
        assert_eq!(setup_transition("install", 1, "ready"), Some(7));
        assert_eq!(setup_transition("verify", 0, "checking_files"), Some(1));
        assert_eq!(setup_transition("verify", 1, "ready"), Some(2));
        assert_eq!(setup_transition("verify", 0, "downloading"), None);
        assert_eq!(setup_transition("verify", 1, "downloading"), None);
        assert_eq!(setup_transition("install", 0, "ready"), None);
    }
    #[test]
    fn install_challenge_is_random_page_scoped_one_shot_and_expired_grants_prune() {
        let mut m = ModelSetup::default();
        let now = Instant::now();
        let (_, challenge) = m.confirm_install("page", true, now).unwrap();
        assert_eq!(challenge.len(), 32);
        assert!(challenge.bytes().all(|b| b.is_ascii_hexdigit()));
        let deadline = now + Duration::from_secs(3);
        assert!(!m.heartbeat(
            "page",
            "00000000000000000000000000000000",
            now + Duration::from_millis(1),
            deadline,
            0
        ));
        assert!(!m.heartbeat(
            "another-page",
            &challenge,
            now + Duration::from_millis(2),
            deadline,
            0
        ));
        assert!(m.awaiting_heartbeat("page", now + Duration::from_millis(3)));
        assert!(m.heartbeat(
            "page",
            &challenge,
            now + Duration::from_millis(4),
            deadline,
            0
        ));
        assert!(!m.heartbeat(
            "page",
            &challenge,
            now + Duration::from_millis(5),
            deadline,
            0
        ));
        assert!(!m.busy()); // The test has no visible dashboard window; no helper may start.
        let (_, expired) = m
            .confirm_install("page", true, now + Duration::from_millis(6))
            .unwrap();
        assert!(!m.awaiting_heartbeat("page", now + Duration::from_secs(17)));
        assert!(m
            .confirm_install("page", true, now + Duration::from_secs(18))
            .is_some());
        assert!(!m.heartbeat(
            "page",
            &expired,
            now + Duration::from_secs(19),
            now + Duration::from_secs(21),
            0
        ));
    }
    #[test]
    fn ready_evidence_is_not_resurrected_after_view_loss() {
        let mut m = ModelSetup::default();
        m.last = json!({"active":false,"session":1,"state":"ready"});
        m.view_lost();
        assert_eq!(m.state()["state"], "unchecked");
    }
}
