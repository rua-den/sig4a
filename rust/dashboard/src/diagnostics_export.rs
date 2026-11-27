//! Privacy boundary for the local copy-only diagnostics report.
use crate::setup;
use serde_json::{json, Value};

const MAX_BYTES: usize = 4096;

fn safe_version(version: &str) -> &str {
    let mut parts = version.split('.');
    for _ in 0..3 {
        let Some(part) = parts.next() else {
            return "unknown";
        };
        if part.is_empty()
            || part.len() > 5
            || !part.bytes().all(|b| b.is_ascii_digit())
            || (part.len() > 1 && part.starts_with('0'))
        {
            return "unknown";
        }
    }
    if parts.next().is_some() {
        "unknown"
    } else {
        version
    }
}

/// Unlike dashboard state, this touches only compile-time constants and a nonblocking
/// read of this process's historical setup projection. It never refreshes anything.
pub fn preview() -> Result<Value, String> {
    let setup = setup::export_history();
    let report = json!({
        "schema_version": 1,
        "app": {
            "version": safe_version(env!("CARGO_PKG_VERSION")),
            "platform": if cfg!(windows) { "windows" } else { "unknown" },
            "arch": match std::env::consts::ARCH {
                "x86_64" => "x86_64",
                "aarch64" => "aarch64",
                "x86" => "x86",
                _ => "unknown",
            },
            "build": if cfg!(debug_assertions) { "debug" } else { "release" },
        },
        "local_ai_setup": {
            "scope": "last_recorded_in_this_dashboard_process",
            "stage": setup.stage.label(),
            "issue": setup.issue.label(),
        },
    });
    if serde_json::to_vec(&report).is_ok_and(|bytes| bytes.len() <= MAX_BYTES) {
        Ok(report)
    } else {
        Err("diagnostics_unavailable".into())
    }
}

pub fn has_exact_empty_args(envelope: &Value) -> bool {
    envelope
        .get("a")
        .and_then(Value::as_array)
        .is_some_and(Vec::is_empty)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_ascii_bounded_numeric_only_and_does_not_echo() {
        for version in ["0.0.0", "1.2.3", "12345.2.99999"] {
            assert_eq!(safe_version(version), version);
        }
        for hostile in [
            "012.3.4",
            "1.2",
            "1..2",
            "1.2.3.4",
            "1.2.3\n",
            "1.2.3+CANARY",
            "1.2.3-secret",
            "١.2.3",
            "C:\\Users\\CANARY",
            "100000.2.3",
        ] {
            assert_eq!(safe_version(hostile), "unknown");
        }
    }

    #[test]
    fn preview_has_exact_key_order_and_no_dependency_on_caller_state() {
        let report = preview().unwrap();
        let keys = |v: &Value| v.as_object().unwrap().keys().cloned().collect::<Vec<_>>();
        assert_eq!(keys(&report), ["schema_version", "app", "local_ai_setup"]);
        assert_eq!(
            keys(&report["app"]),
            ["version", "platform", "arch", "build"]
        );
        assert_eq!(keys(&report["local_ai_setup"]), ["scope", "stage", "issue"]);
        assert!(report.to_string().len() < MAX_BYTES);
    }

    #[test]
    fn raw_arguments_must_be_present_and_really_an_empty_array() {
        for denied in [
            json!({}),
            json!({"a":null}),
            json!({"a":{}}),
            json!({"a":""}),
            json!({"a":[null]}),
            json!({"a":["CANARY"]}),
        ] {
            assert!(!has_exact_empty_args(&denied));
        }
        assert!(has_exact_empty_args(&json!({"a":[]})));
    }
}
