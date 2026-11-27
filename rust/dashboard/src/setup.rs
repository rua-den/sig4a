//! Explicit, per-user local Ollama setup. IPC never accepts paths, arguments, or model names.
use crate::{api, setup_trust as trust};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    ffi::OsStr,
    os::windows::ffi::OsStrExt,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, GetLastError, ERROR_FILE_NOT_FOUND, ERROR_NO_MORE_FILES, ERROR_PATH_NOT_FOUND,
        HANDLE, INVALID_HANDLE_VALUE, WAIT_ABANDONED, WAIT_OBJECT_0, WAIT_TIMEOUT,
    },
    Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY},
    System::{
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
            TH32CS_SNAPPROCESS,
        },
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob,
            JobObjectExtendedLimitInformation, SetInformationJobObject,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        },
        Registry::{
            RegGetValueW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_NOEXPAND,
            RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ,
        },
        SystemInformation::GetSystemDirectoryW,
        Threading::{
            CreateMutexW, CreateProcessW, GetCurrentProcess, GetExitCodeProcess, OpenProcess,
            OpenProcessToken, ReleaseMutex, ResumeThread, TerminateProcess, WaitForSingleObject,
            CREATE_NO_WINDOW, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, PROCESS_INFORMATION,
            PROCESS_QUERY_LIMITED_INFORMATION, STARTUPINFOW,
        },
    },
};

const ENDPOINT: &str = "http://127.0.0.1:11434";
const PRESET: &str = "gemma4:e2b";
const SETUP_LOCK: &str = "Global\\SIG4A.LocalAI.Setup.v1";
const INSTALL_ARGS: &[&str] = &[
    "install",
    "--id",
    "Ollama.Ollama",
    "--exact",
    "--source",
    "winget",
    "--scope",
    "user",
    "--no-upgrade",
    "--disable-interactivity",
    "--silent",
];

type SetupResult<T> = Result<T, Failure>;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Failure {
    code: &'static str,
    exit_code: Option<u32>,
}
impl Failure {
    const fn new(code: &'static str) -> Self {
        Self {
            code,
            exit_code: None,
        }
    }
    const fn exit(code: &'static str, exit: u32) -> Self {
        Self {
            code,
            exit_code: Some(exit),
        }
    }
}

struct Handle(HANDLE);
impl Handle {
    fn new(raw: HANDLE, error: &'static str) -> SetupResult<Self> {
        if raw.is_null() {
            Err(Failure::new(error))
        } else {
            Ok(Self(raw))
        }
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

struct SetupLock(Handle);
impl SetupLock {
    fn try_take() -> SetupResult<Self> {
        Self::try_take_named(SETUP_LOCK)
    }
    fn try_take_named(name: &str) -> SetupResult<Self> {
        let name = wide(OsStr::new(name));
        let handle = Handle::new(
            unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) },
            "lock_unavailable",
        )?;
        match unsafe { WaitForSingleObject(handle.0, 0) } {
            WAIT_OBJECT_0 | WAIT_ABANDONED => Ok(Self(handle)),
            WAIT_TIMEOUT => Err(Failure::new("busy")),
            _ => Err(Failure::new("lock_unavailable")),
        }
    }
}
impl Drop for SetupLock {
    fn drop(&mut self) {
        unsafe {
            ReleaseMutex(self.0 .0);
        }
    }
}

#[derive(Clone, Copy)]
struct Progress {
    phase: &'static str,
    active: bool,
    owned: bool,
    started_server: bool,
    failure: Option<Failure>,
}
static STATE: Mutex<Progress> = Mutex::new(Progress {
    phase: "idle",
    active: false,
    owned: false,
    started_server: false,
    failure: None,
});
static CANCEL: AtomicBool = AtomicBool::new(false);

