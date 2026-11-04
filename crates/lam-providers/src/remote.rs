use std::sync::Mutex;
use std::time::Duration;

use super::RemoteUsage;

pub struct RemoteExtractor {
    endpoint: String,
    host: String,
    model: String,
    key_env: String,
    timeout: Duration,
    session_header: Option<String>,
    last_usage: Mutex<Option<RemoteUsage>>,
    last_failure: Mutex<Option<String>>,
}

impl RemoteExtractor {
    pub fn new(
        config: &lam_config::ProviderConfig,
        timeout_ms: u64,
    ) -> Result<Self, lam_core::LamError> {
        if config.model.trim().is_empty() {
            return Err(lam_core::LamError::validation(
                "E_PROVIDER_REMOTE_INVALID",
                "openai-compatible provider needs a model name".to_string(),
            ));
        }
        if config.api_key_env.trim().is_empty() {
            return Err(lam_core::LamError::validation(
                "E_PROVIDER_REMOTE_INVALID",
                "openai-compatible provider needs api_key_env".to_string(),
            ));
        }
        let endpoint = config.endpoint.trim().trim_end_matches('/').to_string();
        Ok(Self {
            host: endpoint_host(&endpoint),
            endpoint,
            model: config.model.trim().to_string(),
            key_env: config.api_key_env.trim().to_string(),
            timeout: Duration::from_millis(timeout_ms.clamp(500, 120_000)),
            session_header: config.session_header.clone(),
            last_usage: Mutex::new(None),
            last_failure: Mutex::new(None),
        })
    }

    pub fn take_usage(&self) -> Option<RemoteUsage> {
        self.last_usage.lock().ok()?.clone()
    }

    pub fn take_failure(&self) -> Option<String> {
        self.last_failure.lock().ok()?.clone()
    }

    fn note_failure(&self, reason: &str) {
        if let Ok(mut slot) = self.last_failure.lock() {
            *slot = Some(reason.to_string());
        }
    }

    fn api_key(&self) -> Result<String, lam_core::LamError> {
        std::env::var(&self.key_env).map_err(|_| {
            lam_core::LamError::validation(
                "E_PROVIDER_UNAVAILABLE",
                format!(
                    "key holder {} is not set; deterministic rules still apply",
                    self.key_env
                ),
            )
        })
    }
}

pub fn endpoint_host(raw: &str) -> String {
    let text = raw.trim();
    let without_scheme = match text.find("://") {
        Some(position) => &text[position + 3..],
        None => text,
    };
    without_scheme.split('/').next().unwrap_or("").to_string()
}

pub fn user_agent() -> String {
    format!("agent-memory/{}", env!("CARGO_PKG_VERSION"))
}

pub fn stable_session_id(project_id: &str, job_id: &str) -> String {
    super::stable_session_id(project_id, job_id)
}

pub fn is_opencode_host(host: &str) -> bool {
    host.trim().to_lowercase().contains("opencode.ai")
}

