use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct EmbedRequest<'a> {
    pub texts: &'a [&'a str],
}

pub trait Embedder {
    fn model_id(&self) -> String;
    fn embed(&self, request: &EmbedRequest) -> Result<Vec<Vec<f32>>, lam_core::LamError>;
}

pub struct NoneEmbedder;

impl Embedder for NoneEmbedder {
    fn model_id(&self) -> String {
        "none".to_string()
    }

    fn embed(&self, _request: &EmbedRequest) -> Result<Vec<Vec<f32>>, lam_core::LamError> {
        Err(lam_core::LamError::validation(
            "E_PROVIDER_OFFLINE",
            "no embedding provider configured; running degraded FTS-only".to_string(),
        ))
    }
}

pub struct HashEmbedder {
    dimensions: usize,
}

impl HashEmbedder {
    pub fn test_only(dimensions: usize) -> Result<Self, lam_core::LamError> {
        if dimensions == 0 || dimensions > 4096 {
            return Err(lam_core::LamError::validation(
                "E_DIM_INVALID",
                "dimensions must be within 1 and 4096".to_string(),
            ));
        }
        Ok(Self { dimensions })
    }
}

impl Embedder for HashEmbedder {
    fn model_id(&self) -> String {
        "hash-test-v1".to_string()
    }

    fn embed(&self, request: &EmbedRequest) -> Result<Vec<Vec<f32>>, lam_core::LamError> {
        let mut out = Vec::with_capacity(request.texts.len());
        for text in request.texts {
            out.push(hash_vector(text, self.dimensions));
        }
        Ok(out)
    }
}

fn hash_vector(text: &str, dimensions: usize) -> Vec<f32> {
    let mut state = blake3::hash(text.as_bytes()).to_hex().to_string();
    let mut values = Vec::with_capacity(dimensions);
    while values.len() < dimensions {
        let digest = blake3::hash(state.as_bytes());
        let bytes = digest.as_bytes();
        let mut offset = 0;
        while offset + 8 <= bytes.len() && values.len() < dimensions {
            let mut word = [0u8; 8];
            word.copy_from_slice(&bytes[offset..offset + 8]);
            let mixed = splitmix64(u64::from_le_bytes(word).wrapping_add(values.len() as u64));
            values.push((mixed as f64 / u64::MAX as f64) as f32 * 2.0 - 1.0);
            offset += 8;
        }
        state = digest.to_hex().to_string();
    }
    values
}

fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9E3779B97F4A7C15);
    let mut result = value;
    result = (result ^ (result >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    result = (result ^ (result >> 27)).wrapping_mul(0x94D049BB133111EB);
    result ^ (result >> 31)
}

pub struct ExecutableEmbedder {
    path: PathBuf,
    timeout: Duration,
}

impl ExecutableEmbedder {
    pub fn new(path: PathBuf, timeout_ms: u64) -> Result<Self, lam_core::LamError> {
        if !path.is_file() {
            return Err(lam_core::LamError::validation(
                "E_PROVIDER_EXEC_INVALID",
                format!("embedding executable not found: {}", path.to_string_lossy()),
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
                        "embedding path is not executable: {}",
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

impl Embedder for ExecutableEmbedder {
    fn model_id(&self) -> String {
        format!(
            "local-executable:{}",
            self.path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("program")
        )
    }

    fn embed(&self, request: &EmbedRequest) -> Result<Vec<Vec<f32>>, lam_core::LamError> {
        let input = serde_json::json!({ "texts": request.texts });
        let input_text = serde_json::to_string(&input).map_err(lam_core::LamError::from)?;
        if input_text.len() > 65536 {
            return Err(lam_core::LamError::validation(
                "E_EMBED_INPUT_TOO_LARGE",
                "embedding batch exceeds 64KiB".to_string(),
            ));
        }
        let mut child = std::process::Command::new(&self.path)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| lam_core::LamError::io(e.to_string()))?;
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            stdin
                .write_all(input_text.as_bytes())
                .map_err(|e| lam_core::LamError::io(e.to_string()))?;
        }
        let mut collected: Option<Vec<u8>> = None;
        if let Some(stdout) = child.stdout.take() {
            use std::io::Read;
            let started = Instant::now();
            let mut reader = stdout.take(1_048_577);
            let mut buffered = Vec::new();
            let outcome = std::thread::scope(|scope| {
                let handle = scope.spawn(move || {
                    let _ = reader.read_to_end(&mut buffered);
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
                        Err(e) => return Err(lam_core::LamError::io(e.to_string())),
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
                "E_EMBED_TOO_LARGE",
                "embedding output exceeds 1MiB".to_string(),
            ));
        }
        let value: serde_json::Value = serde_json::from_slice(&raw).map_err(|_| {
            lam_core::LamError::validation(
                "E_EMBED_INVALID",
                "embedding output is not JSON".to_string(),
            )
        })?;
        parse_vectors(&value, request.texts.len())
    }
}

fn parse_vectors(
    value: &serde_json::Value,
    expected: usize,
) -> Result<Vec<Vec<f32>>, lam_core::LamError> {
    let invalid = |message: String| lam_core::LamError::validation("E_EMBED_INVALID", message);
    let items = value
        .get("vectors")
        .and_then(|v| v.as_array())
        .ok_or_else(|| invalid("embedding output must contain a vectors array".to_string()))?;
    if items.len() != expected {
        return Err(invalid(format!(
            "embedding count {} does not match input {}",
            items.len(),
            expected
        )));
    }
    let mut out = Vec::with_capacity(items.len());
    let mut dimensions: Option<usize> = None;
    for (index, item) in items.iter().enumerate() {
        let row = item
            .as_array()
            .ok_or_else(|| invalid(format!("vector {index} must be an array")))?;
        if row.is_empty() || row.len() > 4096 {
            return Err(invalid(format!("vector {index} has invalid dimensions")));
        }
        if let Some(expected_dims) = dimensions {
            if row.len() != expected_dims {
                return Err(invalid("embedding dimensions are inconsistent".to_string()));
            }
        } else {
            dimensions = Some(row.len());
        }
        let mut vector = Vec::with_capacity(row.len());
        for component in row {
            let value = component
                .as_f64()
                .ok_or_else(|| invalid(format!("vector {index} must contain numbers")))?;
            if !value.is_finite() {
                return Err(invalid(format!("vector {index} must be finite")));
            }
            #[allow(clippy::cast_possible_truncation)]
            let narrowed = value as f32;
            vector.push(narrowed);
        }
        out.push(vector);
    }
    if out.is_empty() {
        return Err(invalid("embedding output is empty".to_string()));
    }
    Ok(out)
}

pub fn embedding_for(
    config: &lam_config::ProviderConfig,
    timeout_ms: u64,
) -> Result<Box<dyn Embedder>, lam_core::LamError> {
    match config.provider.as_str() {
        "none" => Ok(Box::new(NoneEmbedder)),
        "local-executable" => {
            if config.model.trim().is_empty() {
                return Err(lam_core::LamError::validation(
                    "E_PROVIDER_EXEC_INVALID",
                    "local-executable embedding needs a model path to the executable".to_string(),
                ));
            }
            Ok(Box::new(ExecutableEmbedder::new(
                PathBuf::from(config.model.trim()),
                timeout_ms,
            )?))
        }
        other => Err(lam_core::LamError::validation(
            "E_PROVIDER_UNSUPPORTED",
            format!("{other} embedding provider lands in a later phase; use local-executable"),
        )),
    }
}

pub fn cosine_similarity(left: &[f32], right: &[f32]) -> Option<f32> {
    if left.len() != right.len() || left.is_empty() {
        return None;
    }
    let mut dot = 0.0f64;
    let mut left_norm = 0.0f64;
    let mut right_norm = 0.0f64;
    for (l, r) in left.iter().zip(right.iter()) {
        let l = f64::from(*l);
        let r = f64::from(*r);
        dot += l * r;
        left_norm += l * l;
        right_norm += r * r;
    }
    let denominator = left_norm.sqrt() * right_norm.sqrt();
    if denominator == 0.0 {
        return Some(0.0);
    }
    #[allow(clippy::cast_possible_truncation)]
    let score = (dot / denominator) as f32;
    Some(score.clamp(-1.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_embeddings_are_deterministic() {
        let embedder = HashEmbedder::test_only(8).expect("hash embedder");
        let first = embedder
            .embed(&EmbedRequest {
                texts: &["Temporal provisions the database"],
            })
            .expect("embed");
        let second = embedder
            .embed(&EmbedRequest {
                texts: &["Temporal provisions the database"],
            })
            .expect("embed");
        assert_eq!(first, second);
        assert_eq!(first[0].len(), 8);
        let other = embedder
            .embed(&EmbedRequest {
                texts: &["Kafka streams click events"],
            })
            .expect("embed");
        assert_ne!(first, other);
    }

    #[test]
    fn identical_texts_are_most_similar() {
        let embedder = HashEmbedder::test_only(16).expect("hash embedder");
        let vectors = embedder
            .embed(&EmbedRequest {
                texts: &[
                    "alpha beta gamma",
                    "alpha beta gamma",
                    "completely different zeta",
                ],
            })
            .expect("embed");
        let same = cosine_similarity(&vectors[0], &vectors[1]).expect("score");
        assert!((same - 1.0).abs() < 1e-5);
    }

    #[test]
    fn invalid_dimensions_rejected() {
        assert!(HashEmbedder::test_only(0).is_err());
        assert!(HashEmbedder::test_only(5000).is_err());
    }

    #[test]
    fn malformed_vectors_rejected() {
        assert!(parse_vectors(&serde_json::json!({}), 1).is_err());
        assert!(parse_vectors(&serde_json::json!({"vectors": []}), 0).is_err());
        assert!(parse_vectors(&serde_json::json!({"vectors": [[1.0, "x"]]}), 1).is_err());
        assert!(parse_vectors(&serde_json::json!({"vectors": [[1.0], [1.0, 2.0]]}), 2).is_err());
        let parsed =
            parse_vectors(&serde_json::json!({"vectors": [[0.5, -0.25]]}), 1).expect("parse");
        assert_eq!(parsed[0].len(), 2);
    }

    #[test]
    fn local_executable_round_trip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("emb.sh");
        std::fs::write(
            &path,
            "#!/bin/sh\nprintf '%s' '{\"vectors\": [[0.1, 0.2, 0.3]]}'\n",
        )
        .expect("write");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
        let embedder = ExecutableEmbedder::new(path, 5000).expect("executable");
        let vectors = embedder
            .embed(&EmbedRequest { texts: &["hello"] })
            .expect("embed");
        assert_eq!(vectors.len(), 1);
        assert_eq!(vectors[0].len(), 3);
    }

    #[test]
    fn missing_executable_rejected() {
        let result = ExecutableEmbedder::new(PathBuf::from("/nonexistent-lam-embed"), 1000);
        assert!(result.is_err());
        assert_eq!(
            result.err().map(|e| e.code().to_string()),
            Some("E_PROVIDER_EXEC_INVALID".to_string())
        );
    }
}
