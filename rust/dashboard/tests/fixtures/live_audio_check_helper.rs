//! Synthetic dashboard bridge helper. No audio/model; never included in the package.
use std::{fs::{self, OpenOptions}, io::{self, BufRead, Write},
          sync::{Arc, atomic::{AtomicU64, Ordering}}, time::Duration};

fn write(line: &str) {
    let mut out=io::stdout().lock();
    writeln!(out,"{line}").unwrap();
    out.flush().unwrap();
}
fn check(id:u64,state:&str,completed:Option<bool>) {
    let ext=completed.map(|c|format!(r#","completed":{c}"#)).unwrap_or_default();
    write(&format!(r#"{{"v":1,"type":"check_status","session":{id},"state":"{state}"{ext}}}"#));
}
fn sample(id:u64, text:&str) {
    write(&format!(r#"{{"v":1,"type":"check_sample","session":{id},"en":"{text}"}}"#));
}
fn main() {
    fs::write("pid.fixture",std::process::id().to_string()).unwrap();
    let active=Arc::new(AtomicU64::new(0));
    let mut ignore_stop=false;
    for command in io::stdin().lock().lines().map_while(Result::ok) {
        let mut log=OpenOptions::new().append(true).create(true).open("commands.fixture").unwrap();
        writeln!(log,"{command}").unwrap();
        let id=command.split_once(r#""session":"#).map(|(_,n)|n.chars().take_while(char::is_ascii_digit)
            .collect::<String>().parse::<u64>().unwrap()).unwrap_or(0);
        if command.contains(r#""type":"list""#) {
            write(r#"{"v":1,"type":"devices","devices":[{"id":"silent","name":"Silent playback","source":"playback","is_default":true},{"id":"audible","name":"Audible playback","source":"playback","is_default":false},{"id":"no-model","name":"Audio without model","source":"playback","is_default":false},{"id":"pending-model","name":"Audio while model starts","source":"playback","is_default":false},{"id":"abort-hang","name":"Hung on Stop","source":"playback","is_default":false},{"id":"locked","name":"Unavailable playback","source":"playback","is_default":false},{"id":"hang","name":"Hung playback","source":"playback","is_default":false},{"id":"mic","name":"Microphone","source":"mic","is_default":true}]}"#);
        } else if command.contains(r#""type":"check_start""#) {
            let device=command.split_once(r#""device_id":""#).map(|(_,s)|s.split('"').next().unwrap().to_owned()).unwrap_or_default();
            if device=="locked" {
                write(&format!(r#"{{"v":1,"type":"error","session":{id},"code":"device_missing"}}"#));
                continue;
            }
            active.store(id,Ordering::Release);
            check(id,"started",None);
            if id>1 { sample(id-1,"STALE"); }
            ignore_stop=device=="abort-hang";
            if device=="hang" || ignore_stop { continue; }
            let signal=device!="silent";
            let level=if signal {42} else {0};
            let model=if device=="no-model" {"unavailable"} else {"ready"};
            if device!="pending-model" {
                write(&format!(r#"{{"v":1,"type":"check_model","session":{id},"state":"{model}"}}"#));
            }
            write(&format!(r#"{{"v":1,"type":"check_meter","session":{id},"level":{level},"signal":{signal}}}"#));
            if device=="mic" { sample(id,"<b>English sample</b>"); }
            let session=active.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(900));
                if session.compare_exchange(id,0,Ordering::AcqRel,Ordering::Acquire).is_ok() {
                    check(id,"stopped",Some(true));
                    sample(id,"LATE COMPLETION");
                }
            });
        } else if command.contains(r#""type":"check_stop""#) {
            if ignore_stop { continue; }
            active.store(0,Ordering::Release);
            sample(id,"LATE BEFORE ACK");
            check(id,"stopped",Some(false));
            sample(id,"LATE AFTER ACK");
        } else if command.contains(r#""type":"quit""#) { break; }
    }
}
