//! claude-usage dashboard: its own short-lived process (RAM is freed on close), frameless wry/WebView2 window that
//! shows the same `ui.html` as the Python build through a small pywebview-compatible bridge (`shim.js`).
//! Port of `dashboard.py`; the data/actions half is in `api.rs`, the geometry rules in `geom.rs`.
#![windows_subsystem = "windows"]

mod api;
mod geom;
mod live;
mod region;
mod win;

use api::Core;
use geom::{MIN_H, MIN_W};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tao::dpi::{LogicalSize, PhysicalSize};
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tao::platform::windows::{WindowBuilderExtWindows, WindowExtWindows};
use tao::window::{Window, WindowBuilder};
use windows_sys::Win32::System::DataExchange::COPYDATASTRUCT;
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{WM_COPYDATA, WM_NCDESTROY};
use wry::{http, WebContext, WebViewBuilder, WebViewBuilderExtWindows};

const UI_HTML: &str = include_str!("../../../ui.html");
const SHIM: &str = include_str!("shim.js");
// Wry 0.55 remaps this internal custom scheme to this exact WebView2
// HTTPS URL. Its response comes from memory; it is not a network server.
const DASHBOARD_SCHEME: &str = "sig4a-bundled";
const DASHBOARD_BUNDLE_URL: &str = "sig4a-bundled://localhost/dashboard.html";
const DASHBOARD_DOCUMENT_URL: &str = "https://sig4a-bundled.localhost/dashboard.html";

const PERSIST_DELAY: Duration = Duration::from_millis(800);
const READY_TIMEOUT: Duration = Duration::from_secs(4);
const QUICK_PRESENT_TIMEOUT: Duration = Duration::from_secs(8);
const LIVE_LEASE: Duration = Duration::from_secs(3);

enum Ev {
    Call(u64, String, Vec<Value>),
    Done(u64, Result<Value, String>),
    QuickTool(String, String),
    OpenLive,
    LiveNavigation { replaces_document: bool },
    OcrDone(Option<u64>, Result<Option<region::Capture>, String>),
}

fn bundled_document(uri: &str) -> bool {
    uri == DASHBOARD_DOCUMENT_URL
}

fn revoke_page_for_navigation(page: &mut Option<String>, heartbeat: &mut Option<Instant>, replaces_document: bool) {
    *heartbeat = None;
    if replaces_document { *page = None; }
}

fn bridge_result(id: u64, result: Result<Value, String>) -> String {
    match result {
        Ok(value) => format!("__cu_resolve({id},true,{value})"),
        Err(error) => format!("__cu_resolve({id},false,{})", json!(error)),
    }
}
static OCR_BUSY: AtomicBool = AtomicBool::new(false);
static AI_BUSY: AtomicBool = AtomicBool::new(false);

/// Only the current hidden view can authorize showing the native popup.
#[derive(Default)]
struct QuickPresentation {
    generation: u64,
    pending: Option<u64>,
    deadline: Option<Instant>,
}

impl QuickPresentation {
    /// Accepting a crop blocks another picker even before WebView can receive a token.
    fn await_render(&mut self, now: Instant) {
        self.deadline.get_or_insert(now + QUICK_PRESENT_TIMEOUT);
    }

    fn awaiting_render(&self) -> bool {
        self.deadline.is_some()
    }

    fn begin(&mut self) -> u64 {
        self.await_render(Instant::now());
        self.generation += 1;
        self.pending = Some(self.generation);
        self.generation
    }

    fn invalidate(&mut self) {
        self.pending = None;
        self.deadline = None;
    }

    fn acknowledge(&mut self, token: Option<u64>) -> bool {
        if token.is_none() || self.pending != token {
            return false;
        }
        self.invalidate();
        true
    }
}

fn anchor_capture(hwnd: isize, bounds: [i32; 4]) {
    let area = win::capture_work_area(bounds, hwnd);
    // Move the hidden HWND to its destination before measuring that monitor's DPI.
    win::place(hwnd, geom::capture_popup(bounds, area, 1.0));
    let scale = win::window_scale(hwnd);
    win::place(hwnd, geom::capture_popup(bounds, area, scale));
}

fn start_capture(id: Option<u64>, window: &Window, proxy: &EventLoopProxy<Ev>) -> bool {
    if OCR_BUSY.swap(true, Ordering::AcqRel) {
        return false;
    }
    window.set_visible(false);
    let proxy = proxy.clone();
    std::thread::spawn(move || {
        let _ = proxy.send_event(Ev::OcrDone(id, region::capture()));
    });
    true
}


