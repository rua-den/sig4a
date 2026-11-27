//! Drive the actual WebView2 DOM at normal and native-minimum widths.
#![cfg(any(debug_assertions, feature = "e2e"))]

mod common;
use common::*;

#[test]
fn dashboard_usage_layout_at_normal_and_minimum_widths() {
    assert!(window().is_null(), "close the running dashboard before testing");
    let names = vec!["normal".to_string()];
    let sb = Sandbox::new(&names);
    let mut run = Run::start(
        &sb.local, &sb.home, sb.root.path(), "usage_layout",
        include_str!("js/usage_layout.js"), "",
    );
    run.finish();
    report("real WebView2 Claude Usage layout", &run.checks);
}