#[derive(Debug, PartialEq, Eq)]
pub enum ExportStage {
    NotObserved,
    Working,
    Ready,
    Cancelled,
    Failed,
    Unknown,
}
impl ExportStage {
    pub const fn label(&self) -> &'static str {
        match self {
            Self::NotObserved => "not_observed",
            Self::Working => "working",
            Self::Ready => "ready",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ExportIssue {
    None,
    Unavailable,
    ProviderConflict,
    BlockedPreflight,
    Timeout,
    OperationFailed,
    Unknown,
}
impl ExportIssue {
    pub const fn label(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Unavailable => "unavailable",
            Self::ProviderConflict => "provider_conflict",
            Self::BlockedPreflight => "blocked_preflight",
            Self::Timeout => "timeout",
            Self::OperationFailed => "operation_failed",
            Self::Unknown => "unknown",
        }
    }
}

pub struct ExportHistory {
    pub stage: ExportStage,
    pub issue: ExportIssue,
}

fn project_history(p: Progress) -> ExportHistory {
    use ExportIssue as I;
    use ExportStage as S;
    let stage = match p.phase {
        "idle" => S::NotObserved,
        "checking" | "installing" | "starting_server" | "checking_model" | "pulling" => S::Working,
        "ready" => S::Ready,
        "cancelled" => S::Cancelled,
        "failed" => S::Failed,
        _ => S::Unknown,
    };
    let issue = match stage {
        S::Unknown => I::Unknown,
        S::Failed => match p.failure.map(|f| f.code) {
            Some(
                "winget_unavailable"
                | "ollama_missing"
                | "server_unavailable"
                | "worker_unavailable"
                | "lock_unavailable"
                | "job_unavailable"
                | "system_directory_unavailable",
            ) => I::Unavailable,
            Some("port_conflict" | "model_conflict" | "busy") => I::ProviderConflict,
            Some(
                "installer_processes_active"
                | "installer_host_override"
                | "installer_host_unverified"
                | "process_scan_failed"
                | "untrusted_ollama"
                | "untrusted_executable"
                | "elevated"
                | "elevation_unknown",
            ) => I::BlockedPreflight,
            Some("install_timeout" | "server_timeout" | "pull_timeout") => I::Timeout,
            Some(
                "install_failed"
                | "server_failed"
                | "pull_failed"
                | "model_not_found"
                | "model_check_failed"
                | "process_start_failed"
                | "process_status_failed",
            ) => I::OperationFailed,
            _ => I::Unknown,
        },
        _ => I::None,
    };
    ExportHistory { stage, issue }
}

/// This is history for *this* process, not a health check. No OS mutex or
/// external state; contention or a poisoned lock yields unknown immediately.
pub fn export_history() -> ExportHistory {
    STATE
        .try_lock()
        .map(|p| project_history(*p))
        .unwrap_or(ExportHistory {
            stage: ExportStage::Unknown,
            issue: ExportIssue::Unknown,
        })
}

pub fn status() -> Value {
    let p = *STATE.lock().unwrap();
    let externally_busy = !p.active && matches!(SetupLock::try_take(), Err(f) if f.code == "busy");
    let phase = if externally_busy {
        "external_busy"
    } else {
        p.phase
    };
    json!({"phase":phase,"active":p.active || externally_busy,"can_cancel":p.active && p.owned,
        "started_server":p.started_server,"code":p.failure.map(|f|f.code),
        "exit_code":p.failure.and_then(|f|f.exit_code)})
}

pub fn start() -> Value {
    {
        let mut p = STATE.lock().unwrap();
        if p.active {
            return json!({"accepted":false,"code":"busy"});
        }
        CANCEL.store(false, Ordering::Release);
        *p = Progress {
            phase: "checking",
            active: true,
            owned: false,
            started_server: false,
            failure: None,
        };
    }
    if std::thread::Builder::new()
        .name("sig4a-ollama-setup".into())
        .spawn(|| {
            let outcome = (|| {
                let _lock = SetupLock::try_take()?;
                require_standard_user()?;
                STATE.lock().unwrap().owned = true;
                let mut backend = WindowsSetup::default();
                install_missing(&mut backend, |phase| STATE.lock().unwrap().phase = phase)
            })();
            let mut p = STATE.lock().unwrap();
            p.active = false;
            p.owned = false;
            match outcome {
                Ok(started) => {
                    p.phase = "ready";
                    p.started_server = started;
                }
                Err(failure) => {
                    p.phase = if failure.code == "cancelled" {
                        "cancelled"
                    } else {
                        "failed"
                    };
                    p.failure = Some(failure);
                }
            }
        })
        .is_err()
    {
        let mut p = STATE.lock().unwrap();
        p.active = false;
        p.phase = "failed";
        p.failure = Some(Failure::new("worker_unavailable"));
        return json!({"accepted":false,"code":"worker_unavailable"});
    }
    json!({"accepted":true})
}

pub fn cancel() -> Value {
    let p = STATE.lock().unwrap();
    let accepted = p.active && p.owned;
    if accepted {
        CANCEL.store(true, Ordering::Release);
    }
    json!({"accepted":accepted})
}

fn require_standard_user() -> SetupResult<()> {
    let mut token = std::ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(Failure::new("elevation_unknown"));
    }
    let token = Handle::new(token, "elevation_unknown")?;
    let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
    let mut returned = 0;
    if unsafe {
        GetTokenInformation(
            token.0,
            TokenElevation,
            (&mut elevation as *mut TOKEN_ELEVATION).cast(),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        )
    } == 0
        || returned != std::mem::size_of::<TOKEN_ELEVATION>() as u32
    {
        return Err(Failure::new("elevation_unknown"));
    }
    if elevation.TokenIsElevated != 0 {
        return Err(Failure::new("elevated"));
    }
    Ok(())
}

