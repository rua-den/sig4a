//! Thin Win32 helpers (physical pixels, like dashboard.py's private user32 calls).

use std::path::Path;
use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromRect, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows_sys::Win32::System::Registry::*;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
use windows_sys::Win32::UI::HiDpi::{GetDpiForSystem, GetDpiForWindow};
use windows_sys::Win32::UI::Shell::{SetWindowSubclass, SUBCLASSPROC};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

pub const CLASS: &str = "ClaudeUsageDashboard";
pub const QUICK_CLASS: &str = "ClaudeUsageQuickPopup";
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "ClaudeUsage";

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn h(hwnd: isize) -> HWND {
    hwnd as HWND
}

pub fn rect(hwnd: isize) -> [i32; 4] {
    let mut r = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    // SAFETY: valid out pointer; a stale handle just yields zeros.
    unsafe { GetWindowRect(h(hwnd), &mut r) };
    [r.left, r.top, r.right - r.left, r.bottom - r.top]
}

/// SetWindowPos(NOZORDER | NOACTIVATE): never shows a hidden window (the page sizes it first).
pub fn place(hwnd: isize, r: [i32; 4]) {
    // SAFETY: plain call on our own window.
    unsafe {
        SetWindowPos(
            h(hwnd),
            std::ptr::null_mut(),
            r[0],
            r[1],
            r[2].max(1),
            r[3].max(1),
            SWP_NOZORDER | SWP_NOACTIVATE,
        )
    };
}

/// Work area (excluding the taskbar) of the monitor nearest to the window: [x, y, w, h].
pub fn work_area(hwnd: isize) -> [i32; 4] {
    // SAFETY: MONITORINFO is initialised with its size as the API requires.
    unsafe {
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        GetMonitorInfoW(
            MonitorFromWindow(h(hwnd), MONITOR_DEFAULTTONEAREST),
            &mut mi,
        );
        let w = mi.rcWork;
        [w.left, w.top, w.right - w.left, w.bottom - w.top]
    }
}

/// Monitor with the largest overlap with the selected screen rectangle.
pub fn capture_work_area(bounds: [i32; 4], hwnd: isize) -> [i32; 4] {
    let [x, y, w, h] = bounds;
    let selected = RECT { left: x, top: y, right: x + w, bottom: y + h };
    unsafe {
        let monitor = MonitorFromRect(&selected, MONITOR_DEFAULTTONEAREST);
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(monitor, &mut info) == 0 {
            return work_area(hwnd);
        }
        let r = info.rcWork;
        [r.left, r.top, r.right - r.left, r.bottom - r.top]
    }
}

/// Scale as measured by the per-monitor-aware HWND after moving it to its destination.
pub fn window_scale(hwnd: isize) -> f64 {
    let dpi = unsafe { GetDpiForWindow(h(hwnd)) };
    if dpi == 0 { system_scale() } else { f64::from(dpi) / 96.0 }
}

/// Primary screen size in physical pixels (SM_CXSCREEN / SM_CYSCREEN).
pub fn screen() -> (i32, i32) {
    // SAFETY: no pointers.
    unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) }
}

pub fn system_scale() -> f64 {
    // SAFETY: no pointers.
    let d = unsafe { GetDpiForSystem() };
    if d == 0 {
        1.0
    } else {
        f64::from(d) / 96.0
    }
}

pub fn hide(hwnd: isize) {
    // SAFETY: ShowWindowAsync never blocks the UI thread.
    unsafe { ShowWindowAsync(h(hwnd), SW_HIDE) };
}

/// End the process at once (skips the slow WebView2 / DLL-detach teardown of ExitProcess; WebView2's own
/// msedgewebview2 processes exit when their host is gone).
pub fn terminate() -> ! {
    // SAFETY: terminating ourselves.
    unsafe { TerminateProcess(GetCurrentProcess(), 0) };
    std::process::exit(0)
}

/// An already open dashboard window (any process), for single-instance focus.
pub fn find_existing(class: &str) -> Option<isize> {
    // SAFETY: NUL-terminated class name.
    let w = unsafe { FindWindowW(wide(class).as_ptr(), std::ptr::null()) };
    (!w.is_null()).then_some(w as isize)
}

pub fn focus(hwnd: isize) {
    // SAFETY: plain calls on a foreign window handle.
    unsafe {
        if IsIconic(h(hwnd)) != 0 {
            ShowWindow(h(hwnd), SW_RESTORE);
        } else if IsWindowVisible(h(hwnd)) == 0 {
            ShowWindow(h(hwnd), SW_SHOW);
        }
        SetForegroundWindow(h(hwnd));
    }
}

pub fn install_quick_subclass(hwnd: isize, proc: SUBCLASSPROC, data: usize) -> bool {
    unsafe { SetWindowSubclass(h(hwnd), proc, 1, data) != 0 }
}

pub fn send_quick(hwnd: isize, payload: &[u8]) -> bool {
    if payload.len() > 32 * 1024 {
        return false;
    }
    let mut cds = windows_sys::Win32::System::DataExchange::COPYDATASTRUCT {
        dwData: 0x4b5442,
        cbData: payload.len() as u32,
        lpData: payload.as_ptr() as *mut _,
    };
    let mut out = 0usize;
    unsafe {
        SendMessageTimeoutW(
            h(hwnd),
            WM_COPYDATA,
            0,
            &mut cds as *mut _ as isize,
            SMTO_ABORTIFHUNG,
            1000,
            &mut out,
        ) != 0
    }
}

pub fn message_box(text: &str) {
    // SAFETY: NUL-terminated strings.
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            wide(text).as_ptr(),
            wide("claude-usage").as_ptr(),
            MB_OK | MB_ICONWARNING,
        )
    };
}

/// `core.set_autostart`: same value name as the tray build and the Python one, so there is only ever one entry.
pub fn set_autostart(on: bool, exe: &Path) -> bool {
    // SAFETY: registry calls with NUL-terminated UTF-16 strings; the key is closed.
    unsafe {
        let mut key: HKEY = std::ptr::null_mut();
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            wide(RUN_KEY).as_ptr(),
            0,
            KEY_SET_VALUE,
            &mut key,
        ) != 0
        {
            return false;
        }
        let name = wide(RUN_VALUE);
        let ok = if on {
            let v = wide(&format!("\"{}\" \"--hidden\"", exe.display()));
            RegSetValueExW(
                key,
                name.as_ptr(),
                0,
                REG_SZ,
                v.as_ptr() as *const u8,
                (v.len() * 2) as u32,
            ) == 0
        } else {
            let r = RegDeleteValueW(key, name.as_ptr());
            r == 0 || r == 2
        };
        RegCloseKey(key);
        ok
    }
}
