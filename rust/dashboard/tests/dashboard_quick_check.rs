//! Actual hosted WebView2 route with isolated config and a never-authorized loopback listener.
#![cfg(debug_assertions)]
mod common;
use common::*;
use serde_json::json;
use std::{fs, net::TcpListener, path::Path};

#[test]
fn dashboard_quick_check_passive_refusal_and_generation() {
    assert!(window().is_null(), "close any dashboard before running");
    let fixture = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    fixture.set_nonblocking(true).unwrap();
    let unauthorized = format!("http://{}", fixture.local_addr().unwrap());
    let sb = Sandbox::new(&[]);
    let mut config = sb.cfg();
    config["local_ai_backend"] = json!("ollama");
    config["local_ai_endpoint"] = json!(unauthorized);
    config["local_ai_model"] = json!("gemma4:e2b");
    config["local_ai_managed"] = json!(false);
    config["notify_enabled"] = json!(true);
    let cfg_file = sb.data.join("config.json");
    fs::write(&cfg_file, config.to_string()).unwrap();
    let saved = fs::read(&cfg_file).unwrap();
    let dash = Path::new(env!("CARGO_BIN_EXE_claude-usage-dashboard"));
    let mut run = Run::start_from_visibility(
        dash,
        &sb.local,
        &sb.home,
        sb.root.path(),
        "quick_check",
        include_str!("js/quick_check.js"),
        "",
        true,
    );
    for stage in ["opened-passive", "refused-live-transport"] {
        run.stage(stage);
        run.rust_check(
            &format!("{stage}: unauthorized address receives no connection"),
            matches!(fixture.accept(),Err(ref e) if e.kind()==std::io::ErrorKind::WouldBlock),
            "",
        );
        run.rust_check(
            &format!("{stage}: config untouched and no notification test flag"),
            fs::read(&cfg_file).unwrap() == saved && !sb.data.join("notify_test.flag").exists(),
            "",
        );
        run.go(stage);
    }
    run.finish();
    run.rust_check(
        "no later connection to the unauthorized endpoint",
        matches!(fixture.accept(),Err(ref e) if e.kind()==std::io::ErrorKind::WouldBlock),
        "",
    );
    report(
        "Quick Check WebView2 production refusal + debug-only sample rendering fixture",
        &run.checks,
    );
}
