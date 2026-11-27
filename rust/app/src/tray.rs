//! Tray process: hidden top-level window + notification-area icon + context menu + balloon toasts, with the
//! daemon loop on a worker thread (a slow HTTP call never freezes the menu). Port of the UI half of `app.py`.
//!
//! Threading: the UI thread owns every Win32 object. The worker only posts messages (`WM_*`) and
//! hands data over through [`Shared`] mutexes.

use crate::daemon::{self, Daemon, RealEnv};
use crate::icon::{self, IconSpec};
use crate::transport::Ureq;
use cu_core::config;
use cu_core::daemon::{Toast, TrayView};
use cu_core::hookinstall::{self, HookProfile};
use cu_core::hotkeys::{self, Bindings, Chord};
use cu_core::log::log;
use cu_core::paths::{home_dir, Paths};
use serde_json::Value;
use std::collections::VecDeque;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    CreateBitmap, CreateDIBSection, DeleteObject, GetDC, ReleaseDC, BITMAPINFO, BITMAPINFOHEADER,
    BI_RGB, DIB_RGB_COLORS,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::DataExchange::COPYDATASTRUCT;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, MOD_NOREPEAT,
};
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE,
    NIM_MODIFY, NOTIFYICONDATAW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

pub const CLASS: &str = hotkeys::TRAY_CLASS;
pub const TITLE: &str = "claude-usage-tray";

const WM_TRAY: u32 = WM_APP + 10;
const WM_ICON: u32 = WM_APP + 1;
const WM_TOAST: u32 = WM_APP + 2;
const WM_SHOW: u32 = hotkeys::OPEN_MESSAGE;
const HOTKEY_ID_FIRST: i32 = 100;
const HOTKEY_ID_END: i32 = HOTKEY_ID_FIRST + 8;
const MAX_SELECTION_BYTES: usize = 12 * 1024;

pub const ID_OPEN: usize = 1;
pub const ID_REFRESH: usize = 2;
pub const ID_WIDGET: usize = 3;
pub const ID_NOTIFY: usize = 4;
pub const ID_TEST: usize = 5;
pub const ID_QUIT: usize = 6;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// Copy `s` into a fixed UTF-16 buffer, always NUL-terminated (truncates; never splits a surrogate pair).
fn fill(dst: &mut [u16], s: &str) {
    let mut n = 0;
    for ch in s.chars() {
        let mut b = [0u16; 2];
        let u = ch.encode_utf16(&mut b);
        if n + u.len() >= dst.len() {
            break;
        }
        dst[n..n + u.len()].copy_from_slice(u);
        n += u.len();
    }
    dst[n] = 0;
}

#[derive(Default)]
struct Shared {
    view: Mutex<Option<TrayView>>,
    toasts: Mutex<VecDeque<Toast>>,
    quick_busy: AtomicBool,
    // Do not start another UIA call while a provider is still blocked.
    quick_selection_busy: Arc<AtomicBool>,
}

fn selection_before_deadline(
    busy: &Arc<AtomicBool>,
    timeout: Duration,
    lookup: impl FnOnce() -> Option<String> + Send + 'static,
) -> Option<String> {
    if busy.swap(true, Ordering::AcqRel) {
        return None;
    }
    struct Guard(Arc<AtomicBool>);
    impl Drop for Guard {
        fn drop(&mut self) {
            self.0.store(false, Ordering::Release);
        }
    }
    let guard = Guard(busy.clone());
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    // Guard is dropped even if OS thread creation fails or the lookup panics.
    std::thread::Builder::new()
        .name("quick-selection".into())
        .spawn(move || {
            let result = lookup();
            drop(guard);
            let _ = tx.send(result);
        })
        .ok()?;
    rx.recv_timeout(timeout).ok().flatten()
}

struct State {
    paths: Paths,
    shared: Arc<Shared>,
    hicon: isize,
    taskbar_created: u32,
    hotkeys: [Option<Registered>; 4],
    /// WinRT toast with our own AUMID (release only); false = tray balloon.
    winrt: bool,
}

