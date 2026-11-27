//! Exact Windows owner-bound diagnostic transport. Unlike regular Local AI, this code
//! accepts neither an HTTP destination nor a prompt from the platform bridge.
use crate::setup_trust::{self, ListenerOwner, VerifiedOllama};
use serde::{
    de::{self, MapAccess, SeqAccess, Visitor},
    Deserialize, Deserializer,
};
use serde_json::{json, Map, Value};
use std::{
    fmt,
    io::{self, BufRead, BufReader, Read, Write},
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::NetworkManagement::IpHelper::MIB_TCPROW_OWNER_PID;

const TARGET: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 11434);
const GEMMA: &str = "gemma4:e2b";
const LOCAL_REQUEST_MODEL: &str = "gemma4:e2b:local";
const METADATA_TIMEOUT: Duration = Duration::from_secs(5);
#[cfg(test)]
const MODEL_TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(1);
const OWNER_RECHECK: Duration = Duration::from_millis(100);
const IO_POLL: Duration = Duration::from_millis(200);
const HEADER_LIMIT: usize = 4096;
const STATUS_LIMIT: usize = 4096;
const INVENTORY_LIMIT: usize = 128 * 1024;
const INVENTORY_ROWS: usize = 256;
const CHAT_LIMIT: usize = 8192;
const SAMPLE_UTF16_LIMIT: usize = 512;
const MAX_OUTPUT_TOKENS: u64 = 96;

const TRANSLATION: &str = "Translate this one sentence to Vietnamese, preserving the robot, ownership, count and color. Return only the Vietnamese translation.\n\nThe robot has two blue boxes.";
const GRAMMAR: &str = "Correct the English grammar in this one sentence without changing its meaning. Return only the corrected sentence.\n\nEvery morning the robot carry two blue boxes.";

