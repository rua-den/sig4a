//! Real WebView2 audio check and Windows Job tests with a synthetic protocol child.
#![cfg(any(debug_assertions, feature = "e2e"))]
mod common;
use common::*;
use serde_json::Value;
use std::{fs, path::Path, process::Command};
use windows_sys::Win32::{Foundation::CloseHandle, System::Threading::{OpenProcess, WaitForSingleObject}};

#[test]
fn dashboard_audio_check_explicit_stale_safe_and_bounded() {
    assert!(window().is_null(), "close the running dashboard before testing");
    let sb = Sandbox::new(&[]);
    let dash = sb.root.path().join("claude-usage-dashboard.exe");
    fs::copy(env!("CARGO_BIN_EXE_claude-usage-dashboard"), &dash).unwrap();
    let dir = sb.root.path().join("live-translate");
    fs::create_dir(&dir).unwrap();
    assert!(Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
        .arg("--edition=2021")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/live_audio_check_helper.rs"))
        .arg("-o").arg(dir.join("Sig4a.LiveTranslate.exe")).status().unwrap().success());
    let mut run = Run::start_from_visibility(&dash, &sb.local, &sb.home, sb.root.path(), "audio_check",
        include_str!("js/live_audio_check.js"), "", true);
    run.stage("open");
    run.rust_check("no helper on opening the page", !dir.join("pid.fixture").exists(), "");
    run.go("open");
    run.stage("before-check-timeout");
    let pid: u32 = fs::read_to_string(dir.join("pid.fixture")).unwrap().parse().unwrap();
    let child = unsafe { OpenProcess(0x00100000, 0, pid) }; // SYNCHRONIZE
    assert!(!child.is_null(), "the exact synthetic helper is active before deadline");
    run.go("before-check-timeout");
    run.rust_check("the five-second audio window has a bounded native hard stop",
        unsafe { WaitForSingleObject(child, 6400) } == 0, "");
    unsafe { CloseHandle(child); }
    run.stage("after-check-timeout");
    run.go("after-check-timeout");
    run.finish();
    let commands: Vec<Value> = fs::read_to_string(dir.join("commands.fixture")).unwrap().lines()
        .map(|line| serde_json::from_str(line).unwrap()).collect();
    let checks: Vec<_> = commands.iter().filter(|row| row["type"] == "check_start").collect();
    let expected = [("playback","silent"),("playback","audible"),("playback","no-model"),("playback","pending-model"),("mic","mic"),
        ("playback","locked"),("playback","abort-hang"),("playback","hang"),("mic","mic"),("mic","mic")];
    run.rust_check("each explicit test starts exactly one chosen source/device and cannot request VI",
        checks.len() == expected.len() && checks.iter().zip(expected).all(|(row,(src,dev))|
            row["source"] == src && row["device_id"] == dev && row.get("translate").is_none() &&
            row.as_object().is_some_and(|object|object.len()==5)),
        format!("{checks:?}"));
    run.rust_check("regular capture never starts and aborts use their own wire command",
        !commands.iter().any(|row|row["type"]=="start") &&
        commands.iter().filter(|row|row["type"]=="check_stop").count()==4, format!("{commands:?}"));
    report("WebView2 dedicated audio check; synthetic fixture", &run.checks);
}
