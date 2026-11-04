use chrono::DateTime;
use chrono::Utc;
use std::collections::HashMap;

use crate::native::Host;
use crate::native::NativeInput;

pub fn normalize(
    input: &NativeInput,
    now: DateTime<Utc>,
) -> Result<lam_core::NormalizedEvent, lam_core::LamError> {
    let event_type = normalize_event_type(input)?;
    let cwd = match input.host {
        Host::Codex | Host::ClaudeCode | Host::Hermes => input.get_str(&["cwd"]),
        Host::OpenCode => input.get_str(&["cwd", "directory", "worktree"]),
    }
    .ok_or_else(|| {
        lam_core::LamError::validation(
            "E_CWD_MISSING",
            "hook input carries no working directory".to_string(),
        )
    })?;
    let external_session_id = session_identity(input);
    let timestamp = parse_timestamp(input).unwrap_or(now);
    let branch = input
        .get_str(&["git_branch", "branch"])
        .or_else(|| input.nested_str("payload", "branch"));
    let (payload, event_id) = build_payload(input, &event_type, &external_session_id.0, timestamp);
    let sanitized = crate::redact::sanitize_payload(payload);
    let mut final_payload = sanitized.value;
    if sanitized.redactions > 0 {
        final_payload.insert(
            "redactions".to_string(),
            serde_json::json!(sanitized.redactions),
        );
    }
    if external_session_id.1 {
        final_payload.insert("session_synthetic".to_string(), serde_json::json!(true));
    }
    let mut event = lam_core::NormalizedEvent {
        schema_version: 1,
        host: input.host.as_str().to_string(),
        event_type,
        external_session_id: external_session_id.0,
        event_id,
        cwd,
        git_branch: branch,
        timestamp,
        payload: final_payload,
    };
    if event.payload.len() > 64 {
        let keys: Vec<String> = event.payload.keys().cloned().collect();
        for key in keys.iter().skip(64) {
            event.payload.remove(key);
        }
        event
            .payload
            .insert("fields_truncated".to_string(), serde_json::json!(true));
    }
    event.validate()?;
    Ok(event)
}

fn normalize_event_type(input: &NativeInput) -> Result<String, lam_core::LamError> {
    let mapped = match input.host {
        Host::Codex => match input.event.as_str() {
            "SessionStart" => "session.start",
            "SessionEnd" => "session.end",
            "PreCompact" | "PostCompact" => "session.checkpoint",
            "UserPromptSubmit" => "prompt.submit",
            "Stop" => "session.stop",
            "PreToolUse" | "PostToolUse" => "tool.use",
            "PermissionRequest" => "tool.permission",
            "SubagentStart" => "agent.subagent_start",
            "SubagentStop" => "agent.subagent_stop",
            "Interrupt" => "session.interrupt",
            _ => "agent.unknown",
        },
        Host::ClaudeCode => match input.event.as_str() {
            "SessionStart" => "session.start",
            "SessionEnd" => "session.end",
            "PreCompact" => "session.checkpoint",
            "UserPromptSubmit" => "prompt.submit",
            "Stop" => "session.stop",
            "PreToolUse" | "PostToolUse" => "tool.use",
            "Notification" => "agent.notification",
            "SubagentStart" => "agent.subagent_start",
            "SubagentStop" => "agent.subagent_stop",
            _ => "agent.unknown",
        },
        Host::OpenCode => match input.event.as_str() {
            "session.created" => "session.start",
            "session.compacted" | "experimental.session.compacting" => "session.checkpoint",
            "session.idle" => "session.idle",
            "tool.execute.after" | "file.edited" => "tool.use",
            "chat.message" => "prompt.submit",
            "session.updated" => "agent.session_updated",
            _ => "agent.unknown",
        },
        Host::Hermes => match input.event.as_str() {
            "on_session_start" => "session.start",
            "on_session_end" | "on_session_reset" => "session.end",
            "on_session_finalize" => "session.checkpoint",
            "pre_llm_call" => "prompt.submit",
            "post_llm_call" | "subagent_stop" => "session.stop",
            "pre_tool_call" | "post_tool_call" => "tool.use",
            _ => "agent.unknown",
        },
    };
    Ok(mapped.to_string())
}