type Result<T> = std::result::Result<T, &'static str>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Operation {
    Refresh,
    Translation,
    Grammar,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum Output {
    Inventory { listed: bool },
    Sample(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Request {
    Status,
    Inventory,
    ChatTranslation,
    ChatGrammar,
}

impl Request {
    fn wire(self) -> Vec<u8> {
        let (verb, path, prompt) = match self {
            Self::Status => ("GET", "/api/status", None),
            Self::Inventory => ("GET", "/api/tags", None),
            Self::ChatTranslation => ("POST", "/api/chat", Some(TRANSLATION)),
            Self::ChatGrammar => ("POST", "/api/chat", Some(GRAMMAR)),
        };
        let mut headers = format!("{verb} {path} HTTP/1.1\r\nHost: 127.0.0.1:11434\r\nAccept: application/json\r\nConnection: close\r\n");
        // Bounded, compile-time prompts only; no endpoint, headers, model or data in IPC.
        if let Some(prompt) = prompt {
            let body = json!({"model":LOCAL_REQUEST_MODEL, "stream":false, "think":false,
                "options":{"temperature":0,"num_predict":MAX_OUTPUT_TOKENS},
                "messages":[{"role":"user","content":prompt}]})
            .to_string();
            headers.push_str(&format!(
                "Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            ));
        } else {
            headers.push_str("\r\n");
        }
        headers.into_bytes()
    }

    fn body_limit(self) -> usize {
        match self {
            Self::Status => STATUS_LIMIT,
            Self::Inventory => INVENTORY_LIMIT,
            _ => CHAT_LIMIT,
        }
    }
}

fn remaining(deadline: Instant, active: &AtomicBool) -> Result<Duration> {
    if !active.load(Ordering::Acquire) {
        return Err("cancelled");
    }
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        Err("timeout")
    } else {
        Ok(left)
    }
}

struct Peer {
    owner: ListenerOwner,
    _file: VerifiedOllama,
    creation: u64,
}

impl Peer {
    fn open(deadline: Instant, active: &AtomicBool) -> Result<Self> {
        remaining(deadline, active)?;
        let owner = setup_trust::diagnostic_listener_owner()
            .map_err(|_| "unverified_owner")?
            .ok_or("ollama_unreachable")?;
        let creation = owner.creation_identity().ok_or("unverified_owner")?;
        remaining(deadline, active)?;
        let file = setup_trust::resolve_ollama_cache_only()
            .map_err(|_| "unverified_owner")?
            .ok_or("unverified_owner")?;
        remaining(deadline, active)?;
        if !owner.matches(&file) || owner.creation_identity() != Some(creation) {
            return Err("unverified_owner");
        }
        Ok(Self {
            owner,
            _file: file,
            creation,
        })
    }

    fn connect(&self, deadline: Instant, active: &AtomicBool) -> Result<PinnedConnection> {
        let budget = remaining(deadline, active)?.min(CONNECT_TIMEOUT);
        if self.owner.creation_identity() != Some(self.creation) {
            return Err("unverified_owner");
        }
        let stream =
            TcpStream::connect_timeout(&SocketAddr::V4(TARGET), budget).map_err(io_error)?;
        pin_connected(
            stream,
            TARGET,
            self.owner.pid(),
            deadline,
            active,
            || self.owner.creation_identity() == Some(self.creation),
            || setup_trust::connected_rows_v4().map_err(|_| "unverified_owner"),
        )
    }
}

fn pin_connected(
    stream: TcpStream,
    expected: SocketAddrV4,
    pid: u32,
    deadline: Instant,
    active: &AtomicBool,
    mut process_matches: impl FnMut() -> bool,
    mut connected_rows: impl FnMut() -> Result<Vec<MIB_TCPROW_OWNER_PID>>,
) -> Result<PinnedConnection> {
    let (Ok(SocketAddr::V4(local)), Ok(SocketAddr::V4(peer))) =
        (stream.local_addr(), stream.peer_addr())
    else {
        return Err("unverified_owner");
    };
    if peer != expected || *local.ip() != Ipv4Addr::LOCALHOST || local.port() == 0 {
        return Err("unverified_owner");
    }
    let until = deadline.min(Instant::now() + OWNER_RECHECK);
    loop {
        remaining(deadline, active)?;
        if !process_matches() {
            return Err("unverified_owner");
        }
        match owner_match(&connected_rows()?, local, peer, pid) {
            OwnerMatch::Pinned => {
                if !process_matches() {
                    return Err("unverified_owner");
                }
                remaining(deadline, active)?;
                return Ok(PinnedConnection(stream));
            }
            OwnerMatch::Missing if Instant::now() < until => {
                thread::sleep(Duration::from_millis(5))
            }
            _ => return Err("unverified_owner"),
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
enum OwnerMatch {
    Pinned,
    Missing,
    Conflict,
}

fn owner_match(
    rows: &[MIB_TCPROW_OWNER_PID],
    local: SocketAddrV4,
    peer: SocketAddrV4,
    pid: u32,
) -> OwnerMatch {
    const ESTABLISHED: u32 = 5;
    let mut reverse = rows.iter().filter(|row| {
        row.dwState == ESTABLISHED
            && row.dwLocalAddr.to_ne_bytes() == peer.ip().octets()
            && row.dwRemoteAddr.to_ne_bytes() == local.ip().octets()
            && u16::from_be(row.dwLocalPort as u16) == peer.port()
            && u16::from_be(row.dwRemotePort as u16) == local.port()
    });
    match (reverse.next(), reverse.next()) {
        (None, None) => OwnerMatch::Missing,
        (Some(row), None) if pid != 0 && row.dwOwningPid == pid => OwnerMatch::Pinned,
        _ => OwnerMatch::Conflict,
    }
}

fn io_error(error: io::Error) -> &'static str {
    match error.kind() {
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => "timeout",
        io::ErrorKind::UnexpectedEof | io::ErrorKind::InvalidData => "invalid_response",
        _ => "ollama_unreachable",
    }
}

struct PinnedConnection(TcpStream);

impl PinnedConnection {
    fn request(
        mut self,
        request: Request,
        deadline: Instant,
        active: &AtomicBool,
    ) -> Result<Value> {
        let wire = request.wire();
        let mut at = 0;
        while at < wire.len() {
            self.0
                .set_write_timeout(Some(remaining(deadline, active)?.min(IO_POLL)))
                .map_err(io_error)?;
            match self.0.write(&wire[at..]) {
                Ok(0) => return Err("ollama_unreachable"),
                Ok(n) => at += n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(io_error(e)),
            }
        }
        let body = read_http(
            BufReader::new(DeadlineReader {
                stream: self.0,
                deadline,
                active,
            }),
            request.body_limit(),
        )?;
        remaining(deadline, active)?;
        parse_unique_json(&body)
    }
}

struct DeadlineReader<'a> {
    stream: TcpStream,
    deadline: Instant,
    active: &'a AtomicBool,
}

impl Read for DeadlineReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            let left = remaining(self.deadline, self.active)
                .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))?;
            self.stream.set_read_timeout(Some(left.min(IO_POLL)))?;
            match self.stream.read(buf) {
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::TimedOut
                            | io::ErrorKind::WouldBlock
                            | io::ErrorKind::Interrupted
                    ) =>
                {
                    continue
                }
                result => return result,
            }
        }
    }
}