/// The optional managed Local AI child. It is deliberately owned by the tray: the job object
/// cleans it up with the tray, while disable only terminates this exact child.
struct ManagedLlama {
    child: Option<std::process::Child>,
    enabled: bool,
    failed: bool,
}

impl ManagedLlama {
    fn new() -> Self {
        Self {
            child: None,
            enabled: false,
            failed: false,
        }
    }

    fn tick(&mut self, paths: &Paths) {
        let cfg = config::load_config(paths, &home_dir());
        let enabled = cfg
            .get("local_ai_managed")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !enabled {
            if self.enabled {
                self.stop(paths);
            }
            self.enabled = false;
            self.failed = false;
            return;
        }
        if !self.enabled {
            self.enabled = true;
            self.failed = false;
            self.start(paths);
        }
        if let Some(child) = self.child.as_mut() {
            match child.try_wait() {
                Ok(Some(status)) => {
                    log(
                        paths,
                        &format!(
                            "managed llama-server exited ({status}); retry disabled until toggled"
                        ),
                    );
                    self.child = None;
                    self.failed = true;
                }
                Ok(None) => {}
                Err(e) => {
                    log(paths, &format!("managed llama-server status failed: {e}"));
                    self.failed = true;
                }
            }
        }
    }

    fn start(&mut self, paths: &Paths) {
        if self.failed || self.child.is_some() {
            return;
        }
        use std::os::windows::process::CommandExt;
        let sibling = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("llama-server.exe")));
        let program = sibling
            .filter(|p| p.exists())
            .unwrap_or_else(|| std::path::PathBuf::from("llama-server.exe"));
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let result = std::process::Command::new(&program)
            .args([
                "--host",
                "127.0.0.1",
                "--port",
                "18080",
                "-hf",
                "LiquidAI/LFM2.5-VL-450M-GGUF:Q8_0",
                "--alias",
                "lfm25-vl-q8",
                "-c",
                "4096",
                "-np",
                "1",
                "-ngl",
                "0",
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
        match result {
            Ok(child) => {
                log(
                    paths,
                    "managed llama-server started (first startup may download 482 MB)",
                );
                bind_to_job(&child);
                self.child = Some(child);
            }
            Err(e) => {
                log(
                    paths,
                    &format!(
                        "managed llama-server unavailable ({e}); retry disabled until toggled"
                    ),
                );
                self.failed = true;
            }
        }
    }

    fn stop(&mut self, paths: &Paths) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
            log(paths, "managed llama-server stopped");
        }
    }
}

fn nid(hwnd: HWND) -> NOTIFYICONDATAW {
    // SAFETY: NOTIFYICONDATAW is plain data; all-zero is a valid empty value.
    let mut n: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    n.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    n.hWnd = hwnd;
    n.uID = 1;
    n
}