unsafe extern "system" fn quick_subclass(
    hwnd: windows_sys::Win32::Foundation::HWND,
    msg: u32,
    wp: usize,
    lp: isize,
    _id: usize,
    data: usize,
) -> isize {
    if msg == cu_core::hotkeys::LIVE_OPEN_MESSAGE && data != 0 {
        let _ = (&*(data as *const EventLoopProxy<Ev>)).send_event(Ev::OpenLive);
        return 1;
    }
    if msg == WM_COPYDATA && data != 0 && lp != 0 {
        let cds = &*(lp as *const COPYDATASTRUCT);
        if cds.dwData == 0x4b5442 && cds.cbData <= 32 * 1024 && !cds.lpData.is_null() {
            let bytes = std::slice::from_raw_parts(cds.lpData as *const u8, cds.cbData as usize);
            if let Ok(v) = serde_json::from_slice::<[String; 2]>(bytes) {
                if v[0].len() <= 32
                    && v[1].len() <= 32 * 1024
                    && matches!(v[0].as_str(), "grammar" | "translation" | "ocr")
                {
                    let _ = (&*(data as *const EventLoopProxy<Ev>))
                        .send_event(Ev::QuickTool(v[0].clone(), v[1].clone()));
                    return 1;
                }
            }
        }
    }
    if msg == WM_NCDESTROY {
        let _ = RemoveWindowSubclass(hwnd, Some(quick_subclass), 1);
        drop(Box::from_raw(data as *mut EventLoopProxy<Ev>));
    }
    DefSubclassProc(hwnd, msg, wp, lp)
}

struct Geo {
    user_sized: bool,
    maximized: bool,
    normal: Option<[i32; 4]>,
    persist_at: Option<Instant>,
}

struct App {
    core: Core,
    hwnd: isize,
    quick_mode: bool,
    anchored: bool,
    geo: Geo,
    ready: bool,
    live: live::Live,
    live_page: Option<String>,
    live_heartbeat: Option<Instant>,
    live_pending: bool,
    quick_pending: Option<(String, String)>,
    /// `--e2e` result path: multi-stage scripts hand control to the test through `<out>.<stage>` files.
    #[cfg(any(debug_assertions, feature = "e2e"))]
    e2e_out: Option<String>,
}

impl App {
    fn persist_soon(&mut self) {
        self.geo.persist_at = Some(Instant::now() + PERSIST_DELAY);
    }

    /// Remember the size the user picked (the normal rect while maximized). Only when the user chose one.
    fn persist_now(&mut self) {
        self.geo.persist_at = None;
        if !self.geo.user_sized {
            return;
        }
        let r = match (self.geo.maximized, self.geo.normal) {
            (true, Some(n)) => n,
            _ => win::rect(self.hwnd),
        };
        if r[2] < MIN_W / 2 || r[3] < MIN_H / 2 {
            return; // window already gone: keep the last good size
        }
        self.core.set_dash_rect(Some(r));
    }

    /// `hide`: hide at once (feels instant), save the size, end the process.
    fn close(&mut self) -> ! {
        win::hide(self.hwnd);
        self.live.close();
        if !self.quick_mode { self.persist_now(); }
        win::terminate()
    }

    fn restore_saved(&mut self) {
        let (sw, sh) = win::screen();
        if let Some(r) = geom::restore_rect(&self.core.saved_rect(), sw, sh) {
            win::place(self.hwnd, r);
            self.geo.user_sized = true;
        }
    }

    fn check_live_lease(&mut self, now: Instant) {
        if !win::visible_live_window(self.hwnd)
            || self.live_heartbeat.is_none_or(|seen| now.saturating_duration_since(seen) >= LIVE_LEASE)
        {
            self.live_heartbeat = None;
            self.live.view_lost();
        }
    }

    fn record_live_heartbeat(&mut self, now: Instant) {
        if win::visible_live_window(self.hwnd) {
            self.live_heartbeat = Some(now);
            self.live.renew_view(now + LIVE_LEASE);
        }
    }

