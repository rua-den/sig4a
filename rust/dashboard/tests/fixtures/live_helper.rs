//! Synthetic test child only. Never compiled by the package/build script; no audio capture.
use std::io::{self, BufRead, Write};
use std::fs::{self, OpenOptions};

fn marker(line: &str) {
    let mut out = OpenOptions::new().append(true).create(true).open("commands.fixture").unwrap();
    writeln!(out, "{line}").unwrap();
}

fn send(s: String) {
    let mut o = io::stdout().lock();
    writeln!(o, "{s}").unwrap();
    o.flush().unwrap();
}
fn state(n: u64, s: &str) {
    send(format!(r#"{{"v":1,"type":"status","session":{n},"state":"{s}"}}"#));
}
fn caption(n: u64, s: &str) {
    send(format!(r#"{{"v":1,"type":"caption","session":{n},"en":"{s}","vi":"fixture VI"}}"#));
}
fn sequence(n: u64, seq: u64, en: &str, vi: &str) {
    send(format!(r#"{{"v":1,"type":"caption","session":{n},"en":"{en}","vi":{vi},"seq":{seq}}}"#));
}
fn main() {
    fs::write("pid.fixture", std::process::id().to_string()).unwrap();
    let mut hang = false;
    for s in io::stdin().lock().lines().map_while(Result::ok) {
        let id = s.split_once(r#""session":"#).map(|(_, n)| n.chars().take_while(char::is_ascii_digit).collect::<String>().parse::<u64>().unwrap()).unwrap_or(0);
        if s.contains(r#""type":"list""#) {
            marker("list");
            send(r#"{"v":1,"type":"devices","devices":[{"id":"speaker","name":"Fixture Speakers","source":"playback","is_default":true},{"id":"mic","name":"Fixture Microphone","source":"mic","is_default":true},{"id":"hang","name":"Fixture Timeout","source":"mic","is_default":false}]}"#.into());
        } else if s.contains(r#""type":"start""#) {
            marker(&format!("start {id}"));
            hang = s.contains(r#""device_id":"hang""#);
            state(id, "started");
            if id > 1 { caption(id - 1, "OLD GENERATION"); }
            if s.contains(r#""translate":true"#) {
                sequence(id, 3, "UNAUTHORISED FUTURE", r#""bad future""#);
                sequence(id, 1, "<b>fixture English</b>", "null");
                sequence(id, 1, "WRONG EN", r#""bad association""#);
                sequence(id, 1, "<b>fixture English</b>", r#""fixture VI""#);
                sequence(id, 1, "<b>fixture English</b>", "null");
            } else {
                caption(id, "<b>fixture English</b>");
            }
        } else if s.contains(r#""type":"stop""#) {
            marker(&format!("stop {id}"));
            if !hang {
                caption(id, "LATE BEFORE ACK");
                state(id, "stopped");
                caption(id, "LATE AFTER ACK");
            }
        } else if s.contains(r#""type":"check_start""#) || s.contains(r#""type":"check_stop""#) {
            // Record any breach of the fixture's regular-session-only boundary,
            // never acknowledge a check or open any audio.
            let kind = if s.contains(r#""type":"check_start""#) { "check_start" } else { "check_stop" };
            marker(&format!("{kind} {id}"));
        } else if s.contains(r#""type":"quit""#) {
            marker("quit");
            break;
        }
    }
}
