use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct ExtractInput {
    pub text: String,
    pub source_ref: String,
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub kind: String,
    pub statement: String,
    pub confidence: f64,
    pub epistemic_status: String,
    pub sources: Vec<String>,
    pub entities: Vec<String>,
    pub recommended_action: String,
}

pub trait Extractor {
    fn name(&self) -> &'static str;
    fn extract(&self, input: &ExtractInput) -> Result<Vec<Candidate>, lam_core::LamError>;
    fn remote_usage(&self) -> Option<RemoteUsage> {
        None
    }
    fn last_remote_failure(&self) -> Option<String> {
        None
    }
}

#[derive(Debug, Clone, Default)]
pub struct RemoteUsage {
    pub provider: String,
    pub model: String,
    pub endpoint_host: String,
    pub payload_bytes: usize,
    pub redactions: usize,
}

pub fn candidate_status() -> lam_core::MemoryStatus {
    lam_core::MemoryStatus::Proposed
}

pub fn stable_session_id(project_id: &str, job_id: &str) -> String {
    let digest = blake3::hash(format!("{project_id}:{job_id}").as_bytes());
    let hex = digest.to_hex();
    format!("ses_lam_{}", &hex[..16])
}

fn valid_epistemic(value: &str) -> bool {
    matches!(value, "confirmed" | "observed" | "hypothesis" | "rejected")
}

fn valid_action(value: &str) -> bool {
    matches!(value, "review" | "accept" | "reject" | "defer")
}

pub fn validate_candidates(
    value: &serde_json::Value,
) -> Result<Vec<Candidate>, lam_core::LamError> {
    let invalid = |message: String| lam_core::LamError::validation("E_EXTRACTION_INVALID", message);
    let root = value
        .as_object()
        .ok_or_else(|| invalid("extraction output must be an object".to_string()))?;
    if root.get("schema_version").and_then(|v| v.as_u64()) != Some(1) {
        return Err(invalid("schema_version must be 1".to_string()));
    }
    let items = root
        .get("candidate_memories")
        .and_then(|v| v.as_array())
        .ok_or_else(|| invalid("candidate_memories must be an array".to_string()))?;
    if items.len() > 32 {
        return Err(invalid("candidate_memories exceeds 32 entries".to_string()));
    }
    let mut out = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let field = |name: &str| {
            item.get(name)
                .ok_or_else(|| invalid(format!("candidate {index} misses {name}")))
        };
        let kind = field("kind")?
            .as_str()
            .ok_or_else(|| invalid(format!("candidate {index} kind must be a string")))?;
        if kind.trim().is_empty() || kind.len() > 64 {
            return Err(invalid(format!(
                "candidate {index} kind is empty or too long"
            )));
        }
        let statement = field("statement")?
            .as_str()
            .ok_or_else(|| invalid(format!("candidate {index} statement must be a string")))?;
        if statement.trim().is_empty() || statement.len() > 4096 {
            return Err(invalid(format!(
                "candidate {index} statement is empty or too long"
            )));
        }
        let confidence = field("confidence")?
            .as_f64()
            .ok_or_else(|| invalid(format!("candidate {index} confidence must be a number")))?;
        if !(0.0..=1.0).contains(&confidence) {
            return Err(invalid(format!(
                "candidate {index} confidence out of range"
            )));
        }
        let epistemic = field("epistemic_status")?.as_str().ok_or_else(|| {
            invalid(format!(
                "candidate {index} epistemic_status must be a string"
            ))
        })?;
        if !valid_epistemic(epistemic) {
            return Err(invalid(format!(
                "candidate {index} has unknown epistemic status"
            )));
        }
        let sources = field("sources")?
            .as_array()
            .ok_or_else(|| invalid(format!("candidate {index} sources must be an array")))?;
        if sources.is_empty() {
            return Err(invalid(format!(
                "candidate {index} needs at least one source"
            )));
        }
        let mut source_list = Vec::with_capacity(sources.len());
        for source in sources {
            let text = source
                .as_str()
                .ok_or_else(|| invalid(format!("candidate {index} source must be a string")))?;
            if text.trim().is_empty() {
                return Err(invalid(format!("candidate {index} has an empty source")));
            }
            source_list.push(text.to_string());
        }
        let mut entity_list = Vec::new();
        if let Some(entities) = item.get("entities") {
            let list = entities
                .as_array()
                .ok_or_else(|| invalid(format!("candidate {index} entities must be an array")))?;
            for entity in list {
                let text = entity
                    .as_str()
                    .ok_or_else(|| invalid(format!("candidate {index} entity must be a string")))?;
                entity_list.push(text.to_string());
            }
        }
        let action = field("recommended_action")?.as_str().ok_or_else(|| {
            invalid(format!(
                "candidate {index} recommended_action must be a string"
            ))
        })?;
        if !valid_action(action) {
            return Err(invalid(format!(
                "candidate {index} has unknown recommended action"
            )));
        }
        out.push(Candidate {
            kind: kind.to_string(),
            statement: statement.to_string(),
            confidence,
            epistemic_status: epistemic.to_string(),
            sources: source_list,
            entities: entity_list,
            recommended_action: action.to_string(),
        });
    }
    Ok(out)
}