    fn live_token(value: Option<&Value>) -> Option<&str> {
        value.and_then(Value::as_str).filter(|s| s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit()))
    }

    fn live_page_matches(&self, value: Option<&Value>) -> bool {
        Self::live_token(value).is_some_and(|token| self.live_page.as_deref() == Some(token))
    }

    fn call(&mut self, window: &Window, m: &str, a: &[Value]) -> Result<Value, String> {
        let f = |i: usize| a.get(i).and_then(Value::as_f64).unwrap_or(0.0);
        let c = &self.core;
        Ok(match m {
            "shortcut_status" => {
                use cu_core::hotkeys as hk;
                match win::tray_call(hk::STATUS_MESSAGE, None) {
                    Some(n) if (hk::STATUS..hk::STATUS + hk::STATUS_COUNT).contains(&n) => json!({"code":"ok","unavailable":n - hk::STATUS}),
                    Some(hk::DISABLED) => json!({"code":"disabled"}),
                    _ => json!({"code":"tray"}),
                }
            }
            "shortcut_save" => {
                use cu_core::hotkeys as hk;
                let bindings = match hk::parse(a.first().unwrap_or(&Value::Null)) {
                    Ok(bindings) => bindings,
                    Err((i, duplicate)) => return Ok(json!({"code":if duplicate {"duplicate"} else {"invalid"}, "action":hk::KEYS[i]})),
                };
                let bindings = hk::value(bindings);
                match win::tray_call(WM_COPYDATA, Some(bindings.to_string().as_bytes())) {
                    Some(hk::OK) => json!({"code":"ok","bindings":bindings}),
                    Some(hk::IO_ERROR) => json!({"code":"io"}),
                    Some(hk::DISABLED) => json!({"code":"disabled"}),
                    Some(n) if (hk::CONFLICT..hk::CONFLICT + hk::COUNT).contains(&n) => json!({"code":"conflict","action":hk::KEYS[n - hk::CONFLICT]}),
                    _ => json!({"code":"tray"}),
                }
            }
            "live_view" if !self.quick_mode => {
                let Some(token) = Self::live_token(a.get(1)) else { return Ok(Value::Null); };
                if a.first().and_then(Value::as_bool) == Some(true) {
                    if self.live_page.as_deref() != Some(token) {
                        self.live.view_lost();
                        self.live_page = Some(token.to_owned());
                    }
                    self.record_live_heartbeat(Instant::now());
                } else if self.live_page.as_deref() == Some(token) {
                    self.live_page = None;
                    self.live_heartbeat = None;
                    self.live.stop(false);
                    win::set_topmost(self.hwnd, false);
                }
                self.live.call("live_state", &Value::Null)
            }
            "live_heartbeat" if !self.quick_mode && self.live_page_matches(a.first()) => {
                let now = Instant::now();
                self.check_live_lease(now);
                self.record_live_heartbeat(now);
                Value::Null
            }
            "live_pin" if !self.quick_mode && self.live_page_matches(a.get(1)) => {
                win::set_topmost(self.hwnd, a.first().and_then(Value::as_bool).unwrap_or(false));
                Value::Bool(true)
            }
            "live_list" | "live_start" | "live_stop" | "live_pause" | "live_state"
                if !self.quick_mode && self.live_page_matches(a.get(1)) =>
            {
                self.check_live_lease(Instant::now());
                if m == "live_start" && self.live_heartbeat.is_none() {
                    return Ok(self.live.call("live_state", &Value::Null));
                }
                self.live.call(m, a.first().unwrap_or(&Value::Null))
            }
            "open_toolbox" => Value::Bool(win::open_toolbox()),
            "state" => c.state(),
            "test_notify" => c.flag(&c.paths.testflag),
            "refresh" => c.flag(&c.paths.flag),
            "hook_status" => c.hook_status(),
            "hook_install" => c.hook_install(),
            "hook_uninstall" => c.hook_uninstall(),
            "cleaner_scan" => c.cleaner_scan(Some(a.first().unwrap_or(&Value::Null))),
            "cleaner_status" => c.cleaner_status(),
            "cleaner_dups_scan" => c.cleaner_dups_scan(Some(a.first().unwrap_or(&Value::Null))),
            "cleaner_dups_status" => c.cleaner_dups_status(),
            "cleaner_cancel_review" => c.cleaner_cancel_review(a.first().and_then(Value::as_str).unwrap_or("")),
            "cleaner_review" | "cleaner_clean" => {
                let ids: &[Value] = a
                    .first()
                    .and_then(Value::as_array)
                    .map_or(&[][..], |v| v.as_slice());
                let mode = a.get(1).and_then(Value::as_str).unwrap_or("");
                let scan_id = a.get(2).and_then(Value::as_str).unwrap_or("");
                if m == "cleaner_review" {
                    c.cleaner_review(ids, mode, scan_id)
                } else {
                    c.cleaner_clean(
                        ids, mode, scan_id, a.get(3).and_then(Value::as_str).unwrap_or(""),
                        a.get(4).and_then(Value::as_u64), a.get(5).and_then(Value::as_u64),
                    )
                }
            }
            "widget_unlock" => c.widget_unlock(),
            "widget_reset" => c.widget_reset(),
            "save" => c.save(a.first().unwrap_or(&Value::Null)),
            "hide" => self.close(),
            "minimize" => {
                window.set_minimized(true);
                Value::Null
            }
            "rect" => json!(win::rect(self.hwnd)),
            "set_rect" => {
                // resize by the user (edge handles): from now on the window no longer follows the content height
                self.geo.user_sized = true;
                self.geo.maximized = false;
                let r = [f(0) as i32, f(1) as i32, f(2) as i32, f(3) as i32];
                win::place(self.hwnd, [r[0], r[1], r[2].max(MIN_W), r[3].max(MIN_H)]);
                self.persist_soon();
                Value::Bool(true)
            }
            "work_area" => json!(win::work_area(self.hwnd)),
            "toggle_max" => {
                match (self.geo.maximized, self.geo.normal) {
                    (true, Some(n)) => {
                        win::place(self.hwnd, n);
                        self.geo.maximized = false;
                    }
                    _ => {
                        self.geo.normal = Some(win::rect(self.hwnd));
                        win::place(self.hwnd, win::work_area(self.hwnd));
                        self.geo.maximized = true;
                        self.geo.user_sized = true;
                    }
                }
                self.persist_soon();
                Value::Bool(true)
            }
            "auto_fit" => {
                // forget the chosen size: the window follows the content again
                self.geo.user_sized = false;
                self.geo.maximized = false;
                self.core.set_dash_rect(None);
                Value::Bool(true)
            }
            "win_state" => {
                json!({"user_sized": self.geo.user_sized, "maximized": self.geo.maximized})
            }
            // e2e only: `e2e_stage(name, data)` writes `<out>.<name>` (the test inspects the sandbox, edits files,
            // then creates `<out>.<name>.go`); `e2e_go(name)` tells whether the test released the stage.
            #[cfg(any(debug_assertions, feature = "e2e"))]
            "e2e_stage" | "e2e_go" => {
                let Some(out) = &self.e2e_out else {
                    return Err("not in e2e mode".into());
                };
                let name = a.first().and_then(Value::as_str).unwrap_or("");
                if m == "e2e_stage" {
                    let _ = std::fs::write(
                        format!("{out}.{name}"),
                        a.get(1).map_or("null".into(), Value::to_string),
                    );
                    Value::Bool(true)
                } else {
                    Value::Bool(std::path::Path::new(&format!("{out}.{name}.go")).exists())
                }
            }
            "page_ready" => {
                self.ready = true;
                Value::Bool(true)
            }
            "fit" => {
                if self.anchored || self.geo.user_sized || self.geo.maximized {
                    return Ok(Value::Bool(false)); // the user's size wins
                }
                let scale = window.scale_factor();
                let cur = win::rect(self.hwnd);
                win::place(self.hwnd, geom::fit_rect_in_area(cur, f(0), scale, win::work_area(self.hwnd)));
                Value::Bool(true)
            }
            _ => return Err(format!("unknown method: {m}")),
        })
    }
}

