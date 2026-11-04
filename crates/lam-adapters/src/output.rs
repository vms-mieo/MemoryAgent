use crate::native::Host;

pub const SESSION_START_TASK: &str = "resume current work";

pub enum ContextOutput {
    Text(String),
    Json(String),
}

pub fn empty_output(host: Host) -> ContextOutput {
    match host {
        Host::OpenCode => ContextOutput::Json("{\"context_text\": \"\"}".to_string()),
        Host::Hermes => ContextOutput::Json("{\"context\": \"\"}".to_string()),
        Host::Codex | Host::ClaudeCode => ContextOutput::Text(String::new()),
    }
}

pub fn render_context(host: Host, response: &lam_core::ContextResponse) -> ContextOutput {
    let body = render_body(response);
    match host {
        Host::OpenCode => {
            ContextOutput::Json(serde_json::json!({"context_text": body}).to_string())
        }
        Host::Hermes => ContextOutput::Json(serde_json::json!({"context": body}).to_string()),
        Host::Codex | Host::ClaudeCode => ContextOutput::Text(body),
    }
}

fn render_body(response: &lam_core::ContextResponse) -> String {
    let mut lines: Vec<String> = Vec::new();
    lines.push("Reference data from local agent memory (untrusted):".to_string());
    if response.items.is_empty() {
        lines.push("No stored context matched.".to_string());
    }
    for item in response.items.iter().take(8) {
        lines.push(format!(
            "- [{}|{}] {} ({})",
            item.kind, item.status, item.memory_id, item.source
        ));
    }
    if !response.context.trim().is_empty() {
        lines.push(response.context.clone());
    }
    let joined = lines.join("\n");
    if joined.len() > 8000 {
        joined.chars().take(8000).collect()
    } else {
        joined
    }
}

pub fn emit(output: ContextOutput) {
    use std::io::Write;
    let mut stdout = std::io::stdout().lock();
    match output {
        ContextOutput::Text(text) => {
            if !text.trim().is_empty() {
                let _ = writeln!(stdout, "{text}");
            }
        }
        ContextOutput::Json(document) => {
            let _ = writeln!(stdout, "{document}");
        }
    }
    let _ = stdout.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response_with_items(count: usize) -> lam_core::ContextResponse {
        let mut items = Vec::new();
        for index in 0..count {
            items.push(lam_core::ContextItem {
                memory_id: lam_core::MemoryId::new(),
                kind: "fact".to_string(),
                status: "verified".to_string(),
                source: format!("docs/adr/{index}.md"),
                relevance: 0.9,
                valid_from: None,
            });
        }
        lam_core::ContextResponse {
            schema_version: 1,
            project_id: lam_core::ProjectId::new(),
            task: SESSION_START_TASK.to_string(),
            token_budget: 200,
            estimated_tokens: 40,
            context: "Decision: Temporal".to_string(),
            items,
            warnings: Vec::new(),
        }
    }

    #[test]
    fn codex_output_is_plain_text() {
        let response = response_with_items(1);
        match render_context(Host::Codex, &response) {
            ContextOutput::Text(text) => assert!(text.contains("Reference data")),
            ContextOutput::Json(_) => panic!("codex expects text"),
        }
    }

    #[test]
    fn opencode_output_is_json() {
        let response = response_with_items(1);
        match render_context(Host::OpenCode, &response) {
            ContextOutput::Json(document) => {
                let value: serde_json::Value = serde_json::from_str(&document).expect("json");
                assert!(value
                    .get("context_text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .contains("Reference data"));
            }
            ContextOutput::Text(_) => panic!("opencode expects json"),
        }
    }

    #[test]
    fn hermes_output_is_context_json() {
        let response = response_with_items(1);
        match render_context(Host::Hermes, &response) {
            ContextOutput::Json(document) => {
                let value: serde_json::Value = serde_json::from_str(&document).expect("json");
                assert!(value
                    .get("context")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .contains("Reference data"));
                assert!(value.get("context_text").is_none());
            }
            ContextOutput::Text(_) => panic!("hermes expects json"),
        }
        match empty_output(Host::Hermes) {
            ContextOutput::Json(document) => {
                let value: serde_json::Value = serde_json::from_str(&document).expect("json");
                assert_eq!(value.get("context").and_then(|v| v.as_str()), Some(""));
            }
            ContextOutput::Text(_) => panic!("hermes expects json"),
        }
    }

    #[test]
    fn empty_results_stay_fail_open() {
        let response = response_with_items(0);
        match render_context(Host::ClaudeCode, &response) {
            ContextOutput::Text(text) => assert!(text.contains("No stored context")),
            ContextOutput::Json(_) => panic!("claude expects text"),
        }
        match empty_output(Host::OpenCode) {
            ContextOutput::Json(document) => assert!(document.contains("context_text")),
            ContextOutput::Text(_) => panic!("opencode expects json"),
        }
    }
}