fn read_http(mut input: impl BufRead, limit: usize) -> Result<Vec<u8>> {
    let mut header = Vec::new();
    while !header.ends_with(b"\r\n\r\n") {
        let left = HEADER_LIMIT
            .checked_sub(header.len())
            .filter(|n| *n > 0)
            .ok_or("invalid_response")?;
        let mut line = (&mut input).take(left as u64);
        if line.read_until(b'\n', &mut header).map_err(io_error)? == 0 {
            return Err("invalid_response");
        }
    }
    let mut headers = [httparse::EMPTY_HEADER; 32];
    let mut response = httparse::Response::new(&mut headers);
    if !matches!(response.parse(&header), Ok(httparse::Status::Complete(n)) if n == header.len())
        || response.version != Some(1)
    {
        return Err("invalid_response");
    }
    if response.code != Some(200) {
        return Err("unsupported_response");
    }
    let mut length = None;
    let mut chunked = false;
    let mut json_content = false;
    for field in response.headers {
        if field.name.eq_ignore_ascii_case("Content-Length") {
            let value = std::str::from_utf8(field.value).map_err(|_| "invalid_response")?;
            if length.is_some() || value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
                return Err("invalid_response");
            }
            length = Some(value.parse::<usize>().map_err(|_| "invalid_response")?);
        } else if field.name.eq_ignore_ascii_case("Transfer-Encoding") {
            if chunked || !field.value.eq_ignore_ascii_case(b"chunked") {
                return Err("invalid_response");
            }
            chunked = true;
        } else if field.name.eq_ignore_ascii_case("Content-Type") {
            if json_content
                || !(field.value.eq_ignore_ascii_case(b"application/json")
                    || field
                        .value
                        .eq_ignore_ascii_case(b"application/json; charset=utf-8"))
            {
                return Err("invalid_response");
            }
            json_content = true;
        } else if field.name.eq_ignore_ascii_case("Content-Encoding")
            || field.name.eq_ignore_ascii_case("Trailer")
            || field.name.eq_ignore_ascii_case("Upgrade")
        {
            return Err("invalid_response");
        }
    }
    if !json_content {
        return Err("invalid_response");
    }
    match (length, chunked) {
        (Some(n), false) if n <= limit => {
            let mut body = vec![0; n];
            input.read_exact(&mut body).map_err(io_error)?;
            Ok(body)
        }
        (None, true) => read_chunks(input, limit),
        // No ambiguous or unframed bodies, even though a general HTTP client handles more.
        _ => Err("invalid_response"),
    }
}