/// WebView2 data folder for this run inside our own data dir; older ones (left behind because the process is
/// killed on close) are swept in the background. A folder still in use is locked and simply stays.
fn storage_dir(data: &Path) -> PathBuf {
    let root = data.join("webview");
    let mine = root.join(std::process::id().to_string());
    let keep = mine.clone();
    std::thread::spawn(move || {
        if let Ok(rd) = std::fs::read_dir(&root) {
            for e in rd.flatten() {
                if e.path() != keep && e.path().is_dir() {
                    let _ = std::fs::remove_dir_all(e.path());
                }
            }
        }
    });
    mine
}

fn storage_dir_named(data: &Path, name: &str) -> PathBuf {
    let root = data.join(name);
    let mine = root.join(std::process::id().to_string());
    let keep = mine.clone();
    std::thread::spawn(move || {
        if let Ok(rd) = std::fs::read_dir(&root) {
            for e in rd.flatten() {
                if e.path() != keep && e.path().is_dir() {
                    let _ = std::fs::remove_dir_all(e.path());
                }
            }
        }
    });
    mine
}

fn arg_after(flag: &str) -> Option<(String, String)> {
    let v: Vec<String> = std::env::args().collect();
    let i = v.iter().position(|a| a == flag)?;
    Some((v.get(i + 1)?.clone(), v.get(i + 2)?.clone()))
}

fn quick_tool() -> Option<(String, String)> {
    let mut it = std::env::args().skip(1);
    if it.next()?.as_str() != "--quick-tool" {
        return None;
    }
    let tool = it.next()?;
    if !matches!(tool.as_str(), "grammar" | "translation" | "ocr") {
        return None;
    }
    use std::io::Read;
    let mut text = String::new();
    std::io::stdin()
        .take(32 * 1024)
        .read_to_string(&mut text)
        .ok()?;
    Some((tool, text))
}