pub struct NoopExtractor;

impl Extractor for NoopExtractor {
    fn name(&self) -> &'static str {
        "none"
    }

    fn extract(&self, _input: &ExtractInput) -> Result<Vec<Candidate>, lam_core::LamError> {
        Ok(Vec::new())
    }
}

pub struct LocalExecutableExtractor {
    path: PathBuf,
    timeout: Duration,
}

impl LocalExecutableExtractor {
    pub fn new(path: PathBuf, timeout_ms: u64) -> Result<Self, lam_core::LamError> {
        if !path.is_file() {
            return Err(lam_core::LamError::validation(
                "E_PROVIDER_EXEC_INVALID",
                format!(
                    "extraction executable not found: {}",
                    path.to_string_lossy()
                ),
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path)
                .map(|m| m.permissions().mode())
                .unwrap_or(0);
            if mode & 0o111 == 0 {
                return Err(lam_core::LamError::validation(
                    "E_PROVIDER_EXEC_INVALID",
                    format!(
                        "extraction path is not executable: {}",
                        path.to_string_lossy()
                    ),
                ));
            }
        }
        Ok(Self {
            path,
            timeout: Duration::from_millis(timeout_ms.clamp(100, 120_000)),
        })
    }
}

impl Extractor for LocalExecutableExtractor {
    fn name(&self) -> &'static str {
        "local-executable"
    }

    fn extract(&self, input: &ExtractInput) -> Result<Vec<Candidate>, lam_core::LamError> {
        let excerpt: String = input.text.chars().take(8000).collect();
        let request = serde_json::json!({
            "text": excerpt,
            "source_ref": input.source_ref,
        });
        let request_text = serde_json::to_string(&request).map_err(lam_core::LamError::from)?;
        let mut child = std::process::Command::new(&self.path)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| lam_core::LamError::io(e.to_string()))?;
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            stdin
                .write_all(request_text.as_bytes())
                .map_err(|e| lam_core::LamError::io(e.to_string()))?;
        }
        let mut collected: Option<Vec<u8>> = None;
        if let Some(stdout) = child.stdout.take() {
            use std::io::Read;
            let started = Instant::now();
            let reader = stdout.take(1_048_577);
            let mut buffered = Vec::new();
            let outcome = std::thread::scope(|scope| {
                let handle = scope.spawn(move || {
                    let mut owned = reader;
                    let _ = owned.read_to_end(&mut buffered);
                    buffered
                });
                loop {
                    match child.try_wait() {
                        Ok(Some(_)) => break,
                        Ok(None) => {
                            if started.elapsed() >= self.timeout {
                                let _ = child.kill();
                                break;
                            }
                            std::thread::sleep(Duration::from_millis(10));
                        }
                        Err(e) => {
                            return Err(lam_core::LamError::io(e.to_string()));
                        }
                    }
                }
                Ok(handle.join().unwrap_or_default())
            })?;
            collected = Some(outcome);
        }
        let _ = child.wait();
        let raw = collected.unwrap_or_default();
        if raw.len() > 1_048_576 {
            return Err(lam_core::LamError::validation(
                "E_EXTRACTION_TOO_LARGE",
                "extraction output exceeds 1MiB".to_string(),
            ));
        }
        let value: serde_json::Value = serde_json::from_slice(&raw).map_err(|e| {
            lam_core::LamError::validation(
                "E_EXTRACTION_INVALID",
                format!("extraction output is not JSON: {e}"),
            )
        })?;
        validate_candidates(&value)
    }
}