fn session_identity(input: &NativeInput) -> (String, bool) {
    let keys: &[&str] = match input.host {
        Host::Codex | Host::ClaudeCode | Host::Hermes => &["session_id", "parent_session_id"],
        Host::OpenCode => &["sessionId", "session_id", "sessionID"],
    };
    if let Some(found) = input.get_str(keys) {
        return (found, false);
    }
    let digest = blake3::hash(
        serde_json::to_string(&input.value)
            .unwrap_or_default()
            .as_bytes(),
    )
    .to_hex()
    .to_string();
    (format!("synth-{}", &digest[..16]), true)
}

fn parse_timestamp(input: &NativeInput) -> Option<DateTime<Utc>> {
    input
        .get_str(&["timestamp"])
        .and_then(|text| text.parse::<DateTime<Utc>>().ok())
}

fn short_hash(text: &str) -> String {
    let digest = blake3::hash(text.as_bytes()).to_hex().to_string();
    digest[..16].to_string()
}

fn response_text_of(input: &NativeInput) -> Option<String> {
    if let Some(text) = input.get_str(&["tool_response"]) {
        return Some(text);
    }
    if input.host == Host::Hermes {
        if let Some(text) = input.get_str(&["result", "output"]) {
            return Some(text);
        }
    }
    let from_payload = input
        .nested_str("payload", "output")
        .or_else(|| input.nested_str("payload", "result"))
        .or_else(|| input.nested_str("payload", "error"));
    if from_payload.is_some() {
        return from_payload;
    }
    let object = input.value.get("tool_response").filter(|v| v.is_object())?;
    let mut parts: Vec<String> = Vec::new();
    for key in ["output", "result", "stdout", "stderr", "error", "message"] {
        if let Some(text) = object.get(key).and_then(|v| v.as_str()) {
            if !text.trim().is_empty() {
                parts.push(text.trim().to_string());
            }
        }
    }
    if let Some(code) = object.get("exit_code").and_then(|v| v.as_i64()) {
        parts.push(format!("exit_code: {code}"));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    }
}