fn main() {
    let Ok(paths) = cu_core::paths::Paths::from_env() else {
        std::process::exit(1);
    };
    let quick = quick_tool();
    let quick_mode = quick.is_some();
    let startup_live = !quick_mode && std::env::args().nth(1).as_deref() == Some("--live-translate");
    let startup_ocr = quick.as_ref().is_some_and(|(tool, _)| tool == "ocr");
    let instance_class = if quick_mode { win::QUICK_CLASS } else { win::CLASS };
    if let Some(existing) = win::find_existing(instance_class) {
        if let Some((tool, text)) = quick.clone() {
            if let Ok(payload) = serde_json::to_vec(&[tool, text]) {
                if win::send_quick(existing, &payload) { return; }
            }
        }
        // A quick process owns its visibility. In particular a timeout sending
        // WM_COPYDATA while the first picker is active must not unhide that HWND.
        if !quick_mode {
            if startup_live {
                win::request_live_page(existing);
            }
            win::focus(existing);
        }
        return;
    }
    let exe = std::env::current_exe().unwrap_or_default();
    let core = Core::new(
        paths,
        cu_core::paths::home_dir(),
        exe.with_file_name("claude-usage.exe"),
        !cfg!(debug_assertions) && std::env::var_os("CU_NO_AUTOSTART").is_none(),
    );
    #[cfg(any(debug_assertions, feature = "e2e"))]
    let e2e = arg_after("--e2e");
    #[cfg(not(any(debug_assertions, feature = "e2e")))]
    let e2e: Option<(String, String)> = {
        let _ = arg_after;
        None
    };

    let n_profiles = core
        .cfg()
        .get("profiles")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let scale = win::system_scale();
    let width = if quick_mode { 420 } else { geom::initial_width(n_profiles, f64::from(win::screen().0) / scale) };

    let event_loop = EventLoopBuilder::<Ev>::with_user_event().build();
    let proxy: EventLoopProxy<Ev> = event_loop.create_proxy();
    let mut window_builder = WindowBuilder::new()
        .with_title("Kane's Tool Box")
        .with_decorations(false)
        .with_undecorated_shadow(false)
        .with_visible(false)
        .with_inner_size(LogicalSize::new(f64::from(width), if quick_mode { 315.0 } else { 640.0 }))
        .with_min_inner_size(PhysicalSize::new(if quick_mode { 1 } else { MIN_W as u32 }, if quick_mode { 1 } else { MIN_H as u32 }))
        .with_window_classname(instance_class);
    if quick_mode { window_builder = window_builder.with_skip_taskbar(true); }
    let window = match window_builder.build(&event_loop)
    {
        Ok(w) => w,
        Err(e) => fail(&core, &format!("window: {e}")),
    };
    let quick_proxy = Box::into_raw(Box::new(proxy.clone())) as usize;
    if !win::install_quick_subclass(window.hwnd() as isize, Some(quick_subclass), quick_proxy) {
        unsafe {
            drop(Box::from_raw(quick_proxy as *mut EventLoopProxy<Ev>));
        }
    }

    if quick_mode {
        let wa = win::work_area(window.hwnd() as isize);
        let w = (420.0 * scale) as i32;
        let h = (315.0 * scale) as i32;
        win::place(window.hwnd() as isize, [wa[0] + (wa[2] - w).max(0) / 2, wa[1] + (wa[3] - h).max(0) / 3, w, h]);
    }
    // Cold Ctrl+Alt+O presents the picker immediately, before any WebView2 setup.
    // The picker already has its own Win32 thread/message loop; wait for its result
    // directly instead of posting a Tao event that could be pumped during WebView
    // construction. A first Escape never constructs WebView or leaves a hidden app.
    let initial_capture = if startup_ocr {
        match region::capture() {
            Ok(None) => win::terminate(),
            Ok(Some(capture)) => {
                anchor_capture(window.hwnd() as isize, capture.bounds);
                Some(Ok(capture.image))
            }
            Err(error) => Some(Err(error)),
        }
    } else {
        None
    };
    let mut ctx = WebContext::new(Some(if quick_mode { storage_dir_named(&core.paths.data, "webview-quick") } else { storage_dir(&core.paths.data) }));
    let handler_proxy = proxy.clone();
    let nav_proxy = proxy.clone();
    let mode_script = format!("window.__SIG4A_RUST_LIVE__=true;window.__QUICK_MODE__={};", if quick_mode { "true" } else { "false" });
    let page = core.page(UI_HTML, quick_mode).into_bytes();
    let built = WebViewBuilder::new_with_web_context(&mut ctx)
        .with_https_scheme(true)
        .with_custom_protocol(DASHBOARD_SCHEME.into(), move |_, request| {
            // Windows Wry reverts the URL to the custom scheme in this callback.
            let allowed = request.method() == http::Method::GET && request.uri().to_string() == DASHBOARD_BUNDLE_URL;
            let mut response = http::Response::builder()
                .header("Cache-Control", "no-store")
                .header("X-Content-Type-Options", "nosniff");
            if allowed {
                response = response
                    .header("Content-Type", "text/html; charset=utf-8")
                    .header("Content-Security-Policy", "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data:; base-uri 'none'; form-action 'none'; frame-src 'none'");
                response.body(std::borrow::Cow::Owned(page.clone())).unwrap()
            } else {
                response.status(404).body(std::borrow::Cow::<'static, [u8]>::Borrowed(&[])).unwrap()
            }
        })
        .with_url(DASHBOARD_BUNDLE_URL)
        .with_initialization_script(SHIM)
        .with_initialization_script(&mode_script)
        .with_background_color((10, 10, 10, 255))
        .with_incognito(true)
        .with_navigation_handler(move |uri| {
            // Allow only the fixed bundled Windows URL. A navigation request
            // may differ from a committed document origin (e.g. NavigateToString).
            let replaces_document = bundled_document(&uri);
            let _ = nav_proxy.send_event(Ev::LiveNavigation { replaces_document });
            replaces_document
        })
        .with_ipc_handler(move |req| {
            // The script bridge is globally injected by Wry; validate the actual WebView2 source.
            if !bundled_document(&req.uri().to_string()) { return; }
            if let Ok(v) = serde_json::from_str::<Value>(req.body()) {
                let id = v["id"].as_u64().unwrap_or(0);
                let m = v["m"].as_str().unwrap_or("").to_string();
                let a = v["a"].as_array().cloned().unwrap_or_default();
                let _ = handler_proxy.send_event(Ev::Call(id, m, a));
            }
        })
        .build(&window);
    let webview = match built {
        Ok(w) => w,
        Err(e) => fail(
            &core,
            &format!("WebView2 không khởi động được: {e}\nCài Microsoft Edge WebView2 Runtime rồi mở lại."),
        ),
    };

    let mut app = App {
        hwnd: window.hwnd() as isize,
        core,
        quick_mode,
        anchored: startup_ocr,
        geo: Geo {
            user_sized: false,
            maximized: false,
            normal: None,
            persist_at: None,
        },
        ready: false,
        live: live::Live::default(),
        live_page: None,
        live_heartbeat: None,
        live_pending: startup_live,
        quick_pending: quick.filter(|(tool, _)| tool != "ocr"),
        #[cfg(any(debug_assertions, feature = "e2e"))]
        e2e_out: e2e.as_ref().map(|(_, o)| o.clone()),
    };
    if !quick_mode { app.restore_saved(); }
    let started = Instant::now();
    let mut shown = false;
    let mut e2e_started = false;
    // "shown" means a panel has previously been presented; it stays true
    // while a new native picker temporarily hides that panel.
    let mut hidden_for_picker = startup_ocr;
    let mut want_reveal = false;
    let mut present = QuickPresentation::default();
    let mut hotkey_result = initial_capture;
    if hotkey_result.is_some() {
        present.await_render(started);
    }

    event_loop.run(move |event, _, flow| {
        app.check_live_lease(Instant::now());
        match event {
            Event::UserEvent(Ev::LiveNavigation { replaces_document }) => {
                app.live.view_lost();
                // An accepted replacement forgets the old renderer nonce, even for
                // a same-URI reload. A blocked attempt leaves the current document
                // in place; it can acquire a fresh lease but never restart on its own.
                revoke_page_for_navigation(&mut app.live_page, &mut app.live_heartbeat, replaces_document);
                if replaces_document && !app.quick_mode { win::set_topmost(app.hwnd, false); }
            }
            Event::UserEvent(Ev::OpenLive) => {
                if !app.quick_mode { app.live_pending = true; }
            }
            Event::UserEvent(Ev::QuickTool(tool, text)) => {
                if tool == "ocr" {
                    if !present.awaiting_render() && start_capture(None, &window, &proxy) {
                        hidden_for_picker = true;
                        want_reveal = false;
                        present.invalidate();
                        app.quick_pending = None;
                        hotkey_result = None;
                    }
                } else {
                    app.quick_pending = Some((tool, text));
                }
            }
            Event::UserEvent(Ev::OcrDone(id, result)) => {
                OCR_BUSY.store(false, Ordering::Release);
                if !app.quick_mode {
                    // The regular dashboard still receives exactly the original bridge result.
                    if let Some(id) = id {
                        let reply = result.map(|v| json!(v.map(|capture| capture.image)));
                        let _ = webview.evaluate_script(&bridge_result(id, reply));
                    }
                    want_reveal = shown;
                } else {
                    match result {
                        Ok(Some(capture)) => {
                            present.await_render(Instant::now());
                            app.anchored = true;
                            anchor_capture(app.hwnd, capture.bounds);
                            hotkey_result = Some(Ok(capture.image));
                            if let Some(id) = id {
                                // Button and hotkey use the same guarded presentation path.
                                let _ = webview.evaluate_script(&bridge_result(id, Ok(json!({"presenting":true}))));
                            }
                        }
                        Ok(None) => {
                            if let Some(id) = id {
                                let _ = webview.evaluate_script(&bridge_result(id, Ok(Value::Null)));
                            }
                            if app.quick_pending.is_none() {
                                if !shown { app.close(); }
                                hidden_for_picker = false;
                                want_reveal = true;
                            } else {
                                // A queued regular shortcut may still be waiting for WebView startup.
                                present.await_render(Instant::now());
                            }
                        }
                        Err(error) => {
                            if let Some(id) = id {
                                let _ = webview.evaluate_script(&bridge_result(id, Err(error)));
                                if app.quick_pending.is_none() {
                                    hidden_for_picker = false;
                                    want_reveal = shown;
                                } else {
                                    present.await_render(Instant::now());
                                }
                            } else {
                                present.await_render(Instant::now());
                                hotkey_result = Some(Err(error));
                            }
                        }
                    }
                }
            }
            Event::UserEvent(Ev::Done(id, r)) => {
                AI_BUSY.store(false, Ordering::Release);
                if id != 0 {
                    let js = bridge_result(id, r);
                    let _ = webview.evaluate_script(&js);
                }
            }
            Event::UserEvent(Ev::Call(id, m, a)) => {
                #[cfg(any(debug_assertions, feature = "e2e"))]
                if m == "__e2e_suspend_tao" && e2e.is_some() {
                    // Prove the watchdog can kill the helper with neither renderer beats
                    // nor dashboard event-loop progress. Never present in ordinary release.
                    std::thread::sleep(Duration::from_millis(4800));
                    return;
                }
                if m == "__quick_presented" {
                    if app.quick_mode
                        && !OCR_BUSY.load(Ordering::Acquire)
                        && app.quick_pending.is_none()
                        && present.acknowledge(a.first().and_then(Value::as_u64))
                    {
                        hidden_for_picker = false;
                        want_reveal = true;
                    }
                } else if m == "__drag" {
                    if !app.geo.maximized {
                        let _ = window.drag_window();
                    }
                } else if m == "__log" {
                    let msg = a.first().and_then(Value::as_str).unwrap_or("");
                    cu_core::log::log(&app.core.paths, &format!("dashboard js error: {}", msg.chars().take(300).collect::<String>()));
                } else if m == "__e2e" {
                    if let Some((_, out)) = &e2e {
                        let _ = std::fs::write(out, a.first().map_or("null".into(), Value::to_string));
                    }
                } else {
                    if m == "ocr_capture" {
                        if !present.awaiting_render() && start_capture(Some(id), &window, &proxy) {
                            if app.quick_mode {
                                hidden_for_picker = true;
                                want_reveal = false;
                                present.invalidate();
                                hotkey_result = None;
                            }
                        } else if id != 0 {
                            let _ = webview.evaluate_script(&bridge_result(id, Err("OCR đang chạy".into())));
                        }
                        return;
                    }
                    if matches!(m.as_str(), "local_models" | "local_chat") {
                        if m == "local_chat" {
                            let o = a.first().and_then(Value::as_object);
                            let too_big = o.and_then(|x| x.get("text")).and_then(Value::as_str).is_some_and(|s| s.len() > 32 * 1024)
                                || o.and_then(|x| x.get("image")).and_then(Value::as_str).is_some_and(|s| s.len() > api::MAX_IMAGE_DATA_URL_BYTES);
                            if too_big { if id != 0 { let _ = webview.evaluate_script(&format!("__cu_resolve({id},true,{})", json!({"code":"request_too_large","error":"Yêu cầu quá lớn"}))); } return; }
                        }
                        if AI_BUSY.swap(true, Ordering::AcqRel) {
                            if id != 0 { let _ = webview.evaluate_script(&format!("__cu_resolve({id},true,{})", json!({"code":"busy","error":"Local AI đang xử lý yêu cầu khác"}))); }
                            return;
                        }
                        let p = proxy.clone();
                        std::thread::spawn(move || {
                            let r = match m.as_str() {
                                "local_models" => Ok(api::local_models(a.first().and_then(Value::as_str).unwrap_or("http://127.0.0.1:11434"), a.get(1).and_then(Value::as_str).unwrap_or("ollama"))),
                                _ => {
                                    let o = a.first().and_then(Value::as_object);
                                    Ok(api::local_chat(o.and_then(|x| x.get("endpoint")).and_then(Value::as_str).unwrap_or("http://127.0.0.1:11434"), o.and_then(|x| x.get("backend")).and_then(Value::as_str).unwrap_or("ollama"), o.and_then(|x| x.get("model")).and_then(Value::as_str).unwrap_or("hf.co/LiquidAI/LFM2.5-VL-450M-GGUF:Q8_0"), o.and_then(|x| x.get("task")).and_then(Value::as_str).unwrap_or("grammar"), o.and_then(|x| x.get("text")).and_then(Value::as_str).unwrap_or(""), o.and_then(|x| x.get("target")).and_then(Value::as_str), o.and_then(|x| x.get("image")).and_then(Value::as_str)))
                                }
                            };
                            let _ = p.send_event(Ev::Done(id, r));
                        });
                        return;
                    }
                    let r = app.call(&window, &m, &a);
                    if id != 0 {
                        let js = bridge_result(id, r);
                        let _ = webview.evaluate_script(&js);
                    }
                }
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => app.close(), // Alt+F4 / taskbar "close": same fast path as the ✕ button
            _ => {}
        }
        // A quick OCR panel can only open after the picker has closed. Keep it
        // hidden during capture, even if WebView initialization exceeds the timeout.
        let capturing = OCR_BUSY.load(Ordering::Acquire);
        app.live.tick(Instant::now());
        if app.ready && app.live_pending && !capturing {
            app.live_pending = false;
            let _ = webview.evaluate_script("toolTab('live')");
        }

        if app.ready && !capturing {
            if let Some((tool, text)) = app.quick_pending.take() {
                hotkey_result = None;
                app.anchored = false;
                // Text quick tools are also transient popups. Keep them in the
                // topmost band while the user moves back to the source window,
                // otherwise a normal external click immediately covers the
                // result and makes Grammar/Translate appear not to open.
                // OCR applies its explicit preference below when a capture is
                // actually revealed.
                win::set_topmost(app.hwnd, true);
                let mut js = format!("openQuickTool({},{});", json!(tool), json!(text));
                if hidden_for_picker {
                    let token = present.begin();
                    js.push_str(&format!("notifyQuickPresented({token});"));
                }
                let _ = webview.evaluate_script(&js);
                if shown && !hidden_for_picker { win::focus(app.hwnd); }
            } else if let Some(result) = hotkey_result.take() {
                let (image, error) = match result {
                    Ok(image) => (Some(image), None),
                    Err(error) => (None, Some(error)),
                };
                let token = present.begin();
                let js = format!("showQuickCapture({},{},{token});", json!(image), json!(error));
                let _ = webview.evaluate_script(&js);
            }
        }
        if !capturing && present.deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            // Fail closed if WebView never acknowledges. Never expose old or centered content.
            cu_core::log::log(&app.core.paths, "OCR popup render was not acknowledged");
            app.close();
        }
        if !capturing && !hidden_for_picker
            && (want_reveal || (!shown && (app.ready || started.elapsed() >= READY_TIMEOUT)))
        {
            shown = true;
            want_reveal = false;
            if e2e.is_none() || e2e_visible() {
                if app.quick_mode && app.anchored {
                    let topmost = cu_core::config::ocr_popup_always_on_top(&app.core.cfg());
                    win::set_topmost(app.hwnd, topmost);
                }
                window.set_visible(true);
                window.set_focus();
            }
        }
        if let Some((script, _)) = &e2e {
            if shown && !e2e_started {
                e2e_started = true;
                let body = std::fs::read_to_string(script).unwrap_or_default();
                let _ = webview.evaluate_script(&format!(
                    "(async()=>{{try{{const r=await (async()=>{{{body}\n}})();__cu_send('__e2e',[r])}}catch(e){{__cu_send('__e2e',[{{error:String(e)}}])}}}})()"
                ));
            }
        }
        if !app.quick_mode && app.geo.persist_at.is_some_and(|t| Instant::now() >= t) {
            app.persist_now();
        }
        let mut next: Option<Instant> = app.geo.persist_at;
        if app.live.active_worker() {
            let t = Instant::now() + Duration::from_millis(25);
            next = Some(next.map_or(t, |n| n.min(t)));
        }
        if let Some(t) = present.deadline {
            next = Some(next.map_or(t, |n| n.min(t)));
        }
        if !shown && !capturing && !hidden_for_picker {
            let t = started + READY_TIMEOUT;
            next = Some(next.map_or(t, |n| n.min(t)));
        }
        *flow = next.map_or(ControlFlow::Wait, ControlFlow::WaitUntil);
    });
}

/// Test-only (debug builds / `--features e2e`): `CU_E2E_VISIBLE=1` shows the window even under `--e2e`, so a test
/// can drive it with the real mouse. Not compiled into the shipped release.
#[cfg(any(debug_assertions, feature = "e2e"))]
fn e2e_visible() -> bool {
    std::env::var_os("CU_E2E_VISIBLE").is_some()
}
#[cfg(not(any(debug_assertions, feature = "e2e")))]
fn e2e_visible() -> bool {
    false
}

/// Startup failure: say so (events.log + a message box), never crash silently.
fn fail(core: &Core, msg: &str) -> ! {
    cu_core::log::log(&core.paths, &format!("dashboard: {msg}"));
    win::message_box(msg);
    std::process::exit(1)
}

#[cfg(test)]
mod quick_presentation_tests {
    use super::{Instant, QuickPresentation, QUICK_PRESENT_TIMEOUT};

    #[test]
    fn only_the_latest_ready_token_can_reveal_the_popup_once() {
        let mut present = QuickPresentation::default();
        assert!(!present.acknowledge(None));
        let first = present.begin();
        present.invalidate(); // presentation was abandoned
        assert!(!present.acknowledge(Some(first)));
        let second = present.begin();
        assert_ne!(first, second);
        assert!(!present.acknowledge(Some(first)));
        assert!(present.acknowledge(Some(second)));
        assert!(!present.acknowledge(Some(second)));
    }

    #[test]
    fn another_picker_is_blocked_between_acceptance_and_render_ack() {
        let mut present = QuickPresentation::default();
        let accepted_at = Instant::now();
        assert!(!present.awaiting_render());
        present.await_render(accepted_at);
        // A repeat before WebView is ready cannot start a picker that may cancel.
        assert!(present.awaiting_render());
        assert!(!present.acknowledge(None));
        let token = present.begin();
        assert!(present.awaiting_render());
        assert_eq!(present.deadline, Some(accepted_at + QUICK_PRESENT_TIMEOUT));
        assert!(!present.acknowledge(Some(token + 1)));
        assert!(present.awaiting_render());
        assert!(present.acknowledge(Some(token)));
        assert!(!present.awaiting_render()); // OCR model inference need not have finished
        assert_eq!(present.deadline, None);
    }
}

#[cfg(test)]
mod live_navigation_tests {
    use super::*;

    #[test]
    fn replacement_revokes_old_nonce_and_denied_navigation_only_revokes_lease() {
        let mut page = None;
        let mut lease = None;
        revoke_page_for_navigation(&mut page, &mut lease, bundled_document(DASHBOARD_DOCUMENT_URL));
        assert!(page.is_none() && lease.is_none(), "initial bundled navigation starts without a nonce");

        let old = "8e4de8659de000dadb17df7800a17dae";
        page = Some(old.to_owned());
        lease = Some(Instant::now());
        revoke_page_for_navigation(&mut page, &mut lease, bundled_document("https://sig4a.invalid/"));
        assert_eq!(page.as_deref(), Some(old), "blocked navigation leaves the same document");
        assert!(lease.is_none());

        lease = Some(Instant::now());
        revoke_page_for_navigation(&mut page, &mut lease, bundled_document(DASHBOARD_DOCUMENT_URL));
        assert_ne!(page.as_deref(), Some(old), "old-document heartbeats cannot match after reload");
        assert!(page.is_none() && lease.is_none());
        assert!(!bundled_document("about:blank"));
        assert!(!bundled_document("data:text/html,foreign"));
        assert!(!bundled_document("https://sig4a-bundled.localhost/other"));
        assert!(!bundled_document("https://sig4a-bundled.localhost/dashboard.html?foreign"));
    }
}
