//! Synthetic UI-test helper only. It never downloads, extracts, checks real files, or opens audio.
use std::{
    fs,
    io::{self, BufRead, Write},
};

fn send(v: String) {
    let mut out = io::stdout().lock();
    writeln!(out, "{v}").unwrap();
    out.flush().unwrap();
}
fn event(session: u64, state: &str) -> String {
    format!(r#"{{"v":1,"type":"model_setup","session":{session},"state":"{state}"}}"#)
}
fn main() {
    fs::write("model-helper.pid", std::process::id().to_string()).unwrap();
    let Some(line) = io::stdin().lock().lines().next().and_then(Result::ok) else {
        return;
    };
    if !line.contains(r#""v":1"#) || !line.contains(r#""type":"model_setup_start""#) {
        return;
    }
    let Some(session) = line
        .split_once(r#""session":"#)
        .and_then(|(_, rest)| rest.split(|c: char| !c.is_ascii_digit()).next())
        .and_then(|s| s.parse::<u64>().ok())
    else {
        return;
    };
    let action = if line.contains(r#""action":"verify""#) {
        "verify"
    } else if line.contains(r#""action":"install""#) {
        "install"
    } else {
        return;
    };
    let _ = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("model-helper.commands")
        .and_then(|mut f| writeln!(f, "{action} {session}"));
    if action == "verify" {
        send(event(session, "checking_files"));
        send(event(session, "ready"));
        return;
    }
    if action != "install" {
        return;
    }
    let count = fs::read_to_string("model-install-count")
        .ok()
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0)
        + 1;
    let _ = fs::write("model-install-count", count.to_string());
    // Mirror the real helper: every install first checks for an idempotent existing model.
    send(event(session, "checking_files"));
    send(format!(
        r#"{{"v":1,"type":"model_setup","session":{session},"state":"downloading","received":42,"total":310414022}}"#
    ));
    let cancelled = io::stdin()
        .lock()
        .lines()
        .next()
        .and_then(Result::ok)
        .is_some_and(|cancel| {
            cancel.contains(r#""type":"model_setup_cancel""#)
                && cancel.contains(&format!(r#""session":{session}"#))
        });
    if !cancelled {
        return;
    }
    if count == 1 {
        send(event(session, "cancelled"));
        return;
    }
    // This retry models a cancel competing with a commit that already won. The
    // later stages are still queued when the dashboard is showing Cancelling.
    for stage in [
        "checking_archive",
        "extracting",
        "checking_files",
        "committing",
        "ready",
    ] {
        send(event(session, stage));
    }
}