pub fn extractor_for(
    config: &lam_config::ProviderConfig,
    timeout_ms: u64,
) -> Result<Box<dyn Extractor>, lam_core::LamError> {
    match config.provider.as_str() {
        "none" => Ok(Box::new(NoopExtractor)),
        "local-executable" => {
            if config.model.trim().is_empty() {
                return Err(lam_core::LamError::validation(
                    "E_PROVIDER_EXEC_INVALID",
                    "local-executable provider needs a model path to the executable".to_string(),
                ));
            }
            Ok(Box::new(LocalExecutableExtractor::new(
                PathBuf::from(config.model.trim()),
                timeout_ms,
            )?))
        }
        "openai-compatible" => {
            #[cfg(feature = "remote-provider")]
            {
                Ok(Box::new(crate::remote::RemoteExtractor::new(
                    config, timeout_ms,
                )?))
            }
            #[cfg(not(feature = "remote-provider"))]
            {
                return Err(lam_core::LamError::validation(
                    "E_PROVIDER_UNSUPPORTED",
                    "openai-compatible needs the remote-provider feature; rebuild with default features".to_string(),
                ));
            }
        }
        other => Err(lam_core::LamError::validation(
            "E_PROVIDER_UNSUPPORTED",
            format!("{other} provider extraction is not available in this build"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate_doc() -> serde_json::Value {
        serde_json::json!({
            "schema_version": 1,
            "candidate_memories": [{
                "kind": "lesson",
                "statement": "A timeout can be UDP media, not signaling.",
                "confidence": 0.71,
                "epistemic_status": "hypothesis",
                "sources": ["episode:example:4"],
                "entities": ["WebRTC"],
                "recommended_action": "review",
            }],
        })
    }

    fn write_script(body: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("extract.sh");
        std::fs::write(&path, body).expect("write script");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .expect("chmod script");
        }
        (dir, path)
    }

    #[test]
    fn valid_document_passes_strict_schema() {
        let found = validate_candidates(&candidate_doc()).expect("valid");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].epistemic_status, "hypothesis");
    }

    #[test]
    fn bad_schema_rejected() {
        let mut doc = candidate_doc();
        doc["candidate_memories"][0]["confidence"] = serde_json::json!(2.0);
        assert!(validate_candidates(&doc).is_err());
        let mut doc = candidate_doc();
        doc["candidate_memories"][0]["sources"] = serde_json::json!([]);
        assert!(validate_candidates(&doc).is_err());
        let mut doc = candidate_doc();
        doc["candidate_memories"][0]["recommended_action"] = serde_json::json!("auto-verify");
        assert!(validate_candidates(&doc).is_err());
    }

    #[test]
    fn candidates_always_map_to_proposed() {
        assert_eq!(candidate_status(), lam_core::MemoryStatus::Proposed);
    }

    #[test]
    fn noop_returns_nothing() {
        let found = NoopExtractor
            .extract(&ExtractInput {
                text: "anything".to_string(),
                source_ref: "test".to_string(),
                session_id: None,
            })
            .expect("noop");
        assert!(found.is_empty());
    }

    #[test]
    fn local_executable_round_trip() {
        let script = format!("printf '%s' '{}'", candidate_doc());
        let (_dir, path) = write_script(&script);
        let extractor = LocalExecutableExtractor::new(path, 5000).expect("executable");
        let found = extractor
            .extract(&ExtractInput {
                text: "input text".to_string(),
                source_ref: "test".to_string(),
                session_id: None,
            })
            .expect("extract");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].confidence, 0.71);
    }

    #[test]
    fn local_executable_timeout_fails_open() {
        let (_dir, path) = write_script("sleep 5");
        let extractor = LocalExecutableExtractor::new(path, 200).expect("executable");
        let result = extractor.extract(&ExtractInput {
            text: "input".to_string(),
            source_ref: "test".to_string(),
            session_id: None,
        });
        assert!(result.is_err());
    }

    #[test]
    fn ollama_reports_unsupported() {
        let config = lam_config::ProviderConfig {
            provider: "ollama".to_string(),
            model: "llama3".to_string(),
            endpoint: "http://127.0.0.1:11434".to_string(),
            auto_download: false,
            api_key_env: String::new(),
            allow_remote: false,
            timeout_ms: 25_000,
            session_header: None,
        };
        let result = extractor_for(&config, 1000);
        assert!(result.is_err());
        assert_eq!(
            result.err().map(|e| e.code().to_string()),
            Some("E_PROVIDER_UNSUPPORTED".to_string())
        );
    }
}