/// 64x64 premultiplied BGRA -> HICON (0 on failure).
fn make_hicon(bgra: &[u8]) -> isize {
    // SAFETY: standard DIB-section + CreateIconIndirect sequence; every GDI object is released.
    unsafe {
        let mut bi: BITMAPINFO = std::mem::zeroed();
        bi.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: icon::SIZE as i32,
            biHeight: -(icon::SIZE as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..std::mem::zeroed()
        };
        let mut bits: *mut core::ffi::c_void = null_mut();
        let dc = GetDC(null_mut());
        let color = CreateDIBSection(dc, &bi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        ReleaseDC(null_mut(), dc);
        if color.is_null() || bits.is_null() {
            return 0;
        }
        std::ptr::copy_nonoverlapping(bgra.as_ptr(), bits as *mut u8, bgra.len());
        let mask = CreateBitmap(icon::SIZE as i32, icon::SIZE as i32, 1, 1, null());
        let info = ICONINFO {
            fIcon: 1,
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: color,
        };
        let h = CreateIconIndirect(&info);
        DeleteObject(color);
        DeleteObject(mask);
        h as isize
    }
}

fn add_or_update(hwnd: HWND, st: &mut State, add: bool) {
    let view = st.shared.view.lock().ok().and_then(|g| g.clone());
    let (bytes, tip) = match &view {
        Some(v) => (
            icon::render(&IconSpec {
                color: v.color,
                text: &v.text,
                bar_right: v.bar_right,
                busy: v.busy,
            }),
            v.tooltip.clone(),
        ),
        None => (
            icon::render(&IconSpec {
                color: "#6b6b6b",
                text: "?",
                bar_right: None,
                busy: false,
            }),
            "Kane's Tool Box".to_string(),
        ),
    };
    let h = make_hicon(&bytes);
    let mut n = nid(hwnd);
    n.uFlags = NIF_ICON | NIF_TIP | NIF_MESSAGE;
    n.uCallbackMessage = WM_TRAY;
    n.hIcon = h as _;
    fill(&mut n.szTip, &tip);
    // SAFETY: n is fully initialised; the old icon is destroyed after the shell got the new one.
    unsafe {
        Shell_NotifyIconW(if add { NIM_ADD } else { NIM_MODIFY }, &n);
        if st.hicon != 0 {
            DestroyIcon(st.hicon as _);
        }
    }
    st.hicon = h;
}

fn balloon(hwnd: HWND, t: &Toast) {
    let mut n = nid(hwnd);
    n.uFlags = NIF_INFO;
    n.dwInfoFlags = NIIF_INFO;
    fill(&mut n.szInfoTitle, &t.title);
    fill(&mut n.szInfo, &t.body);
    // SAFETY: valid NOTIFYICONDATAW.
    unsafe { Shell_NotifyIconW(NIM_MODIFY, &n) };
}

/// Toast: WinRT (own AUMID) when enabled, otherwise - or when WinRT fails - the tray balloon.
fn notify(hwnd: HWND, st: &mut State, t: &Toast) {
    if st.winrt {
        match catch_unwind(AssertUnwindSafe(|| crate::toast::show(&t.title, &t.body))) {
            Ok(Ok(())) => return,
            Ok(Err(e)) => log(
                &st.paths,
                &format!("winrt toast failed ({e}); using balloon"),
            ),
            Err(_) => log(&st.paths, "winrt toast panicked; using balloon"),
        }
        st.winrt = false; // do not retry a broken path on every toast
    }
    balloon(hwnd, t);
}

fn toggle(paths: &Paths, key: &str) {
    let mut cfg = config::load_config(paths, &home_dir());
    let cur = cfg.get(key).and_then(Value::as_bool).unwrap_or(true);
    cfg.insert(key.into(), Value::Bool(!cur));
    let _ = config::save_config(paths, &cfg);
}

fn cfg_bool(paths: &Paths, key: &str) -> bool {
    config::load_config(paths, &home_dir())
        .get(key)
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn touch(p: &std::path::Path) {
    let _ = std::fs::write(p, "1");
}

/// Execute a menu command (also reachable through WM_COMMAND, which the UI tests use).
fn command(hwnd: HWND, st: &mut State, id: usize) {
    match id {
        ID_OPEN => open_dashboard(&st.paths),
        ID_REFRESH => touch(&st.paths.flag),
        ID_WIDGET => toggle(&st.paths, "widget_enabled"),
        ID_NOTIFY => toggle(&st.paths, "notify_enabled"),
        ID_TEST => touch(&st.paths.testflag),
        ID_QUIT => {
            // SAFETY: valid window handle owned by this thread.
            unsafe { DestroyWindow(hwnd) };
        }
        _ => {}
    }
}

/// Open the dashboard: its own process (`claude-usage-dashboard.exe` next to this exe). A dashboard that is already
/// open focuses itself and the new process exits, so repeated clicks never stack windows. A missing exe or
/// WebView2 is reported with a message instead of a crash (the tray and the hook do not need either).
fn open_dashboard(paths: &Paths) {
    use std::os::windows::process::CommandExt;
    log(paths, "open dashboard requested");
    if std::env::var_os("CU_NO_DASHBOARD").is_some() {
        return; // UI tests of the tray must not open a real window
    }
    let exe = std::env::current_exe()
        .unwrap_or_default()
        .with_file_name("claude-usage-dashboard.exe");
    if !exe.exists() {
        log(paths, "dashboard exe missing");
        message_box(
            "Không tìm thấy claude-usage-dashboard.exe cạnh claude-usage.exe: cài lại bản đầy đủ.",
        );
        return;
    }
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let spawned = std::process::Command::new(&exe)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
    match spawned {
        Ok(child) => {
            // SAFETY: grant only the child a chance to activate its dashboard.
            unsafe { AllowSetForegroundWindow(child.id()); }
            bind_to_job(&child);
        }
        Err(e) => {
            log(paths, &format!("dashboard spawn failed: {e}"));
            message_box("Không mở được dashboard.");
        }
    }
}

/// One process-wide Job Object (KILL_ON_JOB_CLOSE): children assigned to it die when the tray process ends, even
/// when it is killed. The handle is intentionally never closed. Best effort: failure only means no auto-kill.
fn bind_to_job(child: &std::process::Child) {
    use std::os::windows::io::AsRawHandle;
    use std::sync::OnceLock;
    use windows_sys::Win32::System::JobObjects::*;
    static JOB: OnceLock<usize> = OnceLock::new();
    let job = *JOB.get_or_init(|| {
        // SAFETY: creates an unnamed job and sets its limit flags; the handle lives for the whole process.
        unsafe {
            let h = CreateJobObjectW(null(), null());
            if h.is_null() {
                return 0;
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                h,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 {
                0
            } else {
                h as usize
            }
        }
    });
    if job != 0 {
        // SAFETY: valid job and process handles.
        unsafe { AssignProcessToJobObject(job as _, child.as_raw_handle() as _) };
    }
}

fn message_box(text: &str) {
    let (t, c) = (wide(text), wide("Kane's Tool Box"));
    // SAFETY: NUL-terminated strings; runs on a short-lived thread so the tray loop keeps going.
    std::thread::spawn(move || unsafe {
        MessageBoxW(null_mut(), t.as_ptr(), c.as_ptr(), MB_OK | MB_ICONWARNING);
    });
}

#[derive(Clone, Copy)]
struct Registered {
    id: i32,
    chord: Chord,
}

fn register(hwnd: HWND, id: i32, chord: Chord) -> Option<Registered> {
    // SAFETY: invoked by the owning tray thread with a valid HWND and supported combination.
    (unsafe { RegisterHotKey(hwnd, id, chord.mods | MOD_NOREPEAT, chord.vk) } != 0)
        .then_some(Registered { id, chord })
}

fn unregister(hwnd: HWND, slots: impl IntoIterator<Item = Registered>) {
    for slot in slots {
        // SAFETY: IDs originate from successful registrations on this tray thread.
        unsafe { UnregisterHotKey(hwnd, slot.id); }
    }
}

impl State {
    fn shortcut_status(&self) -> usize {
        if std::env::var_os("CU_NO_HOTKEYS").is_some() {
            return hotkeys::DISABLED;
        }
        let settings = hotkeys::configured(&config::load_config(&self.paths, &home_dir()));
        hotkeys::STATUS + settings.iter().enumerate().fold(0, |mask, (i, chord)| {
            if *chord != self.hotkeys[i].map(|s| s.chord) { mask | (1 << i) } else { mask }
        })
    }

    fn apply_shortcuts(&mut self, hwnd: HWND, bindings: Bindings) -> usize {
        if std::env::var_os("CU_NO_HOTKEYS").is_some() {
            return hotkeys::DISABLED;
        }

        // Acquire all new chords before relinquishing any old ones. A chord we already own
        // can move to a different action without registering it, so swaps are atomic too.
        let mut next = [None; 4];
        let mut acquired: Vec<Registered> = Vec::new();
        for (i, chord) in bindings.into_iter().enumerate() {
            let Some(chord) = chord else { continue; };
            if let Some(existing) = self.hotkeys.iter().flatten().find(|s| s.chord == chord) {
                next[i] = Some(*existing);
                continue;
            }
            let id = (HOTKEY_ID_FIRST..HOTKEY_ID_END).find(|id| {
                !self.hotkeys.iter().flatten().chain(acquired.iter()).any(|slot| slot.id == *id)
            });
            let Some(slot) = id.and_then(|id| register(hwnd, id, chord)) else {
                unregister(hwnd, acquired);
                return hotkeys::CONFLICT + i;
            };
            acquired.push(slot);
            next[i] = Some(slot);
        }
        let mut cfg = config::load_config(&self.paths, &home_dir());
        let value = hotkeys::value(bindings);
        if cfg.get(hotkeys::CONFIG) != Some(&value) {
            cfg.insert(hotkeys::CONFIG.into(), value);
            if config::save_config(&self.paths, &cfg).is_err() {
                unregister(hwnd, acquired);
                return hotkeys::IO_ERROR;
            }
        }
        unregister(hwnd, self.hotkeys.iter().flatten().copied().filter(|s| {
            !next.iter().flatten().any(|n| n.id == s.id)
        }));
        self.hotkeys = next;
        hotkeys::OK
    }
}

/// Read only the current selection from the focused UIA element. This runs on a COM worker because
/// providers can make cross-process calls; no focus or clipboard state is changed.
fn selected_text(hwnd: HWND) -> Option<String> {
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_MULTITHREADED,
    };
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, IUIAutomation, IUIAutomationTextPattern, UIA_TextPatternId,
    };

    unsafe {
        if CoInitializeEx(None, COINIT_MULTITHREADED).is_err() {
            return None;
        }
        let result = (|| {
            let automation: IUIAutomation =
                CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
            let element = automation.GetFocusedElement().ok()?;
            if GetForegroundWindow() != hwnd || element.CurrentIsPassword().ok()?.as_bool() {
                return None;
            }
            let pattern: IUIAutomationTextPattern =
                element.GetCurrentPatternAs(UIA_TextPatternId).ok()?;
            let ranges = pattern.GetSelection().ok()?;
            let count = ranges.Length().ok()?;
            if count <= 0 {
                return None;
            }
            let mut out = String::new();
            for i in 0..count.min(4) {
                let range = ranges.GetElement(i).ok()?;
                let text = range.GetText((MAX_SELECTION_BYTES + 1) as i32).ok()?;
                out.push_str(&text.to_string());
                if out.len() > MAX_SELECTION_BYTES {
                    break;
                }
            }
            while out.len() > MAX_SELECTION_BYTES {
                out.pop();
            }
            if GetForegroundWindow() != hwnd {
                return None;
            }
            (!out.is_empty()).then_some(out)
        })();
        CoUninitialize();
        result
    }
}

fn launch_quick_tool(paths: &Paths, tool: &'static str, hwnd: usize, shared: Arc<Shared>) {
    if shared.quick_busy.swap(true, Ordering::AcqRel) {
        return;
    }
    let paths = paths.clone();
    std::thread::spawn(move || {
        // UIA providers are third-party COM servers and a focused element can
        // block indefinitely (notably Chromium/Office while a document is
        // busy). Do not hold the quick popup behind that call. The popup is
        // launched with an empty draft after a short deadline, where the user
        // can paste or type, while a responsive provider still contributes its
        // selection automatically.
        let text = if tool == "ocr" || hwnd == 0 {
            String::new()
        } else {
            selection_before_deadline(
                &shared.quick_selection_busy,
                Duration::from_millis(350),
                move || selected_text(hwnd as HWND),
            )
            // Foreground may change after the worker read the selection.
            .filter(|_| unsafe { GetForegroundWindow() == hwnd as HWND })
            .unwrap_or_default()
        };
        let exe = std::env::current_exe()
            .unwrap_or_default()
            .with_file_name("claude-usage-dashboard.exe");
        let result = (|| {
            use std::io::Write;
            use std::os::windows::process::CommandExt;
            use std::process::{Command, Stdio};
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            let mut child = Command::new(exe)
                .arg("--quick-tool")
                .arg(tool)
                .creation_flags(CREATE_NO_WINDOW)
                .stdin(Stdio::piped())
                .spawn()
                .ok()?;
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
            }
            bind_to_job(&child);
            Some(())
        })();
        if result.is_none() {
            log(&paths, "quick tool launch failed");
        }
        shared.quick_busy.store(false, Ordering::Release);
    });
}