pub fn resolve_session_header(
    configured: Option<&str>,
    host: &str,
    session_id: Option<&str>,
) -> Option<String> {
    if let Some(raw) = configured {
        let text = raw.trim();
        if text.is_empty() {
            return None;
        }
        let lower = text.to_lowercase();
        if ["off", "none", "false", "disabled"].contains(&lower.as_str()) {
            return None;
        }
        if lower == "auto" {
            return session_id
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
        }
        return Some(text.to_string());
    }
    if !is_opencode_host(host) {
        return None;
    }
    session_id
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn excerpt_of(text: &str) -> String {
    const CAP: usize = 12_000;
    if text.len() <= CAP {
        return text.to_string();
    }
    let mut end = CAP;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

fn chat_url(endpoint: &str) -> String {
    format!("{endpoint}/chat/completions")
}

fn schema_prompt() -> &'static str {
    "Return only JSON with schema_version 1 and a candidate_memories array. Each entry needs kind, statement, confidence 0 to 1, epistemic_status (confirmed, observed, hypothesis or rejected), sources array, entities array and recommended_action (review, accept, reject or defer)."
}

impl super::Extractor for RemoteExtractor {
    fn name(&self) -> &'static str {
        "openai-compatible"
    }

    fn extract(
        &self,
        input: &crate::ExtractInput,
    ) -> Result<Vec<super::Candidate>, lam_core::LamError> {
        let key = self.api_key()?;
        let (scrubbed, redactions) = lam_adapters::redact::scrub_text(&input.text);
        let excerpt = excerpt_of(&scrubbed);
        let body = serde_json::json!({
            "model": self.model,
            "temperature": 0,
            "response_format": {"type": "json_object"},
            "messages": [
                {"role": "system", "content": schema_prompt()},
                {"role": "user", "content": format!("source: {}\n{}", input.source_ref, excerpt)},
            ],
        });
        let payload = serde_json::to_vec(&body).map_err(lam_core::LamError::from)?;
        *self
            .last_usage
            .lock()
            .map_err(|_| lam_core::LamError::storage("remote usage lock poisoned".to_string()))? =
            Some(RemoteUsage {
                provider: "openai-compatible".to_string(),
                model: self.model.clone(),
                endpoint_host: self.host.clone(),
                payload_bytes: payload.len(),
                redactions,
            });
        let session = resolve_session_header(
            self.session_header.as_deref(),
            &self.host,
            input.session_id.as_deref(),
        );
        let text = match post_json(
            &chat_url(&self.endpoint),
            &key,
            &payload,
            self.timeout,
            &user_agent(),
            session.as_deref(),
        ) {
            Ok(text) => text,
            Err((reason, error)) => {
                self.note_failure(&reason);
                return Err(error);
            }
        };
        let inner = chat_choice_text(&text).inspect_err(|_| {
            self.note_failure("schema");
        })?;
        let value: serde_json::Value = serde_json::from_str(&inner).map_err(|_| {
            self.note_failure("schema");
            remote_failed("remote broke the candidate schema")
        })?;
        super::validate_candidates(&value).map_err(|_| {
            self.note_failure("schema");
            remote_failed("remote broke the candidate schema")
        })
    }

    fn remote_usage(&self) -> Option<RemoteUsage> {
        self.take_usage()
    }

    fn last_remote_failure(&self) -> Option<String> {
        self.take_failure()
    }
}

fn remote_failed(message: &str) -> lam_core::LamError {
    lam_core::LamError::validation(
        "E_PROVIDER_UNAVAILABLE",
        format!("{message}; deterministic rules still apply"),
    )
}

fn post_json(
    url: &str,
    key: &str,
    payload: &[u8],
    timeout: Duration,
    user_agent: &str,
    session_id: Option<&str>,
) -> Result<String, (String, lam_core::LamError)> {
    let agent = ureq::AgentBuilder::new().timeout(timeout).build();
    let started = std::time::Instant::now();
    let mut request = agent
        .post(url)
        .set("Authorization", &format!("Bearer {key}"))
        .set("Content-Type", "application/json")
        .set("User-Agent", user_agent);
    if let Some(session) = session_id {
        request = request.set("x-opencode-session", session);
    }
    let response = request.send_bytes(payload);
    let elapsed = started.elapsed();
    match response {
        Ok(ok) => ok.into_string().map_err(|_| {
            (
                "unreadable".to_string(),
                remote_failed("remote body unreadable"),
            )
        }),
        Err(ureq::Error::Status(code, _)) => Err((
            format!("status-{code}"),
            remote_failed(&format!("remote status {code}")),
        )),
        Err(ureq::Error::Transport(transport)) => {
            let reason = transport_reason(&transport, elapsed, timeout);
            let message = transport
                .message()
                .unwrap_or("no detail")
                .chars()
                .take(120)
                .collect::<String>();
            Err((
                reason,
                remote_failed(&format!("remote transport failed ({message})")),
            ))
        }
    }
}

fn transport_reason(transport: &ureq::Transport, elapsed: Duration, timeout: Duration) -> String {
    if elapsed >= timeout {
        return "timeout".to_string();
    }
    match transport.kind() {
        ureq::ErrorKind::ConnectionFailed => "connect".to_string(),
        ureq::ErrorKind::Dns => "dns".to_string(),
        _ => "transport".to_string(),
    }
}