fn check_cancel() -> SetupResult<()> {
    if CANCEL.load(Ordering::Acquire) {
        Err(Failure::new("cancelled"))
    } else {
        Ok(())
    }
}

trait Runner {
    fn installed(&mut self) -> SetupResult<bool>;
    fn install(&mut self) -> SetupResult<()>;
    fn ensure_server(&mut self) -> SetupResult<()>;
    fn model_available(&mut self) -> SetupResult<bool>;
    fn pull(&mut self) -> SetupResult<()>;
    fn finish(&mut self) -> SetupResult<bool>;
}

fn install_missing(
    runner: &mut impl Runner,
    mut report: impl FnMut(&'static str),
) -> SetupResult<bool> {
    if !runner.installed()? {
        report("installing");
        runner.install()?;
    }
    report("starting_server");
    runner.ensure_server()?;
    report("checking_model");
    if !runner.model_available()? {
        report("pulling");
        runner.pull()?;
        report("checking_model");
        if !runner.model_available()? {
            return Err(Failure::new("model_not_found"));
        }
    }
    runner.finish()
}

#[derive(Default)]
struct WindowsSetup {
    ollama: Option<trust::VerifiedOllama>,
    owner: Option<trust::ListenerOwner>,
    server: Option<OwnedProcess>,
    installer: Option<OwnedProcess>,
}

impl WindowsSetup {
    fn find_executable(&mut self) -> SetupResult<bool> {
        self.ollama = trust::resolve_ollama().map_err(|_| Failure::new("untrusted_ollama"))?;
        Ok(self.ollama.is_some())
    }

    fn executable(&self) -> SetupResult<&trust::VerifiedOllama> {
        self.ollama.as_ref().ok_or(Failure::new("ollama_missing"))
    }

    fn checked_owner(&self) -> SetupResult<Option<trust::ListenerOwner>> {
        let owner = trust::loopback_listener_owner().map_err(|_| Failure::new("port_conflict"))?;
        if let Some(owner) = owner {
            if !self
                .ollama
                .as_ref()
                .is_some_and(|verified| owner.matches(verified))
            {
                return Err(Failure::new("port_conflict"));
            }
            if self
                .owner
                .as_ref()
                .is_some_and(|prior| prior.pid() != owner.pid())
            {
                return Err(Failure::new("port_conflict"));
            }
            Ok(Some(owner))
        } else {
            Ok(None)
        }
    }

    fn confirm_listener(&self) -> SetupResult<()> {
        check_cancel()?;
        if self.checked_owner()?.is_none() {
            return Err(Failure::new("server_unavailable"));
        }
        Ok(())
    }
}

impl Runner for WindowsSetup {
    fn installed(&mut self) -> SetupResult<bool> {
        check_cancel()?;
        let installed = self.find_executable()?;
        self.owner = self.checked_owner()?;
        Ok(installed)
    }

    fn install(&mut self) -> SetupResult<()> {
        check_cancel()?;
        let winget = trust::resolve_winget().map_err(|_| Failure::new("winget_unavailable"))?;
        let installer = OwnedProcess::spawn(&winget, INSTALL_ARGS, LaunchKind::WinGet)?;
        installer.wait(
            Duration::from_secs(10 * 60),
            "install_timeout",
            "install_failed",
        )?;
        // The official installer can start the verified Ollama tray/server. Do
        // not tear down its job until a proven listener and preset are ready.
        self.installer = Some(installer);
        check_cancel()?;
        if !self.find_executable()? {
            return Err(Failure::new("ollama_missing"));
        }
        Ok(())
    }

    fn ensure_server(&mut self) -> SetupResult<()> {
        check_cancel()?;
        if let Some(owner) = self.checked_owner()? {
            self.owner = Some(owner);
            return Ok(()); // Never stop, rebind, or claim cloud settings for this daemon.
        }
        if self.owner.is_some() {
            return Err(Failure::new("server_unavailable"));
        }
        let executable = self.executable()?;
        self.server = Some(OwnedProcess::spawn(
            executable.path(),
            &["serve"],
            LaunchKind::Ollama,
        )?);
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            check_cancel()?;
            if let Some(owner) = self.checked_owner()? {
                // The official installer can start a verified daemon concurrently.
                // If it won the port, close only our redundant job and reuse it.
                if self
                    .server
                    .as_ref()
                    .is_some_and(|server| server.pid != owner.pid())
                {
                    self.server.take();
                }
                self.owner = Some(owner);
                return Ok(());
            }
            if let Some(exit) = self.server.as_ref().unwrap().exit_code(Duration::ZERO)? {
                return Err(Failure::exit("server_failed", exit));
            }
            if Instant::now() >= deadline {
                return Err(Failure::new("server_timeout"));
            }
            std::thread::sleep(Duration::from_millis(150));
        }
    }

    fn model_available(&mut self) -> SetupResult<bool> {
        self.confirm_listener()?;
        // Distinguish a missing tag from an excluded/remote alias so we never overwrite the latter.
        let state = api::ollama_setup_model(ENDPOINT, PRESET);
        self.confirm_listener()?;
        match state {
            Ok(api::SetupModel::Missing) => Ok(false),
            Ok(api::SetupModel::LocalVision) => Ok(true),
            Ok(api::SetupModel::Conflicting) => Err(Failure::new("model_conflict")),
            Err(_) => Err(Failure::new("model_check_failed")),
        }
    }

    fn pull(&mut self) -> SetupResult<()> {
        self.confirm_listener()?;
        OwnedProcess::spawn(
            self.executable()?.path(),
            &["pull", PRESET],
            LaunchKind::Ollama,
        )?
        .wait(Duration::from_secs(20 * 60), "pull_timeout", "pull_failed")?;
        self.confirm_listener()
    }

    fn finish(&mut self) -> SetupResult<bool> {
        self.confirm_listener()?;
        // Linearize cancellation with the point at which we detach our daemon.
        {
            let mut state = STATE.lock().unwrap();
            check_cancel()?;
            state.owned = false;
        }
        if let Some(server) = &self.server {
            // Keep the newly verified server for the user's later Local AI requests.
            server.keep_running()?;
            Ok(true)
        } else {
            if let (Some(installer), Some(owner)) = (&self.installer, &self.owner) {
                if installer.contains_process(owner.pid())? {
                    // Keep the installer's verified daemon, without asserting its cloud policy.
                    installer.keep_running()?;
                }
            }
            Ok(false)
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LaunchKind {
    WinGet,
    Ollama,
}

fn installer_targets_process(image: &[u16]) -> bool {
    let image = image.split(|&unit| unit == 0).next().unwrap_or_default();
    // Mirror the current upstream OllamaSetup image names, not paths or partial matches.
    ["ollama.exe", "ollama app.exe", "llama-server.exe"]
        .iter()
        .any(|name| {
            name.len() == image.len()
                && name.bytes().zip(image).all(|(expected, &actual)| {
                    u8::try_from(actual).is_ok_and(|actual| expected.eq_ignore_ascii_case(&actual))
                })
        })
}

fn installer_host_is_default(host: &[u16]) -> bool {
    host.is_empty()
        || ["127.0.0.1:11434", "http://127.0.0.1:11434"]
            .iter()
            .any(|allowed| host.iter().copied().eq(allowed.encode_utf16()))
}

fn check_installer_host_value(root: HKEY, subkey: &str) -> SetupResult<()> {
    let key = wide(OsStr::new(subkey));
    let name = wide(OsStr::new("OLLAMA_HOST"));
    let mut value = [0u16; 512];
    let mut bytes = std::mem::size_of_val(&value) as u32;
    let result = unsafe {
        RegGetValueW(
            root,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ | RRF_NOEXPAND,
            std::ptr::null_mut(),
            value.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    match result {
        ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => Ok(()),
        0 if bytes >= 2 && bytes % 2 == 0 && bytes as usize <= std::mem::size_of_val(&value) => {
            let data = &value[..bytes as usize / 2];
            if data
                .strip_suffix(&[0])
                .is_some_and(installer_host_is_default)
            {
                Ok(())
            } else {
                Err(Failure::new("installer_host_override"))
            }
        }
        _ => Err(Failure::new("installer_host_unverified")),
    }
}

fn ensure_safe_installer_host() -> SetupResult<()> {
    if let Some(value) = std::env::var_os("OLLAMA_HOST") {
        let value: Vec<u16> = value.encode_wide().collect();
        if !installer_host_is_default(&value) {
            return Err(Failure::new("installer_host_override"));
        }
    }
    // Windows/package activation may construct a fresh environment unrelated to
    // WinGet's child environment. Be conservative about persisted host overrides.
    check_installer_host_value(HKEY_CURRENT_USER, "Environment")?;
    check_installer_host_value(HKEY_CURRENT_USER, "Volatile Environment")?;
    check_installer_host_value(
        HKEY_LOCAL_MACHINE,
        r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment",
    )
}

fn ensure_no_installer_targets() -> SetupResult<()> {
    // A process snapshot is system-wide, not restricted to this session or to the
    // default port. Never open a worker with terminate rights or call taskkill.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE || snapshot.is_null() {
        return Err(Failure::new("process_scan_failed"));
    }
    let snapshot = Handle(snapshot);
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    if unsafe { Process32FirstW(snapshot.0, &mut entry) } == 0 {
        return Err(Failure::new("process_scan_failed"));
    }
    loop {
        if installer_targets_process(&entry.szExeFile) {
            return Err(Failure::new("installer_processes_active"));
        }
        if unsafe { Process32NextW(snapshot.0, &mut entry) } == 0 {
            return if unsafe { GetLastError() } == ERROR_NO_MORE_FILES {
                Ok(())
            } else {
                Err(Failure::new("process_scan_failed"))
            };
        }
    }
}

struct OwnedProcess {
    process: Handle,
    job: Handle,
    pid: u32,
}
impl OwnedProcess {
    fn spawn(path: &Path, args: &[&str], kind: LaunchKind) -> SetupResult<Self> {
        if !path.is_absolute() {
            return Err(Failure::new("untrusted_executable"));
        }
        let executable = wide(path.as_os_str());
        let mut command_line = vec![b'"' as u16];
        command_line.extend_from_slice(&executable[..executable.len() - 1]);
        command_line.push(b'"' as u16);
        for arg in args {
            // All args are fixed literals without whitespace or shell/Windows quoting syntax.
            debug_assert!(arg.is_ascii() && !arg.contains([' ', '"', '\\']));
            command_line.push(b' ' as u16);
            command_line.extend(arg.encode_utf16());
        }
        command_line.push(0);

        let mut system_dir = [0u16; 32768];
        let len = unsafe { GetSystemDirectoryW(system_dir.as_mut_ptr(), system_dir.len() as u32) }
            as usize;
        if len == 0 || len >= system_dir.len() {
            return Err(Failure::new("system_directory_unavailable"));
        }
        let env = restricted_environment(&system_dir[..len]);

        let job = Handle::new(
            unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) },
            "job_unavailable",
        )?;
        set_job_limit(&job, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE)?;
        let mut startup: STARTUPINFOW = unsafe { std::mem::zeroed() };
        startup.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        let mut process: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
        let ok = unsafe {
            CreateProcessW(
                executable.as_ptr(),
                command_line.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                CREATE_NO_WINDOW | CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT,
                env.as_ptr().cast(),
                system_dir.as_ptr(),
                &startup,
                &mut process,
            )
        };
        if ok == 0 {
            return Err(Failure::exit(
                "process_start_failed",
                std::io::Error::last_os_error().raw_os_error().unwrap_or(0) as u32,
            ));
        }
        let child = Handle(process.hProcess);
        let thread = Handle(process.hThread);
        // No child code runs until the kill-on-close job is in place.
        if unsafe { AssignProcessToJobObject(job.0, child.0) } == 0 {
            unsafe {
                TerminateProcess(child.0, 1);
            }
            return Err(Failure::new("job_unavailable"));
        }
        // With WinGet still suspended in our kill-on-close job, this is the last
        // boundary before its vendor installer can run. It is a snapshot: we cannot
        // prevent someone from starting a targeted worker after it passes.
        if kind == LaunchKind::WinGet {
            ensure_safe_installer_host()?;
            ensure_no_installer_targets()?;
        }
        check_cancel()?;
        if unsafe { ResumeThread(thread.0) } == u32::MAX {
            return Err(Failure::new("process_start_failed"));
        }
        Ok(Self {
            process: child,
            job,
            pid: process.dwProcessId,
        })
    }

    fn exit_code(&self, wait: Duration) -> SetupResult<Option<u32>> {
        match unsafe { WaitForSingleObject(self.process.0, wait.as_millis() as u32) } {
            WAIT_TIMEOUT => Ok(None),
            WAIT_OBJECT_0 => {
                let mut exit = 0;
                if unsafe { GetExitCodeProcess(self.process.0, &mut exit) } == 0 {
                    Err(Failure::new("process_status_failed"))
                } else {
                    Ok(Some(exit))
                }
            }
            _ => Err(Failure::new("process_status_failed")),
        }
    }

    fn wait(
        &self,
        timeout: Duration,
        timeout_code: &'static str,
        failure_code: &'static str,
    ) -> SetupResult<()> {
        let deadline = Instant::now() + timeout;
        loop {
            check_cancel()?;
            match self.exit_code(Duration::from_millis(150))? {
                Some(0) => return Ok(()),
                Some(exit) => return Err(Failure::exit(failure_code, exit)),
                None if Instant::now() >= deadline => return Err(Failure::new(timeout_code)),
                None => (),
            }
        }
    }

    fn contains_process(&self, pid: u32) -> SetupResult<bool> {
        // The caller retains the previously verified ListenerOwner handle as well.
        let child = Handle::new(
            unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) },
            "process_status_failed",
        )?;
        let mut in_job = 0;
        if unsafe { IsProcessInJob(child.0, self.job.0, &mut in_job) } == 0 {
            Err(Failure::new("process_status_failed"))
        } else {
            Ok(in_job != 0)
        }
    }

    fn keep_running(&self) -> SetupResult<()> {
        set_job_limit(&self.job, 0)
    }
}