fn build_payload(
    input: &NativeInput,
    event_type: &str,
    session_id: &str,
    timestamp: DateTime<Utc>,
) -> (HashMap<String, serde_json::Value>, String) {
    let mut payload = HashMap::new();
    let stamp = timestamp.to_rfc3339();
    let event_id = match event_type {
        "session.start" => {
            let source = input
                .get_str(&["source"])
                .unwrap_or_else(|| "startup".to_string());
            payload.insert("source".to_string(), serde_json::json!(source));
            if let Some(model) = input.get_str(&["model"]) {
                payload.insert("model".to_string(), serde_json::json!(model));
            }
            format!("start-{}", short_hash(&format!("{session_id}|{source}")))
        }
        "session.end" => {
            let reason = input
                .get_str(&["reason"])
                .unwrap_or_else(|| "other".to_string());
            payload.insert("reason".to_string(), serde_json::json!(reason));
            format!("end-{}", short_hash(&format!("{session_id}|{reason}")))
        }
        "session.checkpoint" => {
            let trigger = input
                .get_str(&["trigger"])
                .or_else(|| input.nested_str("payload", "trigger"))
                .unwrap_or_else(|| "manual".to_string());
            payload.insert("trigger".to_string(), serde_json::json!(trigger));
            for key in [
                "goal",
                "current_task",
                "completed_steps",
                "next_steps",
                "blockers",
                "files_touched",
            ] {
                if let Some(found) = input
                    .get_str(&[key])
                    .map(serde_json::Value::String)
                    .or_else(|| input.nested_value("payload", key))
                    .or_else(|| input.value.get(key).cloned())
                {
                    if !found.is_null() {
                        payload.insert(key.to_string(), found);
                    }
                }
            }
            let discriminator = payload
                .get("current_task")
                .and_then(|v| v.as_str())
                .unwrap_or(&trigger);
            format!(
                "checkpoint-{}",
                short_hash(&format!("{session_id}|{discriminator}|{stamp}"))
            )
        }
        "prompt.submit" => {
            let prompt = input
                .get_str(&["prompt", "user_message", "message"])
                .or_else(|| input.nested_str("payload", "prompt"))
                .or_else(|| input.nested_str("extra", "user_message"))
                .unwrap_or_default();
            payload.insert(
                "prompt_hash".to_string(),
                serde_json::json!(short_hash(&prompt)),
            );
            let excerpt: String = prompt.chars().take(500).collect();
            if !excerpt.trim().is_empty() {
                payload.insert("prompt_excerpt".to_string(), serde_json::json!(excerpt));
            }
            format!("prompt-{}", short_hash(&format!("{session_id}|{prompt}")))
        }
        "session.stop" => {
            if let Some(message) = input.get_str(&[
                "last_assistant_message",
                "assistant_response",
                "response",
                "child_summary",
            ]) {
                payload.insert(
                    "last_message_excerpt".to_string(),
                    serde_json::json!(message),
                );
            }
            let turn = input.get_str(&["turn_id"]).unwrap_or_default();
            format!(
                "stop-{}",
                short_hash(&format!("{session_id}|{turn}|{stamp}"))
            )
        }
        "tool.use" => {
            let tool_name = input
                .get_str(&["tool_name"])
                .or_else(|| input.nested_str("payload", "tool"))
                .or_else(|| input.nested_str("tool_input", "command"))
                .unwrap_or_else(|| "unknown".to_string());
            payload.insert("tool_name".to_string(), serde_json::json!(tool_name));
            if let Some(tool_input) = input.value.get("tool_input") {
                payload.insert("tool_input".to_string(), tool_input.clone());
            }
            if let Some(response) = response_text_of(input) {
                payload.insert("tool_response".to_string(), serde_json::json!(response));
            }
            let use_id = input.get_str(&["tool_use_id"]).unwrap_or_else(|| {
                let canonical = serde_json::to_string(&input.value).unwrap_or_default();
                format!("use-{}", short_hash(&canonical))
            });
            format!("tool-{use_id}")
        }
        "session.idle" => {
            for key in ["notes", "summary"] {
                if let Some(found) = input
                    .get_str(&[key])
                    .or_else(|| input.nested_str("payload", key))
                {
                    payload.insert(key.to_string(), serde_json::json!(found));
                }
            }
            let canonical = serde_json::to_string(&input.value).unwrap_or_default();
            format!("idle-{}", short_hash(&format!("{session_id}|{canonical}")))
        }
        _ => {
            payload.insert("native_event".to_string(), serde_json::json!(input.event));
            let canonical = serde_json::to_string(&input.value).unwrap_or_default();
            format!("other-{}", short_hash(&format!("{session_id}|{canonical}")))
        }
    };
    (payload, event_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::Host;
    use crate::native::NativeInput;

    fn fixture(name: &str) -> Vec<u8> {
        let base = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("fixtures")
            .join("hooks")
            .join(name);
        std::fs::read(&base).unwrap_or_else(|_| panic!("missing fixture {name}"))
    }

    fn at() -> DateTime<Utc> {
        "2026-11-02T00:00:00Z".parse().expect("fixed time")
    }

    #[test]
    fn codex_start_maps_with_source() {
        let raw = fixture("codex-sessionstart.json");
        let input = NativeInput::parse(Host::Codex, &raw).expect("parse");
        let event = normalize(&input, at()).expect("normalize");
        assert_eq!(event.event_type, "session.start");
        assert_eq!(event.host, "codex");
        assert_eq!(
            event.payload.get("source").and_then(|v| v.as_str()),
            Some("startup")
        );
    }

    #[test]
    fn claude_end_maps_reason() {
        let raw = fixture("claude-sessionend.json");
        let input = NativeInput::parse(Host::ClaudeCode, &raw).expect("parse");
        let event = normalize(&input, at()).expect("normalize");
        assert_eq!(event.event_type, "session.end");
        assert_eq!(event.external_session_id, "abc123");
    }

    #[test]
    fn checkpoint_keeps_working_fields() {
        let raw = fixture("claude-precompact.json");
        let input = NativeInput::parse(Host::ClaudeCode, &raw).expect("parse");
        let event = normalize(&input, at()).expect("normalize");
        assert_eq!(event.event_type, "session.checkpoint");
        assert!(event.payload.contains_key("current_task"));
        assert!(event.payload.contains_key("blockers"));
    }

    #[test]
    fn secret_payload_is_scrubbed() {
        let raw = fixture("claude-prompt-with-secrets.json");
        let input = NativeInput::parse(Host::ClaudeCode, &raw).expect("parse");
        let event = normalize(&input, at()).expect("normalize");
        let serialized = serde_json::to_string(&event.payload).expect("json");
        assert!(!serialized.contains("ghp_fixture0000000000000000000000000000"));
        assert!(serialized.contains("REDACTED"));
    }

    #[test]
    fn transcript_blob_is_dropped() {
        let raw = fixture("codex-posttooluse-transcript.json");
        let input = NativeInput::parse(Host::Codex, &raw).expect("parse");
        let event = normalize(&input, at()).expect("normalize");
        assert!(!event.payload.contains_key("transcript"));
        assert!(event.payload.contains_key("tool_name"));
    }

    #[test]
    fn missing_session_gets_marked_synthetic() {
        let raw = fixture("missing-session.json");
        let input = NativeInput::parse(Host::OpenCode, &raw).expect("parse");
        let event = normalize(&input, at()).expect("normalize");
        assert!(event.external_session_id.starts_with("synth-"));
        assert_eq!(
            event
                .payload
                .get("session_synthetic")
                .and_then(|v| v.as_bool()),
            Some(true)
        );
    }

    #[test]
    fn unknown_fields_never_fabricate() {
        let raw = fixture("unknown-fields.json");
        let input = NativeInput::parse(Host::ClaudeCode, &raw).expect("parse");
        let event = normalize(&input, at()).expect("normalize");
        assert_eq!(event.event_type, "session.stop");
        assert!(!event.payload.contains_key("model"));
    }

    #[test]
    fn hermes_pre_llm_maps_to_prompt_submit() {
        let raw = fixture("hermes-pre-llm.json");
        let input = NativeInput::parse(Host::Hermes, &raw).expect("parse");
        let event = normalize(&input, at()).expect("normalize");
        assert_eq!(event.event_type, "prompt.submit");
        assert_eq!(event.host, "hermes");
        assert_eq!(event.external_session_id, "hermes-sess-1");
        let excerpt = event
            .payload
            .get("prompt_excerpt")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert!(excerpt.contains("Temporal provisioning"));
    }

    #[test]
    fn hermes_post_tool_keeps_terminal_evidence() {
        let raw = fixture("hermes-post-tool.json");
        let input = NativeInput::parse(Host::Hermes, &raw).expect("parse");
        let event = normalize(&input, at()).expect("normalize");
        assert_eq!(event.event_type, "tool.use");
        assert_eq!(
            event.payload.get("tool_name").and_then(|v| v.as_str()),
            Some("terminal")
        );
        let response = event
            .payload
            .get("tool_response")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert!(response.contains("FAILED"));
    }

    #[test]
    fn hermes_session_start_maps() {
        let raw = fixture("hermes-session-start.json");
        let input = NativeInput::parse(Host::Hermes, &raw).expect("parse");
        let event = normalize(&input, at()).expect("normalize");
        assert_eq!(event.event_type, "session.start");
        assert_eq!(event.external_session_id, "hermes-sess-1");
    }

    #[test]
    fn hermes_post_llm_maps_to_stop_with_excerpt() {
        let raw = fixture("hermes-post-llm.json");
        let input = NativeInput::parse(Host::Hermes, &raw).expect("parse");
        let event = normalize(&input, at()).expect("normalize");
        assert_eq!(event.event_type, "session.stop");
        let excerpt = event
            .payload
            .get("last_message_excerpt")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert!(excerpt.contains("staging credentials"));
    }

    #[test]
    fn hermes_subagent_stop_uses_parent_session() {
        let raw = fixture("hermes-subagent-stop.json");
        let input = NativeInput::parse(Host::Hermes, &raw).expect("parse");
        let event = normalize(&input, at()).expect("normalize");
        assert_eq!(event.event_type, "session.stop");
        assert_eq!(event.external_session_id, "hermes-sess-1");
        let excerpt = event
            .payload
            .get("last_message_excerpt")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert!(excerpt.contains("Vitastor"));
    }
}
