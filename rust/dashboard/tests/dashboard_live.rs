//! Real WebView2 and Windows supervised process fixture; no real audio or model.
#![cfg(any(debug_assertions, feature = "e2e"))]

mod common;
use common::*;
use std::{fs, path::Path, process::Command};
use windows_sys::Win32::{Foundation::{CloseHandle, HANDLE}, System::Threading::{OpenProcess, WaitForSingleObject}};

fn fixture_handle(pid_path: &Path) -> HANDLE {
    let pid: u32 = fs::read_to_string(pid_path).expect("fixture process marker").parse().unwrap();
    let handle = unsafe { OpenProcess(0x00100000, 0, pid) }; // SYNCHRONIZE
    assert!(!handle.is_null(), "fixture process not alive");
    handle
}

#[test]
fn dashboard_live_translate_protocol_and_windows_job() {
    assert!(window().is_null(), "close the running dashboard before testing");
    let sb = Sandbox::new(&[]);
    let dash = sb.root.path().join("claude-usage-dashboard.exe");
    fs::copy(env!("CARGO_BIN_EXE_claude-usage-dashboard"), &dash).unwrap();
    let helper_dir = sb.root.path().join("live-translate");
    let fixture = helper_dir.join("Sig4a.LiveTranslate.exe");
    let pid = helper_dir.join("pid.fixture");
    let protocol = helper_dir.join("commands.fixture");
    let mut run = Run::start_from_visibility(&dash, &sb.local, &sb.home, sb.root.path(), "live_translate",
        include_str!("js/live_translate.js"), "", true);
    run.stage("install");
    run.rust_check("the missing-helper audio check fails before launching anything",
        !helper_dir.exists() && !pid.exists() && !protocol.exists(), "");
    fs::create_dir(&helper_dir).unwrap();
    assert!(Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
        .arg("--edition=2021")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/live_helper.rs"))
        .arg("-o").arg(&fixture).status().expect("compile synthetic fixture").success());
    run.go("install");
    run.stage("opened");
    run.rust_check("no helper process runs just from opening the page", !pid.exists(), "");
    run.go("opened");
    run.stage("before-timeout");
    let first = fixture_handle(&pid);
    run.go("before-timeout");
    run.stage("after-timeout");
    run.rust_check("timeout killed/reaped the exact helper child", unsafe { WaitForSingleObject(first, 3000) } == 0, "");
    unsafe { CloseHandle(first); }
    run.go("after-timeout");
    run.stage("before-lease-loss");
    let stalling = fixture_handle(&pid);
    run.go("before-lease-loss");
    run.rust_check("native watchdog killed the helper before stalled dashboard loop could recover",
        unsafe { WaitForSingleObject(stalling, 4200) } == 0, "");
    unsafe { CloseHandle(stalling); }
    run.stage("before-hard-kill");
    let second = fixture_handle(&pid);
    let commands = fs::read_to_string(&protocol).unwrap_or_default();
    // The explicit missing-helper audio check above reserves generation 1 before
    // failing closed. The fixture only sees the six later regular operations.
    run.rust_check("no extra start from refresh, duplicate start, tab visit, or device change",
        commands.lines().filter(|l| l.starts_with("start ")).collect::<Vec<_>>() ==
            ["start 2", "start 3", "start 4", "start 5", "start 6", "start 7"], &commands);
    run.rust_check("no audio check reaches the installed helper on page visit or while paused",
        !commands.lines().any(|line| line.starts_with("check_")), &commands);
    run.guard.0.kill().unwrap(); // deliberately bypass graceful exit to prove kill-on-close
    run.guard.0.wait().unwrap();
    run.rust_check("hard dashboard termination killed the helper Job", unsafe { WaitForSingleObject(second, 3000) } == 0, "");
    unsafe { CloseHandle(second); }
    report("WebView2 Live Translate supervision; synthetic fixture", &run.checks);
}