fn set_job_limit(job: &Handle, flags: u32) -> SetupResult<()> {
    let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    info.BasicLimitInformation.LimitFlags = flags;
    if unsafe {
        SetInformationJobObject(
            job.0,
            JobObjectExtendedLimitInformation,
            (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            std::mem::size_of_val(&info) as u32,
        )
    } == 0
    {
        Err(Failure::new("job_unavailable"))
    } else {
        Ok(())
    }
}

fn restricted_environment(system_dir: &[u16]) -> Vec<u16> {
    let mut env = BTreeMap::<Vec<u16>, Vec<u16>>::new();
    // Keys known to steer execution or network/config selection are explicitly replaced/removed.
    for (k, v) in std::env::vars_os() {
        let key = k.to_string_lossy().to_ascii_uppercase();
        if matches!(
            key.as_str(),
            "PATH" | "OLLAMA_HOST" | "OLLAMA_NO_CLOUD" | "OLLAMA_API_KEY"
        ) || key.starts_with('=')
        {
            continue;
        }
        env.insert(key.encode_utf16().collect(), v.encode_wide().collect());
    }
    env.insert("PATH".encode_utf16().collect(), system_dir.to_vec());
    {
        // WinGet descendants can inherit these, but package activation may not.
        // The UI only attests to the environment of our directly started daemon.
        env.insert(
            "OLLAMA_HOST".encode_utf16().collect(),
            "127.0.0.1:11434".encode_utf16().collect(),
        );
        env.insert(
            "OLLAMA_NO_CLOUD".encode_utf16().collect(),
            "1".encode_utf16().collect(),
        );
    }
    let mut block = Vec::new();
    for (key, value) in env {
        block.extend(key);
        block.push(b'=' as u16);
        block.extend(value);
        block.push(0);
    }
    block.push(0);
    block
}

fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(Some(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Fake {
        calls: Vec<&'static str>,
        found: bool,
        have_model: bool,
        install_error: Option<Failure>,
        listener_error: Option<Failure>,
        pull_error: Option<Failure>,
    }
    impl Runner for Fake {
        fn installed(&mut self) -> SetupResult<bool> {
            self.calls.push("installed");
            Ok(self.found)
        }
        fn install(&mut self) -> SetupResult<()> {
            self.calls.push("install");
            self.install_error.map_or(Ok(()), Err)
        }
        fn ensure_server(&mut self) -> SetupResult<()> {
            self.calls.push("ensure_server");
            self.listener_error.map_or(Ok(()), Err)
        }
        fn model_available(&mut self) -> SetupResult<bool> {
            self.calls.push("model_available");
            Ok(self.have_model)
        }
        fn pull(&mut self) -> SetupResult<()> {
            self.calls.push("pull");
            if let Some(error) = self.pull_error {
                return Err(error);
            }
            self.have_model = true;
            Ok(())
        }
        fn finish(&mut self) -> SetupResult<bool> {
            self.calls.push("finish");
            Ok(false)
        }
    }
    #[test]
    fn setup_installs_only_missing_components() {
        let mut fake = Fake::default();
        let mut phases = vec![];
        assert_eq!(
            install_missing(&mut fake, |phase| phases.push(phase)),
            Ok(false)
        );
        assert_eq!(
            fake.calls,
            [
                "installed",
                "install",
                "ensure_server",
                "model_available",
                "pull",
                "model_available",
                "finish"
            ]
        );
        assert_eq!(
            phases,
            [
                "installing",
                "starting_server",
                "checking_model",
                "pulling",
                "checking_model"
            ]
        );
        let mut fake = Fake {
            found: true,
            have_model: true,
            ..Default::default()
        };
        assert_eq!(install_missing(&mut fake, |_| {}), Ok(false));
        assert_eq!(
            fake.calls,
            ["installed", "ensure_server", "model_available", "finish"]
        );
    }
    #[test]
    fn setup_stops_after_conflict_install_failure_or_timeout() {
        let error = Failure::exit("install_failed", 12);
        let mut fake = Fake {
            install_error: Some(error),
            ..Default::default()
        };
        assert_eq!(install_missing(&mut fake, |_| {}), Err(error));
        assert_eq!(fake.calls, ["installed", "install"]);
        let error = Failure::new("port_conflict");
        let mut fake = Fake {
            found: true,
            listener_error: Some(error),
            ..Default::default()
        };
        assert_eq!(install_missing(&mut fake, |_| {}), Err(error));
        assert_eq!(fake.calls, ["installed", "ensure_server"]);
        let error = Failure::new("pull_timeout");
        let mut fake = Fake {
            found: true,
            pull_error: Some(error),
            ..Default::default()
        };
        assert_eq!(install_missing(&mut fake, |_| {}), Err(error));
        assert_eq!(
            fake.calls,
            ["installed", "ensure_server", "model_available", "pull"]
        );
    }
    #[test]
    fn installer_preflight_matches_all_upstream_targets_without_false_prefixes() {
        for image in ["ollama.exe", "OLLAMA APP.EXE", "Llama-Server.ExE"] {
            let win_image: Vec<u16> = image.encode_utf16().chain([0, b'x' as u16]).collect();
            assert!(
                installer_targets_process(&win_image),
                "should block {image}"
            );
        }
        for image in [
            "",
            "winget.exe",
            "ollama-runner.exe",
            "ollama.exe.old",
            "my-llama-server.exe",
            "ollamá.exe",
        ] {
            let win_image: Vec<u16> = image.encode_utf16().chain([0]).collect();
            assert!(
                !installer_targets_process(&win_image),
                "should not block {image}"
            );
        }
    }

    #[test]
    fn installer_host_allows_only_unset_or_exact_loopback_default() {
        for host in ["", "127.0.0.1:11434", "http://127.0.0.1:11434"] {
            assert!(installer_host_is_default(
                &host.encode_utf16().collect::<Vec<_>>()
            ));
        }
        for host in [
            "0.0.0.0:11434",
            "127.0.0.1:12345",
            "[::]:11434",
            "localhost:11434",
            "%MY_HOST%",
            "127.0.0.1:11434\0other",
        ] {
            assert!(!installer_host_is_default(
                &host.encode_utf16().collect::<Vec<_>>()
            ));
        }
    }

    #[test]
    fn report_history_maps_fixed_setup_classes() {
        for (phase, failure, stage, issue) in [
            ("idle", None, ExportStage::NotObserved, ExportIssue::None),
            ("checking", None, ExportStage::Working, ExportIssue::None),
            ("installing", None, ExportStage::Working, ExportIssue::None),
            (
                "starting_server",
                None,
                ExportStage::Working,
                ExportIssue::None,
            ),
            (
                "checking_model",
                None,
                ExportStage::Working,
                ExportIssue::None,
            ),
            ("pulling", None, ExportStage::Working, ExportIssue::None),
            ("ready", None, ExportStage::Ready, ExportIssue::None),
            (
                "cancelled",
                Some(Failure::new("cancelled")),
                ExportStage::Cancelled,
                ExportIssue::None,
            ),
            (
                "failed",
                Some(Failure::new("port_conflict")),
                ExportStage::Failed,
                ExportIssue::ProviderConflict,
            ),
            (
                "failed",
                Some(Failure::new("pull_timeout")),
                ExportStage::Failed,
                ExportIssue::Timeout,
            ),
            (
                "failed",
                Some(Failure::new("winget_unavailable")),
                ExportStage::Failed,
                ExportIssue::Unavailable,
            ),
            (
                "failed",
                Some(Failure::exit("install_failed", 42)),
                ExportStage::Failed,
                ExportIssue::OperationFailed,
            ),
        ] {
            let projected = project_history(Progress {
                phase,
                active: false,
                owned: false,
                started_server: false,
                failure,
            });
            assert_eq!(projected.stage, stage);
            assert_eq!(projected.issue, issue);
        }
    }

    #[test]
    fn report_history_never_exports_raw_failure_or_waits_for_lock() {
        let mut raw = Progress {
            phase: "failed",
            active: false,
            owned: false,
            started_server: false,
            failure: Some(Failure::exit("https://private.invalid/CANARY", u32::MAX)),
        };
        let history = project_history(raw);
        assert_eq!(history.stage, ExportStage::Failed);
        assert_eq!(history.issue, ExportIssue::Unknown);
        raw.failure = Some(Failure::new("installer_host_override"));
        assert_eq!(project_history(raw).issue, ExportIssue::BlockedPreflight);
        raw.phase = "C:\\secret\\CANARY";
        assert_eq!(project_history(raw).stage, ExportStage::Unknown);
        let _held = STATE.lock().unwrap();
        assert_eq!(export_history().stage, ExportStage::Unknown);
    }

    #[test]
    fn named_lock_excludes_other_workers_until_owner_drops_it() {
        // Test-specific kernel object: never holds the mutex used by real setup.
        let name = "Local\\SIG4A.LocalAI.Setup.test";
        let owner = SetupLock::try_take_named(name)
            .ok()
            .expect("lock acquisition");
        assert!(std::thread::spawn(
            move || matches!(SetupLock::try_take_named(name), Err(f) if f.code == "busy")
        )
        .join()
        .unwrap());
        drop(owner);
        assert!(
            std::thread::spawn(move || SetupLock::try_take_named(name).is_ok())
                .join()
                .unwrap()
        );
    }
}
