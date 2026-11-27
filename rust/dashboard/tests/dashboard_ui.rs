//! UI test of the real dashboard process (WebView2). Sandboxed LOCALAPPDATA/USERPROFILE, registry/hook writes
//! disabled, no real token, no network. The window stays hidden in this mode: a JS script (`--e2e`, only in debug
//! builds or with `--features e2e`) drives the real page and writes its
//! result to a file. Pixels are not inspected (needs human eyes); behaviour is.
//!
//! ONE test function on purpose (GUI tests must not run concurrently).
#![cfg(any(debug_assertions, feature = "e2e"))]

use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use std::io::Write;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW, WM_CLOSE};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn window() -> HWND {
    // SAFETY: NUL-terminated class name.
    unsafe { FindWindowW(wide("ClaudeUsageDashboard").as_ptr(), std::ptr::null()) }
}

struct Guard(Child);
impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn wait_for(what: &str, secs: u64, mut f: impl FnMut() -> bool) {
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(secs) {
        if f() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("timeout waiting for: {what}");
}

fn spawn(local: &Path, home: &Path, script: Option<(&Path, &Path)>) -> Guard {
    let mut c = Command::new(env!("CARGO_BIN_EXE_claude-usage-dashboard"));
    if let Some((s, o)) = script {
        c.arg("--e2e").arg(s).arg(o);
    }
    c.env("LOCALAPPDATA", local)
        .env("USERPROFILE", home)
        .env_remove("HOMEDRIVE")
        .env_remove("HOMEPATH")
        .env_remove("CLAUDE_CONFIG_DIR")
        .env("CU_NO_AUTOSTART", "1")
        .env("CU_NO_REPAIR", "1");
    Guard(c.spawn().expect("spawn dashboard"))
}

fn spawn_quick(local: &Path, home: &Path, script: (&Path, &Path), text: &str) -> Guard {
    let mut c = Command::new(env!("CARGO_BIN_EXE_claude-usage-dashboard"));
    c.arg("--quick-tool").arg("grammar").arg("--e2e").arg(script.0).arg(script.1)
        .stdin(Stdio::piped())
        .env("LOCALAPPDATA", local)
        .env("USERPROFILE", home)
        .env_remove("HOMEDRIVE")
        .env_remove("HOMEPATH")
        .env_remove("CLAUDE_CONFIG_DIR")
        .env("CU_NO_AUTOSTART", "1")
        .env("CU_NO_REPAIR", "1");
    let mut child = c.spawn().expect("spawn quick dashboard");
    child.stdin.take().unwrap().write_all(text.as_bytes()).unwrap();
    Guard(child)
}

fn run_script(local: &Path, home: &Path, dir: &Path, name: &str, body: &str) -> (Guard, Value) {
    let s = dir.join(format!("{name}.js"));
    let o = dir.join(format!("{name}.out"));
    fs::write(&s, body).unwrap();
    let mut g = spawn(local, home, Some((&s, &o)));
    let deadline = Instant::now() + Duration::from_secs(40);
    while !o.exists() && Instant::now() < deadline {
        if let Ok(Some(exit)) = g.0.try_wait() {
            panic!("{name} dashboard exited ({exit}); events: {}", fs::read_to_string(local.join("claude-usage/events.log")).unwrap_or_default());
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(o.exists(), "{name} script result missing; events: {}", fs::read_to_string(local.join("claude-usage/events.log")).unwrap_or_default());
    std::thread::sleep(Duration::from_millis(100)); // the file is written in one call; be safe
    let v = serde_json::from_str(&fs::read_to_string(&o).unwrap()).unwrap();
    (g, v)
}

const SCRIPT1: &str = r#"
window.__cu_send('__log',['dashboard_ui: SCRIPT1 started']);
await new Promise(r => setTimeout(r, 700));
toolTab('usage'); await new Promise(r => setTimeout(r, 100)); document.querySelector('.toolnav').style.display='none';
const $ = id => document.getElementById(id);
const out = {};
out.init = !!window.__INIT__;
out.accs = $('accs').innerText;
out.sub = $('sub').textContent;
out.hookst = $('hookst').textContent;
out.routes = [...document.querySelectorAll('.toolnav button')].map(b => b.dataset.tool);
out.ai_tools = [...document.querySelectorAll('.ai-subnav button')].map(b => b.dataset.ai);
toolTab('usage'); out.usage_visible = $('usage-accounts').getBoundingClientRect().height > 0 && $('settings-box').getBoundingClientRect().height === 0;
toolTab('cleaner'); out.cleaner_visible = $('clean').hidden === false && $('usage-accounts').getBoundingClientRect().height === 0;
toolTab('settings'); tab('gen'); const settingsGen = !$('gen').hidden && $('gen').getBoundingClientRect().height > 0; tab('look'); const settingsLook = !$('look').hidden && $('look').getBoundingClientRect().height > 0; out.settings_visible = settingsGen && settingsLook && $('settings-box').getBoundingClientRect().height > 0 && $('clean').hidden;
toolTab('localai'); out.localai_settings = document.querySelector('.toolnav button[data-tool="settings"]').getAttribute('aria-current') === 'page' && $('tool-localai').classList.contains('on') && getComputedStyle($('settings-box')).display === 'none' && getComputedStyle($('ai-subnav-wrap')).display === 'none'; toolTab('settings');
$('t-ocrpopup').click();
out.ocr_settings = {
  visible: !$('ocrpopup').hidden && $('ocrpopup').getBoundingClientRect().height > 0 && $('gen').hidden,
  active: $('t-ocrpopup').getAttribute('aria-pressed') === 'true',
  enabled_by_default: $('ocr-popup-topmost').checked
};
$('ocr-popup-topmost').click();
$('settings-save').click();
window.__cu_send('__log',['dashboard_ui: waiting for existing OCR save']);
await saving;
out.ocr_topmost_saved = (await api.state()).cfg.ocr_popup_always_on_top;
$('t-shortcuts').click();
out.shortcut_tab = !$('shortcuts').hidden && $('ocrpopup').hidden && $('t-shortcuts').getAttribute('aria-pressed')==='true';
out.shortcut_defaults = Object.fromEntries(['toolbox','grammar','translation','ocr'].map(k=>[k,$('shortcut-'+k).value]));
$('shortcut-toolbox').value='alt + ctrl + g';
window.__cu_send('__log',['dashboard_ui: waiting for duplicate']);
await saveShortcuts();
out.shortcut_duplicate = $('shortcuts-status').dataset.state;
$('shortcut-toolbox').value='Win+Q';
window.__cu_send('__log',['dashboard_ui: waiting for invalid']);
await saveShortcuts();
out.shortcut_invalid = $('shortcuts-status').dataset.state;

toolTab('ai'); document.querySelector('.toolnav button[data-tool="ai"]').focus(); document.querySelector('.toolnav button[data-tool="ai"]').dispatchEvent(new KeyboardEvent('keydown', {key:'ArrowRight', bubbles:true}));
out.keyboard_route = document.querySelector('.toolnav button.on').dataset.tool;
toolTab('overview');
window.__cu_send('__log',['dashboard_ui: waiting for original state']);
out.state = await api.state();
out.win0 = await api.win_state();
out.accent_initial = getComputedStyle(document.documentElement).getPropertyValue('--accent').trim();
out.fit_first = await api.fit(520);
out.rect_fit = await api.rect();
out.fit_narrow = await api.fit(420);
out.rect_narrow = await api.rect();
const titleBox = document.querySelector('.title').getBoundingClientRect();
const pickerBox = document.querySelector('.accent-picker').getBoundingClientRect();
out.title_layout = {titleHeight: titleBox.height, pickerTop: pickerBox.top, pickerBottom: pickerBox.bottom, titleBottom: titleBox.bottom, pickerRight: pickerBox.right, titleRight: titleBox.right};
out.dpr = window.devicePixelRatio;
// settings go through the page's own save(): fields -> api.save -> config.json
$('warn').value = '77';
$('interval').value = '30';
$('accent').value = 'purple'; $('accent').dispatchEvent(new Event('change', {bubbles:true})); await accentSaving;
$('wlabel').value = 'short';
$('wfont').value = '135';
await save(true);
out.accent_before_restart = getComputedStyle(document.documentElement).getPropertyValue('--accent').trim();
out.widget_text_before_restart = {label: $('wlabel').value, font: $('wfont').value};
await api.refresh();
await api.test_notify();
out.hook_install = await api.hook_install();
out.hook_status = await api.hook_status();
await refresh();                       // the page's refresh button path (also writes refresh.flag)
await api.widget_unlock();
// window geometry
await api.set_rect(50, 60, 900, 700);
out.rect_set = await api.rect();
out.win1 = await api.win_state();
out.fit_after_user = await api.fit(520);
await api.toggle_max();
out.win_max = await api.win_state();
out.wa = await api.work_area();
out.rect_max = await api.rect();
await api.toggle_max();
out.rect_back = await api.rect();
out.win_back = await api.win_state();
try { await api.no_such_method(); out.unknown = 'resolved'; } catch (e) { out.unknown = String(e); }
window.__cu_send('__log',['dashboard_ui: SCRIPT1 returning']);
return out;
"#;

const SCRIPT2: &str = r#"
await new Promise(r => setTimeout(r, 500));
document.querySelector('.toolnav button[data-tool="settings"]').click();
$('t-ocrpopup').click();
const ocr_topmost = $('ocr-popup-topmost').checked;
const out = { ocr_topmost, win: await api.win_state(), rect: await api.rect(), accent: getComputedStyle(document.documentElement).getPropertyValue('--accent').trim(), widget_label: $('wlabel').value, widget_font: $('wfont').value };
setTimeout(() => api.hide(), 50);       // the page's own close path
return out;
"#;

const SCRIPT_QUICK: &str = r#"
await new Promise(r => setTimeout(r, 700));
const out = {
  toolboxButton: $('quick-open-toolbox').textContent === 'Mở Tool Box' && getComputedStyle($('quick-open-toolbox')).display !== 'none',
  quick: !!window.__QUICK_MODE__ && document.body.classList.contains('quick'),
  grammar: $('tool-grammar').classList.contains('on'),
  payload: $('grammar-in').value,
  pickerHidden: getComputedStyle($('accent').closest('.accent-picker')).display === 'none',
  accent: getComputedStyle(document.documentElement).getPropertyValue('--accent').trim()
};
setTimeout(() => api.hide(), 50);
return out;
"#;

#[test]
fn dashboard_process_end_to_end() {
    assert!(
        window().is_null(),
        "a dashboard window is already open: close it before running this test"
    );
    let tmp = tempfile::tempdir().unwrap();
    let local = tmp.path().join("local");
    let home = tmp.path().join("home");
    let prof = tmp.path().join("prof");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&prof).unwrap();
    fs::write(prof.join("settings.json"), "{}").unwrap();
    let data = local.join("claude-usage");
    fs::create_dir_all(&data).unwrap();
    fs::write(
        data.join("config.json"),
        json!({"profiles": [{"name": "p1", "path": prof.to_string_lossy()}], "autostart": false, "widget_theme": "light", "accent_test_sentinel": "keep"})
            .to_string(),
    )
    .unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    fs::write(
        data.join("usage_cache.json"),
        json!({"ts": now, "block_enabled": true, "block_pct": 95, "warn_pct": 80, "busy": [], "refresh": null,
               "profiles": {"p1": {"path": prof.to_string_lossy(), "five": 42.0, "week": 7.0,
                   "five_reset": "2999-01-01T00:00:00+00:00", "week_reset": "2999-01-01T00:00:00+00:00",
                   "fetched_at": now, "retry_at": now + 180.0, "err": null}}})
        .to_string(),
    )
    .unwrap();

    // ---- run 1: render, save, flags, hooks, geometry, single instance, close by WM_CLOSE ----
    let (mut main, r) = run_script(&local, &home, tmp.path(), "s1", SCRIPT1);
    assert!(r.get("error").is_none(), "{r}");
    assert_eq!(r["init"], true, "state inlined into the page");
    assert_eq!(r["accent_initial"], "#fab283", "missing accent migrates to orange");
    assert!(r["accs"].as_str().unwrap().contains("p1"), "{r}");
    assert!(r["accs"].as_str().unwrap().contains("42%"), "{r}");
    assert_eq!(r["routes"], json!(["overview", "ai", "usage", "cleaner", "settings"]));
    assert_eq!(r["ai_tools"], json!(["grammar", "translation", "ocr"]));
    assert_eq!(r["usage_visible"], true);
    assert_eq!(r["cleaner_visible"], true);
    assert_eq!(r["settings_visible"], true);
    assert_eq!(r["localai_settings"], true);
    assert_eq!(r["ocr_settings"], json!({"visible":true,"active":true,"enabled_by_default":true}));
    assert_eq!(r["ocr_topmost_saved"], false);
    assert_eq!(r["shortcut_tab"], true);
    assert_eq!(r["shortcut_defaults"], json!({"toolbox":"","grammar":"Ctrl+Alt+G","translation":"Ctrl+Alt+T","ocr":"Ctrl+Alt+O"}));
    assert_eq!(r["shortcut_duplicate"], "duplicate");
    assert_eq!(r["shortcut_invalid"], "invalid");
    assert_eq!(r["keyboard_route"], "usage");
    assert!(r["sub"].as_str().unwrap().contains("1 accounts"), "{r}");
    assert_eq!(r["state"]["cfg"]["warn_pct"], 80);
    assert_eq!(r["state"]["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(r["hookst"], "hook: đã cài 0/1 profile");
    assert_eq!(r["win0"], json!({"user_sized": false, "maximized": false}));
    assert_eq!(r["fit_first"], true);
    assert!(r["title_layout"]["pickerTop"].as_f64().unwrap() >= r["title_layout"]["titleHeight"].as_f64().unwrap() * 0.0);
    assert!(r["title_layout"]["pickerBottom"].as_f64().unwrap() <= r["title_layout"]["titleBottom"].as_f64().unwrap() + 1.0, "picker clipped by title bar: {r}");
    assert!(r["title_layout"]["pickerRight"].as_f64().unwrap() <= r["title_layout"]["titleRight"].as_f64().unwrap() + 1.0, "picker overflows title bar: {r}");
    assert_eq!(
        r["rect_fit"][3].as_i64().unwrap(),
        (520.0 * r["dpr"].as_f64().unwrap()) as i64,
        "window followed the content height: {r}"
    );

    let cfg: Value =
        serde_json::from_str(&fs::read_to_string(data.join("config.json")).unwrap()).unwrap();
    assert_eq!(cfg["warn_pct"], 77, "save() from the page");
    assert_eq!(cfg["interval_sec"], 90, "interval floor");
    assert_eq!(cfg["widget_locked"], false, "widget_unlock");
    assert_eq!(cfg["dashboard_accent"], "purple", "accent persisted independently");
    assert_eq!(cfg["widget_label"], "short", "widget text setting preserved");
    assert_eq!(cfg["widget_font"], 135, "widget font setting preserved");
    assert_eq!(cfg["widget_theme"], "light", "widget theme remains independent");
    assert_eq!(cfg["ocr_popup_always_on_top"], false, "OCR popup preference persisted");
    assert_eq!(cfg["accent_test_sentinel"], "keep", "unknown config key preserved");
    assert!(data.join("refresh.flag").exists() && data.join("notify_test.flag").exists());
    assert_eq!(r["hook_install"], json!([["p1", "đã cài"]]));
    assert_eq!(r["hook_status"], json!([["p1", true]]));
    let st = fs::read_to_string(prof.join("settings.json")).unwrap();
    assert!(
        st.contains("claude-usage.exe") && st.contains("hook"),
        "{st}"
    );

    assert_eq!(r["rect_set"], json!([50, 60, 900, 700]));
    assert_eq!(r["win1"], json!({"user_sized": true, "maximized": false}));
    assert_eq!(
        r["fit_after_user"], false,
        "the user's size wins over auto fit"
    );
    assert_eq!(r["win_max"], json!({"user_sized": true, "maximized": true}));
    assert_eq!(r["rect_max"], r["wa"], "maximized = work area");
    assert_eq!(r["rect_back"], json!([50, 60, 900, 700]));
    assert_eq!(
        r["win_back"],
        json!({"user_sized": true, "maximized": false})
    );
    assert!(
        r["unknown"].as_str().unwrap().contains("unknown method"),
        "{r}"
    );

    // single instance: a second dashboard focuses the first and exits
    let h = window();
    assert!(!h.is_null(), "dashboard window class registered");
    let mut second = spawn(&local, &home, None);
    wait_for("second dashboard exits", 10, || {
        second.0.try_wait().unwrap().is_some()
    });
    assert!(main.0.try_wait().unwrap().is_none(), "first keeps running");

    // close request (Alt+F4 path): saves the size and ends the process
    // SAFETY: posting WM_CLOSE to our own test window.
    unsafe { PostMessageW(h, WM_CLOSE, 0, 0) };
    wait_for("process exit on close", 10, || {
        main.0.try_wait().unwrap().is_some()
    });
    let cfg: Value =
        serde_json::from_str(&fs::read_to_string(data.join("config.json")).unwrap()).unwrap();
    assert_eq!(
        cfg["dash_rect"],
        json!([50, 60, 900, 700]),
        "size remembered"
    );
    wait_for("window gone", 5, || window().is_null());

    // ---- run 2: the saved size is restored; the page's hide() ends the process ----
    let (mut main2, r2) = run_script(&local, &home, tmp.path(), "s2", SCRIPT2);
    assert_eq!(r2["ocr_topmost"], false, "OCR preference restored on dashboard restart");
    assert_eq!(
        r2["win"],
        json!({"user_sized": true, "maximized": false}),
        "{r2}"
    );
    assert_eq!(r2["rect"], json!([50, 60, 900, 700]), "{r2}");
    assert_eq!(r2["accent"], "#c7a0ff", "accent survives dashboard restart");
    assert_eq!(r2["widget_label"], "short", "widget text survives accent restart");
    assert_eq!(r2["widget_font"], "135", "widget font survives accent restart");
    wait_for("process exit on hide()", 10, || {
        main2.0.try_wait().unwrap().is_some()
    });
    wait_for("window gone", 5, || window().is_null());

    // ---- run quick hotkey popup with piped stdin: same persisted theme, real quick class and payload ----
    let quick_script = tmp.path().join("quick.js");
    let quick_out = tmp.path().join("quick.out");
    fs::write(&quick_script, SCRIPT_QUICK).unwrap();
    let mut quick = spawn_quick(
        &local,
        &home,
        (&quick_script, &quick_out),
        "sample grammar payload",
    );
    wait_for("quick script result", 40, || quick_out.exists());
    let quick_result: Value = serde_json::from_str(&fs::read_to_string(&quick_out).unwrap()).unwrap();
    assert_eq!(quick_result["quick"], true, "quick dashboard class");
    assert_eq!(quick_result["toolboxButton"], true, "quick dashboard class");
    assert_eq!(quick_result["grammar"], true, "quick opens grammar panel");
    assert_eq!(quick_result["payload"], "sample grammar payload", "quick stdin payload");
    assert_eq!(quick_result["pickerHidden"], true, "quick hides picker only");
    assert_eq!(quick_result["accent"], "#c7a0ff", "quick inherits persisted purple accent");
    wait_for("quick process exit", 10, || quick.0.try_wait().unwrap().is_some());
    wait_for("quick window gone", 5, || window().is_null());

    // ---- run 3: result actions follow the active OCR image (mock model, real WebView) ----
    let (ocr, result) = run_script(
        &local,
        &home,
        tmp.path(),
        "ocr",
        include_str!("js/ocr_result.js"),
    );
    assert_eq!(result, json!({"ocr_result_lifecycle": true}), "{result}");
    drop(ocr);
    wait_for("OCR test window gone", 5, || window().is_null());

    let (_text, result) = run_script(
        &local, &home, tmp.path(), "text_tools", include_str!("js/text_tools.js"),
    );
    assert_eq!(result, json!({"text_tools": true}), "{result}");

    let log = fs::read_to_string(data.join("events.log")).unwrap_or_default();
    assert!(
        !log.contains("js error") && !log.contains("dashboard:"),
        "{log}"
    );
    assert!(
        data.join("webview").exists(),
        "webview data dir under the sandboxed data dir"
    );
}
