#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Triage {
    pub store: bool,
    pub reason: &'static str,
}

pub fn triage(event: &lam_core::NormalizedEvent) -> Triage {
    match event.event_type.as_str() {
        "session.start" | "session.end" | "session.checkpoint" | "session.stop"
        | "session.idle" => Triage {
            store: true,
            reason: "session_lifecycle",
        },
        "prompt.submit" => Triage {
            store: true,
            reason: "user_intent",
        },
        "tool.use" => triage_tool(event),
        _ => Triage {
            store: false,
            reason: "transient_noise",
        },
    }
}

fn triage_tool(event: &lam_core::NormalizedEvent) -> Triage {
    let tool_name = event
        .payload
        .get("tool_name")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let response = event
        .payload
        .get("tool_response")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let input_text = event
        .payload
        .get("tool_input")
        .map(|v| serde_json::to_string(v).unwrap_or_default())
        .unwrap_or_default();
    let haystack = format!("{response}\n{input_text}").to_lowercase();
    if nonzero_exit(&haystack)
        || haystack.contains("fail")
        || haystack.contains("error")
        || haystack.contains("panic")
        || haystack.contains("non-zero")
        || haystack.contains("test result: failed")
    {
        return Triage {
            store: true,
            reason: "tool_failure",
        };
    }
    if matches!(tool_name, "apply_patch" | "Edit" | "Write") && !response.trim().is_empty() {
        return Triage {
            store: true,
            reason: "file_change",
        };
    }
    Triage {
        store: false,
        reason: "transient_tool",
    }
}

fn nonzero_exit(haystack: &str) -> bool {
    let mut search_from = 0;
    while let Some(relative) = haystack[search_from..].find("exit_code") {
        let mut cursor = search_from + relative + "exit_code".len();
        let bytes = haystack.as_bytes();
        while cursor < bytes.len() && !bytes[cursor].is_ascii_digit() {
            cursor += 1;
        }
        let mut digits = String::new();
        while cursor < bytes.len() && bytes[cursor].is_ascii_digit() {
            digits.push(bytes[cursor] as char);
            cursor += 1;
        }
        if !digits.is_empty() && !digits.trim_matches('0').is_empty() {
            return true;
        }
        search_from = cursor.max(search_from + 1);
        if search_from >= bytes.len() {
            break;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::Host;
    use crate::native::NativeInput;
    use crate::normalize::normalize;

    fn fixture(name: &str) -> Vec<u8> {
        let base = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("fixtures")
            .join("hooks")
            .join(name);
        std::fs::read(&base).unwrap_or_else(|_| panic!("missing fixture {name}"))
    }

    fn at() -> chrono::DateTime<chrono::Utc> {
        "2026-11-02T00:00:00Z".parse().expect("fixed time")
    }

    fn event_for(host: Host, name: &str) -> lam_core::NormalizedEvent {
        let raw = fixture(name);
        let input = NativeInput::parse(host, &raw).expect("parse");
        normalize(&input, at()).expect("normalize")
    }

    #[test]
    fn session_starts_are_stored() {
        let event = event_for(Host::Codex, "codex-sessionstart.json");
        let decision = triage(&event);
        assert!(decision.store);
    }

    #[test]
    fn prompts_are_stored_as_intent() {
        let event = event_for(Host::ClaudeCode, "claude-prompt-with-secrets.json");
        let decision = triage(&event);
        assert!(decision.store);
        assert_eq!(decision.reason, "user_intent");
    }

    #[test]
    fn failing_tools_are_stored() {
        let raw = serde_json::json!({
            "session_id": "s1",
            "cwd": "/workspace/repo",
            "hook_event_name": "PostToolUse",
            "tool_name": "Bash",
            "tool_use_id": "call_9",
            "tool_input": {"command": "cargo test"},
            "tool_response": {"exit_code": 1, "output": "test result: FAILED. 1 failed"}
        });
        let input = NativeInput::parse(Host::Codex, &serde_json::to_vec(&raw).expect("json"))
            .expect("parse");
        let event = normalize(&input, at()).expect("normalize");
        let decision = triage(&event);
        assert!(decision.store);
        assert_eq!(decision.reason, "tool_failure");
    }

    #[test]
    fn quiet_tools_are_dropped() {
        let event = event_for(Host::Codex, "codex-posttooluse-transcript.json");
        let decision = triage(&event);
        assert!(!decision.store);
    }
}