fn read_chunks(mut input: impl BufRead, limit: usize) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    for _ in 0..512 {
        let mut line = Vec::new();
        let _ = (&mut input)
            .take(16)
            .read_until(b'\n', &mut line)
            .map_err(io_error)?;
        let line = line.strip_suffix(b"\r\n").ok_or("invalid_response")?;
        if line.is_empty() || !line.iter().all(|b| b.is_ascii_hexdigit()) {
            return Err("invalid_response");
        }
        let chunk = usize::from_str_radix(
            std::str::from_utf8(line).map_err(|_| "invalid_response")?,
            16,
        )
        .map_err(|_| "invalid_response")?;
        let start = body.len();
        let end = start
            .checked_add(chunk)
            .filter(|n| *n <= limit)
            .ok_or("invalid_response")?;
        body.resize(end, 0);
        input.read_exact(&mut body[start..end]).map_err(io_error)?;
        let mut delimiter = [0; 2];
        input.read_exact(&mut delimiter).map_err(io_error)?;
        if delimiter != *b"\r\n" {
            return Err("invalid_response");
        }
        if chunk == 0 {
            return Ok(body);
        }
    }
    Err("invalid_response")
}

/// JSON fields used in the trust gates must not take "first wins"/"last wins" semantics.
struct Unique(Value);
impl<'de> Deserialize<'de> for Unique {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct UniqueVisitor;
        impl<'de> Visitor<'de> for UniqueVisitor {
            type Value = Unique;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("one unambiguous JSON value")
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> std::result::Result<Unique, E> {
                Ok(Unique(Value::Bool(v)))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> std::result::Result<Unique, E> {
                Ok(Unique(v.into()))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<Unique, E> {
                Ok(Unique(v.into()))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> std::result::Result<Unique, E> {
                serde_json::Number::from_f64(v)
                    .map(Value::Number)
                    .map(Unique)
                    .ok_or_else(|| E::custom("invalid number"))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Unique, E> {
                Ok(Unique(v.into()))
            }
            fn visit_unit<E: de::Error>(self) -> std::result::Result<Unique, E> {
                Ok(Unique(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<Unique, A::Error> {
                let mut v = Vec::new();
                while let Some(Unique(item)) = a.next_element()? {
                    v.push(item);
                }
                Ok(Unique(Value::Array(v)))
            }
            fn visit_map<A: MapAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<Unique, A::Error> {
                let mut v = Map::new();
                while let Some((key, Unique(item))) = a.next_entry::<String, Unique>()? {
                    if v.insert(key, item).is_some() {
                        return Err(de::Error::custom("duplicate JSON field"));
                    }
                }
                Ok(Unique(Value::Object(v)))
            }
        }
        d.deserialize_any(UniqueVisitor)
    }
}

fn parse_unique_json(body: &[u8]) -> Result<Value> {
    serde_json::from_slice::<Unique>(body)
        .map(|v| v.0)
        .map_err(|_| "invalid_response")
}

fn cloud_disabled(status: &Value) -> Result<()> {
    let cloud = status
        .get("cloud")
        .and_then(Value::as_object)
        .ok_or("cloud_unverified")?;
    if cloud.get("disabled").and_then(Value::as_bool) == Some(true)
        && matches!(
            cloud.get("source").and_then(Value::as_str),
            Some("env" | "both")
        )
    {
        Ok(())
    } else {
        Err("cloud_unverified")
    }
}

fn inventory(inventory: &Value, exact: &str) -> Result<bool> {
    let rows = inventory
        .get("models")
        .and_then(Value::as_array)
        .filter(|rows| rows.len() <= INVENTORY_ROWS)
        .ok_or("invalid_response")?;
    let mut found = false;
    for row in rows {
        let row = row.as_object().ok_or("invalid_response")?;
        // Ollama returns both names in this audited schema.
        let name = row
            .get("name")
            .and_then(Value::as_str)
            .ok_or("invalid_response")?;
        let model = row
            .get("model")
            .and_then(Value::as_str)
            .ok_or("invalid_response")?;
        if name.eq_ignore_ascii_case(exact) || model.eq_ignore_ascii_case(exact) {
            if found
                || name != exact
                || model != exact
                || ["remote_host", "remote_model"]
                    .iter()
                    .any(|key| row.get(*key).is_some_and(|v| !v.is_null()))
            {
                return Err("unverified_alias");
            }
            found = true;
        }
    }
    Ok(found)
}

fn sample(response: &Value) -> Result<String> {
    let obj = response.as_object().ok_or("invalid_response")?;
    let message = obj
        .get("message")
        .and_then(Value::as_object)
        .ok_or("invalid_response")?;
    if obj.get("model").and_then(Value::as_str) != Some(LOCAL_REQUEST_MODEL)
        || obj.get("done").and_then(Value::as_bool) != Some(true)
        || obj.get("done_reason").and_then(Value::as_str) != Some("stop")
        || obj
            .get("eval_count")
            .is_some_and(|v| !v.as_u64().is_some_and(|n| n < MAX_OUTPUT_TOKENS))
        || ["remote_host", "remote_model", "error"]
            .iter()
            .any(|key| obj.get(*key).is_some_and(|v| !v.is_null()))
        || ["remote_host", "remote_model"]
            .iter()
            .any(|key| message.get(*key).is_some_and(|v| !v.is_null()))
        || message.get("role").and_then(Value::as_str) != Some("assistant")
        || ["tool_calls", "images"].iter().any(|key| {
            message
                .get(*key)
                .is_some_and(|v| !v.is_null() && !v.as_array().is_some_and(Vec::is_empty))
        })
        || message
            .get("thinking")
            .is_some_and(|v| !v.is_null() && v.as_str() != Some(""))
    {
        return Err("invalid_response");
    }
    let text = message
        .get("content")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| {
            !s.is_empty()
                && s.encode_utf16().count() <= SAMPLE_UTF16_LIMIT
                && !s.chars().any(|c| {
                    (c.is_control() && !matches!(c, '\r' | '\n' | '\t'))
                        || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
                })
        })
        .ok_or("invalid_response")?;
    Ok(text.to_owned())
}

fn admitted(operation: Operation, selection: &str, eligible: bool) -> bool {
    eligible
        && !selection.is_empty()
        && selection.len() <= 256
        && !selection
            .chars()
            .any(|c| c.is_control() || c.is_whitespace())
        && !selection.to_ascii_lowercase().ends_with(":cloud")
        && !selection.to_ascii_lowercase().ends_with("-cloud")
        && (operation == Operation::Refresh || selection == GEMMA)
}

#[cfg(test)]
fn run(
    operation: Operation,
    selection: &str,
    eligible: bool,
    active: &AtomicBool,
) -> Result<Output> {
    let deadline = Instant::now()
        + if operation == Operation::Refresh {
            METADATA_TIMEOUT
        } else {
            MODEL_TIMEOUT
        };
    run_checked(operation, selection, eligible, active, deadline, &|| Ok(()))
}

pub(crate) fn run_checked(
    operation: Operation,
    selection: &str,
    eligible: bool,
    active: &AtomicBool,
    deadline: Instant,
    before_chat: &impl Fn() -> Result<()>,
) -> Result<Output> {
    if !admitted(operation, selection, eligible) {
        return Err("unsupported_selection");
    }
    let peer = Peer::open(deadline, active)?;
    run_pinned(operation, selection, |request| {
        let request_deadline = if matches!(request, Request::Status | Request::Inventory) {
            deadline.min(Instant::now() + METADATA_TIMEOUT)
        } else {
            deadline
        };
        let pinned = peer.connect(request_deadline, active)?;
        if matches!(request, Request::ChatTranslation | Request::ChatGrammar) {
            before_chat()?;
        }
        let reply = pinned.request(request, request_deadline, active);
        remaining(request_deadline, active)?;
        if peer.owner.creation_identity() != Some(peer.creation) {
            return Err("unverified_owner");
        }
        reply
    })
}

fn run_pinned(
    operation: Operation,
    selection: &str,
    mut request: impl FnMut(Request) -> Result<Value>,
) -> Result<Output> {
    if operation == Operation::Refresh {
        return inventory(&request(Request::Inventory)?, selection)
            .map(|listed| Output::Inventory { listed });
    }
    let status = request(Request::Status).map_err(|code| {
        if matches!(code, "unsupported_response" | "invalid_response") {
            "cloud_unverified"
        } else {
            code
        }
    })?;
    cloud_disabled(&status)?;
    if !inventory(&request(Request::Inventory)?, GEMMA)? {
        return Err("model_missing");
    }
    let reply = request(match operation {
        Operation::Translation => Request::ChatTranslation,
        Operation::Grammar => Request::ChatGrammar,
        Operation::Refresh => unreachable!(),
    })?;
    sample(&reply).map(Output::Sample)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn tags() -> Value {
        json!({"models":[{"name":GEMMA,"model":GEMMA}]})
    }
    fn status(source: &str) -> Value {
        json!({"cloud":{"disabled":true,"source":source}})
    }
    fn chat() -> Value {
        json!({"model":LOCAL_REQUEST_MODEL,"done":true,"done_reason":"stop",
        "message":{"role":"assistant","content":"fixture"},"eval_count":10})
    }

    #[test]
    fn transport_requests_are_exact_and_do_not_allow_dynamic_model_or_message() {
        for (op, prompt) in [
            (Request::ChatGrammar, GRAMMAR),
            (Request::ChatTranslation, TRANSLATION),
        ] {
            let bytes = String::from_utf8(op.wire()).unwrap();
            let (header, body) = bytes.split_once("\r\n\r\n").unwrap();
            assert!(header.starts_with("POST /api/chat HTTP/1.1\r\nHost: 127.0.0.1:11434\r\n"));
            assert!(header.ends_with(&format!("Content-Length: {}", body.len())));
            assert_eq!(
                serde_json::from_str::<Value>(body).unwrap(),
                json!({
                "model":"gemma4:e2b:local","stream":false,"think":false,
                "options":{"temperature":0,"num_predict":96},"messages":[{"role":"user","content":prompt}]})
            );
        }
        assert!(Request::Status
            .wire()
            .starts_with(b"GET /api/status HTTP/1.1\r\n"));
        assert!(Request::Inventory
            .wire()
            .starts_with(b"GET /api/tags HTTP/1.1\r\n"));
        assert_eq!(
            run(Operation::Refresh, GEMMA, false, &AtomicBool::new(true)),
            Err("unsupported_selection")
        );
        assert_eq!(
            run(
                Operation::Grammar,
                "gemma4:e2b:local",
                true,
                &AtomicBool::new(true)
            ),
            Err("unsupported_selection")
        );
        assert_eq!(
            run(Operation::Translation, GEMMA, true, &AtomicBool::new(false)),
            Err("cancelled")
        );
    }

    #[test]
    fn refresh_sends_only_inventory_and_wrong_scope_does_not_start_transport() {
        let mut sent = Vec::new();
        assert_eq!(
            run_pinned(Operation::Refresh, GEMMA, |req| {
                sent.push(req);
                Ok(tags())
            }),
            Ok(Output::Inventory { listed: true })
        );
        assert_eq!(sent, [Request::Inventory]);
        for (op, alias) in [
            (Operation::Refresh, "text:cloud"),
            (Operation::Grammar, "wrong"),
        ] {
            assert!(!admitted(op, alias, true));
        }
    }

    #[test]
    fn samples_fail_before_post_on_proof_cloud_tag_or_listener_change() {
        for cloud in [
            status("config"),
            status(""),
            json!({"cloud":{"disabled":false,"source":"env"}}),
        ] {
            let mut sent = Vec::new();
            assert_eq!(
                run_pinned(Operation::Translation, GEMMA, |req| {
                    sent.push(req);
                    Ok(cloud.clone())
                }),
                Err("cloud_unverified")
            );
            assert_eq!(sent, [Request::Status]);
        }
        let mut sent = Vec::new();
        assert_eq!(
            run_pinned(Operation::Grammar, GEMMA, |req| {
                sent.push(req);
                if req == Request::Status {
                    Ok(status("env"))
                } else {
                    Err("unverified_owner")
                }
            }),
            Err("unverified_owner")
        );
        assert_eq!(sent, [Request::Status, Request::Inventory]);
        for (op, request) in [
            (Operation::Grammar, Request::ChatGrammar),
            (Operation::Translation, Request::ChatTranslation),
        ] {
            let mut sent = Vec::new();
            let res = run_pinned(op, GEMMA, |req| {
                sent.push(req);
                Ok(match req {
                    Request::Status => status("both"),
                    Request::Inventory => tags(),
                    _ => chat(),
                })
            });
            assert_eq!(res, Ok(Output::Sample("fixture".into())));
            assert_eq!(sent, [Request::Status, Request::Inventory, request]);
        }
    }

    #[test]
    fn strict_unique_local_inventory_and_assistant_stop_only() {
        assert_eq!(inventory(&tags(), GEMMA), Ok(true));
        assert_eq!(inventory(&json!({"models":[]}), GEMMA), Ok(false));
        assert_eq!(
            inventory(
                &json!({"models":[{"name":"Gemma4:e2b","model":GEMMA}]}),
                GEMMA
            ),
            Err("unverified_alias")
        );
        assert_eq!(
            inventory(
                &json!({"models":[{"name":GEMMA,"model":GEMMA},{"name":GEMMA,"model":GEMMA}]}),
                GEMMA
            ),
            Err("unverified_alias")
        );
        assert_eq!(
            inventory(
                &json!({"models":[{"name":GEMMA,"model":GEMMA,"remote_host":"present"}]}),
                GEMMA
            ),
            Err("unverified_alias")
        );
        for (pointer, value) in [
            ("/model", json!(GEMMA)),
            ("/done", json!(false)),
            ("/done_reason", json!("length")),
            ("/eval_count", json!(96)),
            ("/message/role", json!("user")),
            ("/message/content", json!("🤖".repeat(257))),
        ] {
            let mut data = chat();
            *data.pointer_mut(pointer).unwrap() = value;
            assert_eq!(sample(&data), Err("invalid_response"));
        }
        let mut data = chat();
        data["message"]["tool_calls"] = json!([{"function":"never"}]);
        assert_eq!(sample(&data), Err("invalid_response"));
        let mut data = chat();
        data["remote_model"] = json!("unexpected");
        assert_eq!(sample(&data), Err("invalid_response"));
    }

    #[test]
    fn framed_http_and_duplicates_fail_closed() {
        assert_eq!(read_http(Cursor::new(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}"),8).unwrap(),b"{}");
        assert_eq!(read_http(Cursor::new(b"HTTP/1.1 200 OK\r\nContent-Type: application/json; charset=utf-8\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n0\r\n\r\n"),8).unwrap(),b"{}");
        for msg in [
            "HTTP/1.1 307 Found\r\nLocation: https://example.invalid\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 10\r\n\r\n{}",
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nContent-Length: 2\r\n\r\n{}",
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nTransfer-Encoding: chunked\r\n\r\n{}",
        ] {
            assert!(read_http(Cursor::new(msg),8).is_err());
        }
        assert!(parse_unique_json(
            br#"{"cloud":{"disabled":false,"disabled":true,"source":"env"}}"#
        )
        .is_err());
        assert!(
            parse_unique_json(br#"{"cloud":{"disabled":true,"source":"env"},"cloud":{}}"#).is_err()
        );
        assert!(parse_unique_json(br#"{"models":[{"name":"one","name":"two"}]}"#).is_err());
        assert!(parse_unique_json(br#"{}{}"#).is_err());
        assert_eq!(
            parse_unique_json(br#"{"cloud":{"disabled":true,"source":"env"}}"#).unwrap(),
            status("env")
        );
    }

    fn row(local: SocketAddrV4, peer: SocketAddrV4, pid: u32) -> MIB_TCPROW_OWNER_PID {
        MIB_TCPROW_OWNER_PID {
            dwState: 5,
            dwLocalAddr: u32::from_ne_bytes(local.ip().octets()),
            dwLocalPort: local.port().to_be() as u32,
            dwRemoteAddr: u32::from_ne_bytes(peer.ip().octets()),
            dwRemotePort: peer.port().to_be() as u32,
            dwOwningPid: pid,
        }
    }

    #[test]
    fn only_exact_unique_server_side_reverse_tuple_matches() {
        let client = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 49123);
        let unrelated = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 49124);
        assert_eq!(
            owner_match(&[row(client, TARGET, 441)], client, TARGET, 441),
            OwnerMatch::Missing
        );
        assert_eq!(
            owner_match(&[row(TARGET, unrelated, 441)], client, TARGET, 441),
            OwnerMatch::Missing
        );
        assert_eq!(
            owner_match(&[row(TARGET, client, 442)], client, TARGET, 441),
            OwnerMatch::Conflict
        );
        assert_eq!(
            owner_match(
                &[row(TARGET, client, 441), row(TARGET, client, 441)],
                client,
                TARGET,
                441
            ),
            OwnerMatch::Conflict
        );
        assert_eq!(
            owner_match(
                &[row(client, TARGET, 441), row(TARGET, client, 441)],
                client,
                TARGET,
                441
            ),
            OwnerMatch::Pinned
        );
    }

    #[test]
    fn mismatched_owner_or_reused_process_closes_exact_socket_before_http() {
        use std::net::TcpListener;
        for (row_pid, died_after_snapshot) in [(43, false), (42, true)] {
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
            let SocketAddr::V4(server) = listener.local_addr().unwrap() else {
                panic!()
            };
            let stream = TcpStream::connect(server).unwrap();
            let (mut accepted, address) = listener.accept().unwrap();
            let SocketAddr::V4(client) = address else {
                panic!()
            };
            let mut identity_checks = 0;
            let result = pin_connected(
                stream,
                server,
                42,
                Instant::now() + Duration::from_secs(1),
                &AtomicBool::new(true),
                || {
                    identity_checks += 1;
                    !died_after_snapshot || identity_checks == 1
                },
                || Ok(vec![row(server, client, row_pid)]),
            );
            assert!(matches!(result, Err("unverified_owner")));
            accepted
                .set_read_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            assert_eq!(
                accepted.read(&mut [0; 128]).unwrap(),
                0,
                "no HTTP bytes on failed proof"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn actual_windows_tcp_table_has_connected_reverse_tuple() {
        use std::net::TcpListener;
        // Ephemeral loopback only; no privilege or official executable is required for
        // the kernel tuple proof test and no target/process identity is spoofed.
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let SocketAddr::V4(server) = listener.local_addr().unwrap() else {
            panic!()
        };
        let stream = TcpStream::connect(server).unwrap();
        let (_accepted, address) = listener.accept().unwrap();
        let SocketAddr::V4(client) = address else {
            panic!()
        };
        let limit = Instant::now() + Duration::from_secs(1);
        let active = AtomicBool::new(true);
        let conn = pin_connected(
            stream,
            server,
            std::process::id(),
            limit,
            &active,
            || true,
            || setup_trust::connected_rows_v4().map_err(|_| "unverified_owner"),
        )
        .ok()
        .unwrap();
        let rows = setup_trust::connected_rows_v4().unwrap();
        assert_eq!(owner_match(&rows, client, server, 0), OwnerMatch::Conflict);
        drop(conn);
    }
}
