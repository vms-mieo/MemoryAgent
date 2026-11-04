use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Host {
    Codex,
    ClaudeCode,
    OpenCode,
    Hermes,
}

impl Host {
    pub const ALL: [Host; 4] = [Host::Codex, Host::ClaudeCode, Host::OpenCode, Host::Hermes];

    pub fn as_str(&self) -> &'static str {
        match self {
            Host::Codex => "codex",
            Host::ClaudeCode => "claude-code",
            Host::OpenCode => "opencode",
            Host::Hermes => "hermes",
        }
    }

    pub fn binary_name(&self) -> &'static str {
        match self {
            Host::Codex => "codex",
            Host::ClaudeCode => "claude",
            Host::OpenCode => "opencode",
            Host::Hermes => "hermes",
        }
    }

    pub fn supported_events(&self) -> &'static [&'static str] {
        match self {
            Host::Codex => &[
                "SessionStart",
                "SessionEnd",
                "UserPromptSubmit",
                "PreToolUse",
                "PostToolUse",
                "PermissionRequest",
                "PreCompact",
                "PostCompact",
                "SubagentStart",
                "SubagentStop",
                "Stop",
                "Interrupt",
            ],
            Host::ClaudeCode => &[
                "SessionStart",
                "SessionEnd",
                "UserPromptSubmit",
                "PreToolUse",
                "PostToolUse",
                "PreCompact",
                "Stop",
                "Notification",
                "SubagentStart",
                "SubagentStop",
            ],
            Host::OpenCode => &[
                "session.created",
                "session.idle",
                "session.compacted",
                "session.updated",
                "tool.execute.after",
                "file.edited",
                "chat.message",
                "experimental.session.compacting",
            ],
            Host::Hermes => &[
                "pre_tool_call",
                "post_tool_call",
                "pre_llm_call",
                "post_llm_call",
                "on_session_start",
                "on_session_end",
                "on_session_finalize",
                "on_session_reset",
                "subagent_stop",
            ],
        }
    }
}

impl FromStr for Host {
    type Err = lam_core::LamError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim() {
            "codex" => Ok(Host::Codex),
            "claude" | "claude-code" | "claude_code" => Ok(Host::ClaudeCode),
            "opencode" | "open-code" => Ok(Host::OpenCode),
            "hermes" => Ok(Host::Hermes),
            _ => Err(lam_core::LamError::validation(
                "E_HOST_INVALID",
                format!("unknown host: {s}"),
            )),
        }
    }
}

#[derive(Debug, Clone)]
pub struct NativeInput {
    pub host: Host,
    pub event: String,
    pub value: serde_json::Value,
}

impl NativeInput {
    pub fn parse(host: Host, raw: &[u8]) -> Result<Self, lam_core::LamError> {
        if raw.len() > 1_048_576 {
            return Err(lam_core::LamError::validation(
                "E_HOOK_INPUT_TOO_LARGE",
                "hook input exceeds 1MiB".to_string(),
            ));
        }
        let value: serde_json::Value = serde_json::from_slice(raw).map_err(|e| {
            lam_core::LamError::validation(
                "E_HOOK_INPUT_INVALID",
                format!("hook input is not JSON: {e}"),
            )
        })?;
        let event = match host {
            Host::Codex | Host::ClaudeCode | Host::Hermes => value
                .get("hook_event_name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            Host::OpenCode => value
                .get("event")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        };
        if event.trim().is_empty() {
            return Err(lam_core::LamError::validation(
                "E_HOOK_EVENT_MISSING",
                "hook input carries no event name".to_string(),
            ));
        }
        if !host.supported_events().contains(&event.as_str()) {
            return Err(lam_core::LamError::validation(
                "E_HOOK_EVENT_UNKNOWN",
                format!("unsupported {} event: {event}", host.as_str()),
            ));
        }
        Ok(Self { host, event, value })
    }

    pub fn get_str(&self, keys: &[&str]) -> Option<String> {
        for key in keys {
            if let Some(text) = self.value.get(key).and_then(|v| v.as_str()) {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
            if self.host == Host::Hermes {
                if let Some(text) = self
                    .value
                    .get("extra")
                    .and_then(|e| e.get(key))
                    .and_then(|v| v.as_str())
                {
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        return Some(trimmed.to_string());
                    }
                }
            }
        }
        None
    }

    pub fn nested_str(&self, outer: &str, inner: &str) -> Option<String> {
        self.nested_value(outer, inner)
            .as_ref()
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    pub fn nested_value(&self, outer: &str, inner: &str) -> Option<serde_json::Value> {
        self.value.get(outer).and_then(|o| o.get(inner)).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Vec<u8> {
        let base = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("fixtures")
            .join("hooks")
            .join(name);
        std::fs::read(&base).unwrap_or_else(|_| panic!("missing fixture {name}"))
    }

    #[test]
    fn parses_codex_session_start() {
        let raw = fixture("codex-sessionstart.json");
        let input = NativeInput::parse(Host::Codex, &raw).expect("parse");
        assert_eq!(input.event, "SessionStart");
        assert!(input.get_str(&["session_id"]).is_some());
    }

    #[test]
    fn parses_host_aliases() {
        assert_eq!("claude".parse::<Host>().expect("claude"), Host::ClaudeCode);
        assert_eq!(
            "claude-code".parse::<Host>().expect("claude-code"),
            Host::ClaudeCode
        );
        assert_eq!("codex".parse::<Host>().expect("codex"), Host::Codex);
        assert_eq!(
            "opencode".parse::<Host>().expect("opencode"),
            Host::OpenCode
        );
        assert_eq!("hermes".parse::<Host>().expect("hermes"), Host::Hermes);
    }

    #[test]
    fn rejects_unknown_event() {
        let raw = serde_json::json!({"hook_event_name": "TimeTravel"}).to_string();
        let result = NativeInput::parse(Host::Codex, raw.as_bytes());
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), "E_HOOK_EVENT_UNKNOWN");
    }

    #[test]
    fn rejects_oversize_input() {
        let raw = vec![b' '; 1_048_577];
        let result = NativeInput::parse(Host::ClaudeCode, &raw);
        assert!(result.is_err());
    }

    #[test]
    fn parses_opencode_envelope() {
        let raw = fixture("opencode-session-created.json");
        let input = NativeInput::parse(Host::OpenCode, &raw).expect("parse");
        assert_eq!(input.event, "session.created");
        assert_eq!(
            input.get_str(&["sessionId", "session_id"]).as_deref(),
            Some("opaque-session-9")
        );
    }

    #[test]
    fn parses_hermes_shell_hook() {
        let raw = fixture("hermes-pre-llm.json");
        let input = NativeInput::parse(Host::Hermes, &raw).expect("parse");
        assert_eq!(input.event, "pre_llm_call");
        assert_eq!(
            input.get_str(&["session_id"]).as_deref(),
            Some("hermes-sess-1")
        );
        assert_eq!(
            input.get_str(&["user_message"]).as_deref(),
            Some("Continue the Temporal provisioning work, what was decided?")
        );
    }

    #[test]
    fn rejects_unknown_hermes_event() {
        let raw = serde_json::json!({"hook_event_name": "on_hover"}).to_string();
        let result = NativeInput::parse(Host::Hermes, raw.as_bytes());
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), "E_HOOK_EVENT_UNKNOWN");
    }
}