fn show_menu(hwnd: HWND, st: &State) {
    // SAFETY: standard popup-menu sequence on the owning thread.
    unsafe {
        let m = CreatePopupMenu();
        let add = |id: usize, text: &str, checked: Option<bool>, default: bool| {
            let mut f = MF_STRING;
            if checked == Some(true) {
                f |= MF_CHECKED;
            }
            AppendMenuW(m, f, id, wide(text).as_ptr());
            if default {
                SetMenuDefaultItem(m, id as u32, 0);
            }
        };
        add(ID_OPEN, "Open Kane's Tool Box", None, true);
        add(ID_REFRESH, "Refresh now", None, false);
        add(
            ID_WIDGET,
            "Show widget",
            Some(cfg_bool(&st.paths, "widget_enabled")),
            false,
        );
        add(
            ID_NOTIFY,
            "Notifications",
            Some(cfg_bool(&st.paths, "notify_enabled")),
            false,
        );
        add(ID_TEST, "Send test notification", None, false);
        add(ID_QUIT, "Quit", None, false);
        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        SetForegroundWindow(hwnd);
        TrackPopupMenu(
            m,
            TPM_RIGHTBUTTON | TPM_BOTTOMALIGN,
            pt.x,
            pt.y,
            0,
            hwnd,
            null(),
        );
        PostMessageW(hwnd, WM_NULL, 0, 0);
        DestroyMenu(m);
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
    if p.is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    let st = &mut *p;
    let r = catch_unwind(AssertUnwindSafe(|| match msg {
        WM_TRAY => {
            match lp as u32 {
                WM_LBUTTONUP => command(hwnd, st, ID_OPEN), // pystray's default item runs on a single left click
                WM_RBUTTONUP => show_menu(hwnd, st),
                _ => {}
            }
            Some(0)
        }
        WM_COMMAND => {
            command(hwnd, st, wp & 0xffff);
            Some(0)
        }
        WM_ICON => {
            add_or_update(hwnd, st, false);
            Some(0)
        }
        WM_TOAST => {
            let next = st.shared.toasts.lock().ok().and_then(|mut q| q.pop_front());
            if let Some(t) = next {
                notify(hwnd, st, &t);
            }
            Some(0)
        }
        WM_SHOW => {
            open_dashboard(&st.paths);
            Some(0)
        }
        hotkeys::STATUS_MESSAGE => Some(st.shortcut_status() as isize),
        WM_COPYDATA if lp != 0 => {
            let packet = &*(lp as *const COPYDATASTRUCT);
            if packet.dwData != hotkeys::APPLY_MESSAGE || packet.lpData.is_null() || !(1..=1024).contains(&packet.cbData) {
                return None;
            }
            let bytes = std::slice::from_raw_parts(packet.lpData as *const u8, packet.cbData as usize);
            let result = serde_json::from_slice::<Value>(bytes).ok()
                .and_then(|value| hotkeys::parse(&value).ok())
                .map_or(hotkeys::INVALID, |bindings| st.apply_shortcuts(hwnd, bindings));
            Some(result as isize)
        }
        WM_HOTKEY => {
            let matches = |s: &Registered| s.id == wp as i32 && s.chord.mods == (lp as u32 & 7)
                && s.chord.vk == ((lp as u32 >> 16) & 0xffff);
            if let Some(i) = st.hotkeys.iter().position(|s| s.as_ref().is_some_and(matches)) {
                if i == 0 {
                    open_dashboard(&st.paths);
                } else {
                    let foreground = GetForegroundWindow();
                    launch_quick_tool(&st.paths, hotkeys::KEYS[i], foreground as usize, st.shared.clone());
                }
            }
            Some(0)
        }
        WM_DESTROY => {
            unregister(hwnd, st.hotkeys.iter().flatten().copied());
            let n = nid(hwnd);
            Shell_NotifyIconW(NIM_DELETE, &n);
            if st.hicon != 0 {
                DestroyIcon(st.hicon as _);
                st.hicon = 0;
            }
            PostQuitMessage(0);
            Some(0)
        }
        m if m == st.taskbar_created && m != 0 => {
            add_or_update(hwnd, st, true); // explorer restarted: the icon is gone, add it again
            Some(0)
        }
        _ => None,
    }));
    match r {
        Ok(Some(v)) => v,
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

fn worker(paths: Paths, hwnd_raw: usize, shared: Arc<Shared>, stop: Arc<AtomicBool>) {
    let post = move |m: u32| {
        // SAFETY: PostMessageW is thread-safe; a dead window just makes it fail.
        unsafe {
            PostMessageW(hwnd_raw as HWND, m, 0, 0);
        }
    };
    let mut env = RealEnv::new(paths.clone(), Box::new(Ureq));
    {
        let sh = shared.clone();
        env.on_icon = Box::new(move |v| {
            if let Ok(mut g) = sh.view.lock() {
                *g = Some(v.clone());
            }
            post(WM_ICON);
        });
        let sh = shared.clone();
        env.on_toast = Box::new(move |t| {
            sh.toasts
                .lock()
                .map_err(|e| e.to_string())?
                .push_back(t.clone());
            post(WM_TOAST);
            Ok(())
        });
    }
    let mut d = Daemon::new(env);
    let mut managed_llama = ManagedLlama::new();
    // R1 policy: debug builds never touch settings.json; release only repairs a hook whose exe vanished.
    if std::env::var_os("CU_NO_REPAIR").is_none() {
        if let Ok(exe) = std::env::current_exe() {
            let profs: Vec<HookProfile> = config::profiles(&d.poller.cfg)
                .iter()
                .map(|p| HookProfile::new(p.name.clone(), &p.path))
                .collect();
            let lp = paths.clone();
            hookinstall::repair(&profs, &exe, false, &mut |m| log(&lp, m));
        }
    }
    // App.__init__: keep the registry command pointing at this exe when autostart is on.
    if daemon::autostart_allowed()
        && d.poller.cfg.get("autostart").and_then(Value::as_bool) == Some(true)
    {
        if let Ok(exe) = std::env::current_exe() {
            daemon::set_autostart(true, &exe);
        }
    }
    while !stop.load(Ordering::SeqCst) {
        managed_llama.tick(&paths);
        let _ = catch_unwind(AssertUnwindSafe(|| d.step()));
        if std::mem::take(&mut d.show_requested) {
            post(WM_SHOW);
        }
        for _ in 0..10 {
            if stop.load(Ordering::SeqCst) {
                managed_llama.stop(&paths);
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    managed_llama.stop(&paths);
}

/// Run the tray app until Quit.
pub fn run(paths: Paths) -> i32 {
    crate::widget::init_dpi();
    // SAFETY: Win32 setup/teardown on the UI thread; the worker only posts messages.
    unsafe {
        let hinst = GetModuleHandleW(null());
        let class = wide(CLASS);
        let wc = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinst,
            hIcon: null_mut(),
            hCursor: null_mut(),
            hbrBackground: null_mut(),
            lpszMenuName: null(),
            lpszClassName: class.as_ptr(),
        };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            wide(TITLE).as_ptr(),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            hinst,
            null(),
        );
        if hwnd.is_null() {
            log(&paths, "tray: cannot create window");
            return 1;
        }
        daemon::startup(&paths);
        let shared = Arc::new(Shared::default());
        let st = Box::into_raw(Box::new(State {
            paths: paths.clone(),
            shared: shared.clone(),
            hicon: 0,
            taskbar_created: RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()),
            hotkeys: [None; 4],
            winrt: daemon::autostart_allowed()
                && std::env::current_exe().is_ok_and(|e| crate::toast::register(&e)),
        }));
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, st as isize);
        add_or_update(hwnd, &mut *st, true);
        if std::env::var_os("CU_NO_HOTKEYS").is_none() {
            let cfg = config::load_config(&paths, &home_dir());
            for (i, chord) in hotkeys::configured(&cfg).into_iter().enumerate() {
                if let Some(chord) = chord {
                    (*st).hotkeys[i] = register(hwnd, HOTKEY_ID_FIRST + i as i32, chord);
                    if (*st).hotkeys[i].is_none() {
                        log(&paths, &format!("hotkey unavailable: {}", chord.label()));
                    }
                }
            }
        }
        crate::widget::win::spawn(paths.clone(), hwnd as isize);
        let stop = Arc::new(AtomicBool::new(false));
        let (p2, s2, st2) = (paths.clone(), shared.clone(), stop.clone());
        let hw = hwnd as usize;
        let th = std::thread::spawn(move || worker(p2, hw, s2, st2));
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        stop.store(true, Ordering::SeqCst);
        let _ = th.join();
        daemon::shutdown(&paths);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        drop(Box::from_raw(st));
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_timeout_keeps_only_one_lookup_and_ignores_late_results() {
        use std::sync::mpsc::channel;
        use std::time::Instant;
        let busy = Arc::new(AtomicBool::new(false));
        let (release, blocked) = channel();
        let result = selection_before_deadline(&busy, Duration::from_millis(20), move || {
            let _ = blocked.recv();
            Some("stale".into())
        });
        assert_eq!(result, None, "timed-out UIA must allow an empty draft");
        assert!(busy.load(Ordering::Acquire));
        let starts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let other = starts.clone();
        let result = selection_before_deadline(&busy, Duration::from_secs(1), move || {
            other.fetch_add(1, Ordering::Relaxed);
            Some("unexpected".into())
        });
        assert_eq!(result, None);
        assert_eq!(starts.load(Ordering::Relaxed), 0);
        release.send(()).unwrap();
        let until = Instant::now() + Duration::from_secs(2);
        while busy.load(Ordering::Acquire) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(!busy.load(Ordering::Acquire), "lookup did not release slot");
        assert_eq!(
            selection_before_deadline(&busy, Duration::from_secs(2), || Some("current".into())),
            Some("current".into())
        );
    }

    #[test]
    fn fill_truncates_and_terminates() {
        let mut b = [0xffffu16; 8];
        fill(&mut b, "abcdefghij");
        assert_eq!(&b[..7], &"abcdefg".encode_utf16().collect::<Vec<_>>()[..]);
        assert_eq!(b[7], 0);
        let mut b = [0xffffu16; 4];
        fill(&mut b, "ab\u{1F600}"); // 2 units + NUL do not leave room for a surrogate pair: dropped whole
        assert_eq!((b[0], b[1], b[2]), (b'a' as u16, b'b' as u16, 0));
        let mut b = [1u16; 3];
        fill(&mut b, "");
        assert_eq!(b[0], 0);
    }
}