fn chat_choice_text(raw: &str) -> Result<String, lam_core::LamError> {
    let value: serde_json::Value =
        serde_json::from_str(raw).map_err(|_| remote_failed("remote returned non-JSON"))?;
    value
        .get("choices")
        .and_then(|choices| choices.as_array())
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(|content| content.as_str())
        .map(|text| text.to_string())
        .ok_or_else(|| remote_failed("remote missed chat choices"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Extractor;
    use std::io::Read;
    use std::io::Write;
    use std::sync::mpsc;

    fn fake_server(
        bodies: mpsc::Sender<Vec<u8>>,
        status: u16,
        reply: &'static str,
        delay_ms: u64,
    ) -> (String, std::thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind fake server");
        let port = listener.local_addr().expect("port").port();
        let handle = std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut reader = std::io::BufReader::new(stream.try_clone().expect("clone"));
                let mut header = String::new();
                let mut length = 0usize;
                loop {
                    header.clear();
                    if std::io::BufRead::read_line(&mut reader, &mut header).unwrap_or(0) == 0 {
                        break;
                    }
                    if header.trim().is_empty() {
                        break;
                    }
                    if let Some(value) = header.strip_prefix("Content-Length:") {
                        length = value.trim().parse().unwrap_or(0);
                    }
                    if let Some(value) = header.strip_prefix("content-length:") {
                        length = value.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0u8; length];
                reader.read_exact(&mut body).unwrap_or(());
                let _ = bodies.send(body);
                if delay_ms > 0 {
                    std::thread::sleep(Duration::from_millis(delay_ms));
                }
                let phrase = match status {
                    200 => "OK",
                    500 => "Internal Server Error",
                    _ => "Error",
                };
                let response = format!(
                    "HTTP/1.1 {status} {phrase}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                    reply.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        (format!("http://127.0.0.1:{port}"), handle)
    }

    fn config_for(endpoint: String, holder: &str) -> lam_config::ProviderConfig {
        lam_config::ProviderConfig {
            provider: "openai-compatible".to_string(),
            model: "muse-spark".to_string(),
            endpoint,
            auto_download: false,
            api_key_env: holder.to_string(),
            allow_remote: false,
            timeout_ms: 25_000,
            session_header: None,
        }
    }

    fn config_with_session(
        endpoint: String,
        holder: &str,
        session_header: Option<String>,
    ) -> lam_config::ProviderConfig {
        lam_config::ProviderConfig {
            provider: "openai-compatible".to_string(),
            model: "muse-spark".to_string(),
            endpoint,
            auto_download: false,
            api_key_env: holder.to_string(),
            allow_remote: false,
            timeout_ms: 25_000,
            session_header,
        }
    }

    fn candidate_reply() -> &'static str {
        "{\"choices\":[{\"message\":{\"content\":\"{\\\"schema_version\\\":1,\\\"candidate_memories\\\":[{\\\"kind\\\":\\\"lesson\\\",\\\"statement\\\":\\\"Timeout handling needs media path checks\\\",\\\"confidence\\\":0.7,\\\"epistemic_status\\\":\\\"hypothesis\\\",\\\"sources\\\":[\\\"episode:1\\\"],\\\"entities\\\":[],\\\"recommended_action\\\":\\\"review\\\"}]}\"}}]}"
    }

    #[test]
    fn redacted_payload_reaches_server_and_parses() {
        let (bodies, received) = mpsc::channel();
        let (endpoint, handle) = fake_server(bodies, 200, candidate_reply(), 0);
        unsafe { std::env::set_var("LAM_TEST_REMOTE_KEY_A", "test-key") };
        let extractor = RemoteExtractor::new(&config_for(endpoint, "LAM_TEST_REMOTE_KEY_A"), 5000)
            .expect("extractor");
        let found = extractor
            .extract(&crate::ExtractInput {
                text: "Deploy with token ghp_fixture0000000000000000000000 and api_key=fixture-secret-value then continue".to_string(),
                source_ref: "episode:9".to_string(),
                session_id: None
            })
            .expect("extract");
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0].statement,
            "Timeout handling needs media path checks"
        );
        let body = received.recv_timeout(Duration::from_secs(5)).expect("body");
        let text = String::from_utf8_lossy(&body).to_string();
        assert!(
            !text.contains("ghp_fixture"),
            "secret must be scrubbed before send"
        );
        assert!(
            !text.contains("fixture-secret-value"),
            "secret must be scrubbed before send"
        );
        assert!(
            text.contains("[REDACTED:"),
            "scrub marker must be visible instead"
        );
        let usage = extractor.take_usage().expect("usage");
        assert_eq!(usage.provider, "openai-compatible");
        assert!(usage.payload_bytes > 0);
        assert!(usage.redactions >= 2);
        handle.join().expect("server done");
    }

    #[test]
    fn server_error_falls_back_without_content() {
        let (bodies, _) = mpsc::channel();
        let (endpoint, handle) = fake_server(bodies, 500, "boom", 0);
        unsafe { std::env::set_var("LAM_TEST_REMOTE_KEY_B", "test-key") };
        let extractor = RemoteExtractor::new(&config_for(endpoint, "LAM_TEST_REMOTE_KEY_B"), 5000)
            .expect("extractor");
        let error = extractor
            .extract(&crate::ExtractInput {
                text: "plain text".to_string(),
                source_ref: "episode:1".to_string(),
                session_id: None,
            })
            .expect_err("must fail");
        assert_eq!(error.code(), "E_PROVIDER_UNAVAILABLE");
        assert_eq!(
            extractor.take_failure().as_deref(),
            Some("status-500"),
            "server errors must be classified"
        );
        handle.join().expect("server done");
    }

    #[test]
    fn timeout_falls_back() {
        let (bodies, _) = mpsc::channel();
        let (endpoint, handle) = fake_server(bodies, 200, candidate_reply(), 1500);
        unsafe { std::env::set_var("LAM_TEST_REMOTE_KEY_C", "test-key") };
        let extractor = RemoteExtractor::new(&config_for(endpoint, "LAM_TEST_REMOTE_KEY_C"), 300)
            .expect("extractor");
        let error = extractor
            .extract(&crate::ExtractInput {
                text: "plain text".to_string(),
                source_ref: "episode:1".to_string(),
                session_id: None,
            })
            .expect_err("must time out");
        assert_eq!(error.code(), "E_PROVIDER_UNAVAILABLE");
        assert_eq!(
            extractor.take_failure().as_deref(),
            Some("timeout"),
            "slow servers must be classified as timeouts"
        );
        handle.join().expect("server done");
    }

    #[test]
    fn missing_key_falls_back_before_network() {
        let extractor = RemoteExtractor::new(
            &config_for(
                "http://127.0.0.1:9".to_string(),
                "LAM_TEST_REMOTE_KEY_ABSENT_FOR_TEST",
            ),
            1000,
        )
        .expect("extractor");
        let error = extractor
            .extract(&crate::ExtractInput {
                text: "plain text".to_string(),
                source_ref: "episode:1".to_string(),
                session_id: None,
            })
            .expect_err("must fail");
        assert_eq!(error.code(), "E_PROVIDER_UNAVAILABLE");
        assert!(extractor.take_usage().is_none());
    }

    fn fake_server_capture(
        captured: mpsc::Sender<(Vec<String>, Vec<u8>)>,
        status: u16,
        reply: &'static str,
        expected_requests: usize,
    ) -> (String, std::thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind fake server");
        let port = listener.local_addr().expect("port").port();
        let handle = std::thread::spawn(move || {
            for _ in 0..expected_requests {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                let mut headers = Vec::new();
                let mut length = 0usize;
                {
                    let mut reader = std::io::BufReader::new(stream.try_clone().expect("clone"));
                    loop {
                        let mut line = String::new();
                        if std::io::BufRead::read_line(&mut reader, &mut line).unwrap_or(0) == 0 {
                            break;
                        }
                        if line.trim().is_empty() {
                            break;
                        }
                        headers.push(line.trim().to_string());
                        if let Some(value) = line
                            .strip_prefix("Content-Length:")
                            .or_else(|| line.strip_prefix("content-length:"))
                        {
                            length = value.trim().parse().unwrap_or(0);
                        }
                    }
                    let mut body = vec![0u8; length];
                    let _ = std::io::Read::read_exact(&mut reader, &mut body);
                    let _ = captured.send((headers, body));
                }
                let phrase = match status {
                    200 => "OK",
                    _ => "Error",
                };
                let response = format!(
                    "HTTP/1.1 {status} {phrase}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                    reply.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        (format!("http://127.0.0.1:{port}"), handle)
    }

    fn header_value(headers: &[String], name: &str) -> Option<String> {
        headers.iter().find_map(|line| {
            let mut parts = line.splitn(2, ':');
            let key = parts.next()?.trim();
            let value = parts.next()?.trim();
            if key.eq_ignore_ascii_case(name) {
                Some(value.to_string())
            } else {
                None
            }
        })
    }

    #[test]
    fn user_agent_uses_crate_version() {
        let agent = user_agent();
        assert!(agent.starts_with("agent-memory/"));
        assert!(agent.contains(env!("CARGO_PKG_VERSION")));
        assert!(!agent.to_lowercase().contains("ureq"));
    }

    #[test]
    fn stable_session_id_is_deterministic_without_secrets() {
        let first = stable_session_id("proj-1", "job-1");
        let second = stable_session_id("proj-1", "job-1");
        assert_eq!(first, second);
        assert!(first.starts_with("ses_lam_"));
        assert_eq!(first.len(), "ses_lam_".len() + 16);
        assert!(!first.contains("proj-1"));
        assert!(!first.contains("job-1"));
        assert_ne!(first, stable_session_id("proj-1", "job-2"));
        assert_ne!(first, stable_session_id("proj-2", "job-1"));
    }

    #[test]
    fn resolve_session_header_defaults_to_opencode_host() {
        assert_eq!(
            resolve_session_header(None, "opencode.ai", Some("ses_lam_abc")),
            Some("ses_lam_abc".to_string())
        );
        assert_eq!(
            resolve_session_header(None, "127.0.0.1:8080", Some("ses_lam_abc")),
            None
        );
        assert_eq!(resolve_session_header(None, "opencode.ai", None), None);
        assert_eq!(
            resolve_session_header(Some("auto"), "127.0.0.1:8080", Some("ses_lam_abc")),
            Some("ses_lam_abc".to_string())
        );
        assert_eq!(
            resolve_session_header(Some("off"), "opencode.ai", Some("ses_lam_abc")),
            None
        );
        assert_eq!(
            resolve_session_header(Some("ses_custom_1"), "127.0.0.1:8080", Some("ses_lam_abc")),
            Some("ses_custom_1".to_string())
        );
    }

    #[test]
    fn opencode_headers_present_and_stable_on_loopback() {
        let (captured, received) = mpsc::channel();
        let (endpoint, handle) = fake_server_capture(captured, 200, candidate_reply(), 2);
        unsafe { std::env::set_var("LAM_TEST_REMOTE_KEY_HEADERS", "test-key") };
        let config = config_with_session(
            endpoint,
            "LAM_TEST_REMOTE_KEY_HEADERS",
            Some("auto".to_string()),
        );
        let extractor = RemoteExtractor::new(&config, 5000).expect("extractor");
        let session = stable_session_id("proj-headers", "job-headers");
        for _ in 0..2 {
            let found = extractor
                .extract(&crate::ExtractInput {
                    text: "plain text".to_string(),
                    source_ref: "episode:1".to_string(),
                    session_id: Some(session.clone()),
                })
                .expect("extract");
            assert_eq!(found.len(), 1);
        }
        let mut sessions = Vec::new();
        for _ in 0..2 {
            let (headers, _) = received
                .recv_timeout(Duration::from_secs(5))
                .expect("request");
            let agent = header_value(&headers, "User-Agent").expect("user agent");
            assert!(agent.starts_with("agent-memory/"));
            assert!(!agent.to_lowercase().contains("ureq"));
            let session_header =
                header_value(&headers, "x-opencode-session").expect("session header");
            assert_eq!(session_header, session);
            assert!(!session_header.contains("test-key"));
            sessions.push(session_header);
        }
        assert_eq!(sessions[0], sessions[1]);
        handle.join().expect("server done");
    }

    #[test]
    fn disabled_session_header_sends_no_session() {
        let (captured, received) = mpsc::channel();
        let (endpoint, handle) = fake_server_capture(captured, 200, candidate_reply(), 1);
        unsafe { std::env::set_var("LAM_TEST_REMOTE_KEY_NOSESSION", "test-key") };
        let config = config_with_session(
            endpoint,
            "LAM_TEST_REMOTE_KEY_NOSESSION",
            Some("off".to_string()),
        );
        let extractor = RemoteExtractor::new(&config, 5000).expect("extractor");
        extractor
            .extract(&crate::ExtractInput {
                text: "plain text".to_string(),
                source_ref: "episode:1".to_string(),
                session_id: Some("ses_lam_should_hide".to_string()),
            })
            .expect("extract");
        let (headers, _) = received
            .recv_timeout(Duration::from_secs(5))
            .expect("request");
        assert!(header_value(&headers, "User-Agent").is_some());
        assert!(header_value(&headers, "x-opencode-session").is_none());
        handle.join().expect("server done");
    }

    #[test]
    fn custom_session_header_value_is_sent_verbatim() {
        let (captured, received) = mpsc::channel();
        let (endpoint, handle) = fake_server_capture(captured, 200, candidate_reply(), 1);
        unsafe { std::env::set_var("LAM_TEST_REMOTE_KEY_CUSTOM", "test-key") };
        let config = config_with_session(
            endpoint,
            "LAM_TEST_REMOTE_KEY_CUSTOM",
            Some("ses_custom_9".to_string()),
        );
        let extractor = RemoteExtractor::new(&config, 5000).expect("extractor");
        extractor
            .extract(&crate::ExtractInput {
                text: "plain text".to_string(),
                source_ref: "episode:1".to_string(),
                session_id: Some("ses_lam_ignored".to_string()),
            })
            .expect("extract");
        let (headers, _) = received
            .recv_timeout(Duration::from_secs(5))
            .expect("request");
        assert_eq!(
            header_value(&headers, "x-opencode-session").as_deref(),
            Some("ses_custom_9")
        );
        handle.join().expect("server done");
    }
}
