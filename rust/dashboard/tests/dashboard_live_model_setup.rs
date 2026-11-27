//! Synthetic WebView2 setup controls and fixed helper protocol; fake child has no network/assets/audio.
#![cfg(any(debug_assertions, feature = "e2e"))]
mod common;
use common::*;
use std::{fs, path::Path, process::Command, thread, time::Duration};
use windows_sys::Win32::{
    Foundation::HWND,
    UI::WindowsAndMessaging::{FindWindowW, SendMessageW, WM_COMMAND},
};

fn find_dialog(title: &str) -> HWND {
    unsafe { FindWindowW(std::ptr::null(), wide(title).as_ptr()) }
}
fn answer(dialog: HWND, button: usize) {
    unsafe {
        SendMessageW(dialog, WM_COMMAND, button, 0);
    }
}

#[test]
fn dashboard_english_model_setup_is_explicit_bounded_and_cancelable() {
    assert!(
        window().is_null(),
        "close the running dashboard before testing"
    );
    let sb = Sandbox::new(&[]);
    let dash = sb.root.path().join("claude-usage-dashboard.exe");
    fs::copy(env!("CARGO_BIN_EXE_claude-usage-dashboard"), &dash).unwrap();
    let helper_dir = sb.root.path().join("live-translate");
    let helper = helper_dir.join("Sig4a.LiveTranslate.exe");
    let pid = helper_dir.join("model-helper.pid");
    let commands = helper_dir.join("model-helper.commands");
    fs::create_dir_all(&helper_dir).unwrap();
    let status = Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
        .arg("--edition=2021")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/english_model_helper.rs"))
        .arg("-o")
        .arg(&helper)
        .status()
        .expect("compile synthetic model helper");
    assert!(status.success());
    let script = include_str!("js/live_model_setup.js");
    let mut run = Run::start_from_visibility(
        &dash,
        &sb.local,
        &sb.home,
        sb.root.path(),
        "live_model_setup",
        script,
        "",
        true,
    );
    run.stage("page-open");
    run.rust_check(
        "opening the Live page did not spawn the helper",
        !pid.exists(),
        "",
    );
    run.go("page-open");
    run.stage("verified");
    run.rust_check(
        "only explicit Verify reached the synthetic helper",
        fs::read_to_string(&commands)
            .unwrap_or_default()
            .lines()
            .next()
            == Some("verify 1"),
        "",
    );
    run.go("verified");
    // The consent dialog is a real native dialog. This test process only presses its documented No/Yes buttons.
    for (i, button) in [7usize, 6, 6].into_iter().enumerate() {
        let deadline = std::time::Instant::now() + Duration::from_secs(8);
        let dlg = loop {
            let h = find_dialog("Live Translate · English model");
            if !h.is_null() {
                break h;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "native model consent dialog not shown (attempt {i})"
            );
            thread::sleep(Duration::from_millis(20));
        };
        answer(dlg, button);
        if i == 0 {
            run.stage("declined");
            let lines = fs::read_to_string(&commands).unwrap_or_default();
            run.rust_check(
                "declining consent did not spawn another helper",
                lines.lines().count() == 1,
                &lines,
            );
            run.go("declined");
        } else if i == 1 {
            run.stage("cancelled");
            run.rust_check(
                "install start was sent only after the Yes dialog",
                fs::read_to_string(&commands)
                    .unwrap_or_default()
                    .lines()
                    .count()
                    == 2,
                "",
            );
            run.go("cancelled");
        }
    }
    run.finish();
    report(
        "WebView2 English model setup; synthetic helper",
        &run.checks,
    );
}
