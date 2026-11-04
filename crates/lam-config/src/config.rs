use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    #[serde(default = "default_mode")]
    pub mode: String,
    #[serde(default = "default_log_level")]
    pub log_level: String,
    #[serde(default)]
    pub storage: StorageConfig,
    #[serde(default)]
    pub memory: MemoryConfig,
    #[serde(default)]
    pub recall: RecallConfig,
    #[serde(default)]
    pub admission: AdmissionConfig,
    #[serde(default)]
    pub models: ModelsConfig,
    #[serde(default)]
    pub dreaming: DreamingConfig,
    #[serde(default)]
    pub maintenance: MaintenanceConfig,
    #[serde(default)]
    pub privacy: PrivacyConfig,
    #[serde(default)]
    pub vault: VaultConfig,
}

fn default_schema_version() -> u32 {
    1
}
fn default_mode() -> String {
    "local_only".to_string()
}
fn default_log_level() -> String {
    "warn".to_string()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StorageConfig {
    #[serde(default = "storage_engine_default")]
    pub engine: String,
    #[serde(default = "bool_true")]
    pub wal: bool,
    #[serde(default = "default_busy_timeout")]
    pub busy_timeout_ms: u64,
}

fn storage_engine_default() -> String {
    "sqlite".to_string()
}
fn bool_true() -> bool {
    true
}
fn default_busy_timeout() -> u64 {
    3000
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            engine: storage_engine_default(),
            wal: true,
            busy_timeout_ms: default_busy_timeout(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryConfig {
    #[serde(default = "bool_true")]
    pub project_isolation: bool,
    #[serde(default = "default_ttl")]
    pub episodic_ttl_days: u64,
    #[serde(default = "default_capture_mode")]
    pub capture_mode: String,
    #[serde(default = "bool_true")]
    pub create_memory_files: bool,
}

fn default_ttl() -> u64 {
    365
}
fn default_capture_mode() -> String {
    "high_signal".to_string()
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            project_isolation: true,
            episodic_ttl_days: default_ttl(),
            capture_mode: default_capture_mode(),
            create_memory_files: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecallConfig {
    #[serde(default = "bool_true")]
    pub auto_inject_enabled: bool,
    #[serde(default = "default_max_tokens")]
    pub auto_inject_max_tokens: u32,
    #[serde(default = "default_min_score")]
    pub auto_inject_min_score: f64,
    #[serde(default = "default_max_results")]
    pub max_results: u32,
    #[serde(default = "bool_true")]
    pub keyword_search: bool,
    #[serde(default = "default_vector_search")]
    pub vector_search: String,
}

fn default_max_tokens() -> u32 {
    200
}
fn default_min_score() -> f64 {
    0.30
}
fn default_max_results() -> u32 {
    5
}
fn default_vector_search() -> String {
    "auto".to_string()
}

impl Default for RecallConfig {
    fn default() -> Self {
        Self {
            auto_inject_enabled: true,
            auto_inject_max_tokens: default_max_tokens(),
            auto_inject_min_score: default_min_score(),
            max_results: default_max_results(),
            keyword_search: true,
            vector_search: default_vector_search(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdmissionConfig {
    #[serde(default = "default_admission_mode")]
    pub mode: String,
    #[serde(default = "default_min_confidence")]
    pub minimum_confidence: f64,
    #[serde(default = "default_fallback")]
    pub fallback: String,
}

fn default_admission_mode() -> String {
    "shadow".to_string()
}
fn default_min_confidence() -> f64 {
    0.85
}
fn default_fallback() -> String {
    "deterministic".to_string()
}

impl Default for AdmissionConfig {
    fn default() -> Self {
        Self {
            mode: default_admission_mode(),
            minimum_confidence: default_min_confidence(),
            fallback: default_fallback(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderConfig {
    #[serde(default = "default_provider_none")]
    pub provider: String,
    #[serde(default)]
    pub model: String,
    #[serde(default = "default_endpoint")]
    pub endpoint: String,
    #[serde(default)]
    pub auto_download: bool,
    #[serde(default)]
    pub api_key_env: String,
    #[serde(default = "default_allow_remote")]
    pub allow_remote: bool,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default)]
    pub session_header: Option<String>,
}

fn default_provider_none() -> String {
    "none".to_string()
}
fn default_endpoint() -> String {
    "http://127.0.0.1:11434".to_string()
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            provider: default_provider_none(),
            model: String::new(),
            endpoint: default_endpoint(),
            auto_download: false,
            api_key_env: String::new(),
            allow_remote: default_allow_remote(),
            timeout_ms: default_timeout_ms(),
            session_header: None,
        }
    }
}

fn default_allow_remote() -> bool {
    false
}

fn default_timeout_ms() -> u64 {
    25_000
}

pub type EmbeddingConfig = ProviderConfig;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ModelsConfig {
    #[serde(default)]
    pub embedding: ProviderConfig,
    #[serde(default)]
    pub extraction: ProviderConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DreamingConfig {
    #[serde(default = "bool_true")]
    pub enabled: bool,
    #[serde(default = "default_min_episodes")]
    pub min_new_episodes: u32,
    #[serde(default = "default_min_idle")]
    pub min_idle_minutes: u32,
    #[serde(default)]
    pub auto_verify: bool,
}

fn default_min_episodes() -> u32 {
    10
}
fn default_min_idle() -> u32 {
    10
}

impl Default for DreamingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            min_new_episodes: default_min_episodes(),
            min_idle_minutes: default_min_idle(),
            auto_verify: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MaintenanceConfig {
    #[serde(default = "bool_true")]
    pub run_on_startup: bool,
    #[serde(default = "default_startup_budget")]
    pub startup_budget_ms: u64,
    #[serde(default = "default_schedule")]
    pub schedule: String,
}

fn default_startup_budget() -> u64 {
    100
}
fn default_schedule() -> String {
    "manual".to_string()
}

impl Default for MaintenanceConfig {
    fn default() -> Self {
        Self {
            run_on_startup: true,
            startup_budget_ms: default_startup_budget(),
            schedule: default_schedule(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PrivacyConfig {
    #[serde(default = "bool_true")]
    pub redact_secrets: bool,
    #[serde(default)]
    pub allow_network: bool,
    #[serde(default)]
    pub allow_cloud_context: bool,
    #[serde(default)]
    pub include_raw_transcript: bool,
}

impl Default for PrivacyConfig {
    fn default() -> Self {
        Self {
            redact_secrets: true,
            allow_network: false,
            allow_cloud_context: false,
            include_raw_transcript: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VaultConfig {
    #[serde(default = "default_max_file_bytes")]
    pub max_file_bytes: u64,
    #[serde(default)]
    pub follow_symlinks: bool,
    #[serde(default = "default_max_read_bytes")]
    pub max_read_bytes: u64,
}

fn default_max_file_bytes() -> u64 {
    1048576
}
fn default_max_read_bytes() -> u64 {
    20000
}

impl Default for VaultConfig {
    fn default() -> Self {
        Self {
            max_file_bytes: default_max_file_bytes(),
            follow_symlinks: false,
            max_read_bytes: default_max_read_bytes(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: 1,
            mode: default_mode(),
            log_level: default_log_level(),
            storage: StorageConfig::default(),
            memory: MemoryConfig::default(),
            recall: RecallConfig::default(),
            admission: AdmissionConfig::default(),
            models: ModelsConfig::default(),
            dreaming: DreamingConfig::default(),
            maintenance: MaintenanceConfig::default(),
            privacy: PrivacyConfig::default(),
            vault: VaultConfig::default(),
        }
    }
}

impl Config {
    pub fn validate(&self) -> Result<(), lam_core::LamError> {
        if self.schema_version != 1 {
            return Err(lam_core::LamError::config(
                "schema_version must be 1".to_string(),
            ));
        }
        if self.mode != "local_only" {
            return Err(lam_core::LamError::config(
                "mode must be local_only".to_string(),
            ));
        }
        match self.log_level.as_str() {
            "trace" | "debug" | "info" | "warn" | "error" => {}
            _ => {
                return Err(lam_core::LamError::config(
                    "log_level must be trace, debug, info, warn or error".to_string(),
                ))
            }
        }
        if self.storage.engine != "sqlite" {
            return Err(lam_core::LamError::config(
                "storage.engine must be sqlite".to_string(),
            ));
        }
        if self.storage.busy_timeout_ms == 0 || self.storage.busy_timeout_ms > 60000 {
            return Err(lam_core::LamError::config(
                "storage.busy_timeout_ms must be within 1 and 60000".to_string(),
            ));
        }
        if self.memory.episodic_ttl_days == 0 {
            return Err(lam_core::LamError::config(
                "memory.episodic_ttl_days must be greater than 0".to_string(),
            ));
        }
        match self.memory.capture_mode.as_str() {
            "high_signal" | "all" | "minimal" => {}
            _ => {
                return Err(lam_core::LamError::config(
                    "memory.capture_mode must be high_signal, all or minimal".to_string(),
                ))
            }
        }
        if self.recall.auto_inject_max_tokens == 0 || self.recall.auto_inject_max_tokens > 8000 {
            return Err(lam_core::LamError::config(
                "recall.auto_inject_max_tokens must be within 1 and 8000".to_string(),
            ));
        }
        if !(0.0..=1.0).contains(&self.recall.auto_inject_min_score) {
            return Err(lam_core::LamError::config(
                "recall.auto_inject_min_score must be within 0.0 and 1.0".to_string(),
            ));
        }
        if self.recall.max_results == 0 || self.recall.max_results > 100 {
            return Err(lam_core::LamError::config(
                "recall.max_results must be within 1 and 100".to_string(),
            ));
        }
        match self.recall.vector_search.as_str() {
            "auto" | "on" | "off" => {}
            _ => {
                return Err(lam_core::LamError::config(
                    "recall.vector_search must be auto, on or off".to_string(),
                ))
            }
        }
        match self.admission.mode.as_str() {
            "off" | "shadow" | "gate" => {}
            _ => {
                return Err(lam_core::LamError::config(
                    "admission.mode must be off, shadow or gate".to_string(),
                ))
            }
        }
        if !(0.0..=1.0).contains(&self.admission.minimum_confidence) {
            return Err(lam_core::LamError::config(
                "admission.minimum_confidence must be within 0.0 and 1.0".to_string(),
            ));
        }
        for provider in [&self.models.embedding, &self.models.extraction] {
            match provider.provider.as_str() {
                "none" | "ollama" | "local-executable" | "openai-compatible" => {}
                _ => return Err(lam_core::LamError::config(
                    "models provider must be none, ollama, local-executable or openai-compatible"
                        .to_string(),
                )),
            }
            if provider.auto_download {
                return Err(lam_core::LamError::config(
                    "models.auto_download must be false in local-only mode".to_string(),
                ));
            }
            if provider.provider == "none" && !provider.model.is_empty() {
                return Err(lam_core::LamError::config(
                    "models.model must be empty when provider is none".to_string(),
                ));
            }
            if provider.provider == "openai-compatible" {
                if provider.model.trim().is_empty() {
                    return Err(lam_core::LamError::config(
                        "models.model must name the remote model for openai-compatible".to_string(),
                    ));
                }
                if provider.api_key_env.trim().is_empty() {
                    return Err(lam_core::LamError::config(
                        "models.api_key_env must name the key holder for openai-compatible"
                            .to_string(),
                    ));
                }
            }
            if !provider.api_key_env.trim().is_empty() && !is_env_name(&provider.api_key_env) {
                return Err(lam_core::LamError::config(
                    "models.api_key_env must be a plain environment variable name".to_string(),
                ));
            }
            if provider.timeout_ms.clamp(500, 300_000) != provider.timeout_ms {
                return Err(lam_core::LamError::config(
                    "models.timeout_ms must be within 500 and 300000".to_string(),
                ));
            }
            if let Some(header) = provider.session_header.as_deref() {
                validate_session_header(header)?;
            }
            if provider.provider != "none"
                && !is_loopback_endpoint(&provider.endpoint)
                && !(provider.allow_remote && is_https_endpoint(&provider.endpoint))
            {
                return Err(lam_core::LamError::config(
                    "non-loopback models.endpoint needs allow_remote = true and an https URL"
                        .to_string(),
                ));
            }
        }
        match self.maintenance.schedule.as_str() {
            "manual" | "launchd" | "systemd-user" => {}
            _ => {
                return Err(lam_core::LamError::config(
                    "maintenance.schedule must be manual, launchd or systemd-user".to_string(),
                ))
            }
        }
        if self.privacy.allow_network {
            return Err(lam_core::LamError::config(
                "privacy.allow_network must be false in local-only mode".to_string(),
            ));
        }
        if self.vault.max_file_bytes == 0 {
            return Err(lam_core::LamError::config(
                "vault.max_file_bytes must be greater than 0".to_string(),
            ));
        }
        if self.vault.max_read_bytes == 0 {
            return Err(lam_core::LamError::config(
                "vault.max_read_bytes must be greater than 0".to_string(),
            ));
        }
        Ok(())
    }
    pub fn from_toml_str(text: &str) -> Result<Self, lam_core::LamError> {
        reject_inline_api_key(text)?;
        let parsed: Self =
            toml::from_str(text).map_err(|e| lam_core::LamError::config(e.to_string()))?;
        parsed.validate()?;
        Ok(parsed)
    }
    pub fn to_toml_string(&self) -> Result<String, lam_core::LamError> {
        self.validate()?;
        toml::to_string_pretty(self).map_err(|e| lam_core::LamError::config(e.to_string()))
    }
}

fn reject_inline_api_key(text: &str) -> Result<(), lam_core::LamError> {
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            continue;
        }
        let lower = trimmed.to_lowercase();
        let candidate = lower
            .split('=')
            .next()
            .unwrap_or("")
            .trim()
            .trim_matches(|c| c == '"' || c == '\'');
        if (candidate == "api_key" || candidate == "api-key" || candidate == "token")
            && !lower.contains("api_key_env")
        {
            return Err(lam_core::LamError::config(
                "config must not store secrets; name an environment variable with api_key_env instead".to_string(),
            ));
        }
    }
    Ok(())
}

fn is_https_endpoint(raw: &str) -> bool {
    raw.trim().to_lowercase().starts_with("https://")
}

fn is_env_name(raw: &str) -> bool {
    let text = raw.trim();
    !text.is_empty()
        && text.len() <= 128
        && text.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !text.chars().next().is_some_and(|c| c.is_ascii_digit())
}

fn is_loopback_endpoint(raw: &str) -> bool {
    let text = raw.trim();
    if text.is_empty() {
        return false;
    }
    let without_scheme = match text.find("://") {
        Some(position) => &text[position + 3..],
        None => text,
    };
    let host_port = without_scheme.split('/').next().unwrap_or("");
    let host = host_port
        .rsplit_once(':')
        .map(|(left, _)| left.trim_matches(|c| c == '[' || c == ']'))
        .unwrap_or(host_port);
    host == "127.0.0.1" || host == "localhost" || host == "::1" || host.starts_with("127.")
}

fn validate_session_header(raw: &str) -> Result<(), lam_core::LamError> {
    let text = raw.trim();
    if text.is_empty() {
        return Ok(());
    }
    let lower = text.to_lowercase();
    if ["off", "none", "false", "disabled", "auto"].contains(&lower.as_str()) {
        return Ok(());
    }
    if text.len() > 128
        || !text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
    {
        return Err(lam_core::LamError::config(
            "models.session_header must be off, none, auto or a plain header value".to_string(),
        ));
    }
    if lam_core::secrets::find_secret_kind(text).is_some() {
        return Err(lam_core::LamError::config(
            "models.session_header must not carry secret material".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_config_validates() {
        let config = Config::default();
        config.validate().expect("default config valid");
    }
    #[test]
    fn default_config_matches_spec_values() {
        let config = Config::default();
        assert_eq!(config.schema_version, 1);
        assert_eq!(config.storage.busy_timeout_ms, 3000);
        assert_eq!(config.memory.episodic_ttl_days, 365);
        assert_eq!(config.recall.auto_inject_max_tokens, 200);
        assert_eq!(config.admission.mode, "shadow");
        assert_eq!(config.models.embedding.provider, "none");
        assert!(!config.privacy.allow_network);
    }
    #[test]
    fn non_loopback_endpoint_rejected_for_local_provider() {
        let mut config = Config {
            models: ModelsConfig {
                extraction: ProviderConfig {
                    provider: "local-executable".to_string(),
                    model: "/bin/echo".to_string(),
                    endpoint: "https://models.example.com".to_string(),
                    auto_download: false,
                    api_key_env: String::new(),
                    allow_remote: false,
                    timeout_ms: default_timeout_ms(),
                    session_header: None,
                },
                ..ModelsConfig::default()
            },
            ..Config::default()
        };
        assert!(config.validate().is_err());
        config.models.extraction.endpoint = "http://127.0.0.1:11434".to_string();
        assert!(config.validate().is_ok());
    }

    fn remote_extraction(endpoint: &str, allow_remote: bool) -> Config {
        Config {
            models: ModelsConfig {
                extraction: ProviderConfig {
                    provider: "openai-compatible".to_string(),
                    model: "muse-spark".to_string(),
                    endpoint: endpoint.to_string(),
                    auto_download: false,
                    api_key_env: "LAM_OPENAI_COMPAT_API_KEY".to_string(),
                    allow_remote,
                    timeout_ms: default_timeout_ms(),
                    session_header: None,
                },
                ..ModelsConfig::default()
            },
            ..Config::default()
        }
    }

    #[test]
    fn remote_endpoint_allowed_only_with_flag_and_https() {
        assert!(
            remote_extraction("https://opencode.ai/inference/go/openai/v1", true)
                .validate()
                .is_ok()
        );
        assert!(
            remote_extraction("https://opencode.ai/inference/go/openai/v1", false)
                .validate()
                .is_err()
        );
        assert!(
            remote_extraction("http://opencode.ai/inference/go/openai/v1", true)
                .validate()
                .is_err()
        );
        assert!(remote_extraction("http://127.0.0.1:8080/v1", false)
            .validate()
            .is_ok());
    }

    #[test]
    fn remote_provider_requires_model_and_key_holder() {
        let mut config = remote_extraction("https://opencode.ai/inference/go/openai/v1", true);
        config.models.extraction.model.clear();
        assert!(config.validate().is_err());
        let mut config = remote_extraction("https://opencode.ai/inference/go/openai/v1", true);
        config.models.extraction.api_key_env.clear();
        assert!(config.validate().is_err());
        let mut config = remote_extraction("https://opencode.ai/inference/go/openai/v1", true);
        config.models.extraction.api_key_env = "has space".to_string();
        assert!(config.validate().is_err());
    }

    #[test]
    fn extraction_timeout_defaults_and_validates() {
        assert_eq!(Config::default().models.extraction.timeout_ms, 25_000);
        let mut config = remote_extraction("https://opencode.ai/inference/go/openai/v1", true);
        config.models.extraction.timeout_ms = 100;
        assert!(config.validate().is_err());
        let mut config = remote_extraction("https://opencode.ai/inference/go/openai/v1", true);
        config.models.extraction.timeout_ms = 400_000;
        assert!(config.validate().is_err());
        let mut config = remote_extraction("https://opencode.ai/inference/go/openai/v1", true);
        config.models.extraction.timeout_ms = 20_000;
        assert!(config.validate().is_ok());
    }

    #[test]
    fn session_header_accepts_auto_off_and_plain_value() {
        let mut config = remote_extraction("https://opencode.ai/inference/go/openai/v1", true);
        config.models.extraction.session_header = Some("auto".to_string());
        assert!(config.validate().is_ok());
        config.models.extraction.session_header = Some("off".to_string());
        assert!(config.validate().is_ok());
        config.models.extraction.session_header = Some("ses_lam_abc123".to_string());
        assert!(config.validate().is_ok());
        config.models.extraction.session_header = Some("has space".to_string());
        assert!(config.validate().is_err());
        config.models.extraction.session_header =
            Some("oc_sk_000000000000_FAKEfakeFAKEfakeFAKEfakeFAKEfa".to_string());
        assert!(config.validate().is_err());
    }

    #[test]
    fn inline_api_key_rejected_from_toml() {
        let base = Config::default().to_toml_string().expect("serialize");
        let poisoned = format!("{base}\n[models.extraction]\napi_key = \"sk-live\"\n");
        assert!(Config::from_toml_str(&poisoned).is_err());
        let named = base.replace("provider = \"none\"", "provider = \"openai-compatible\"");
        assert!(Config::from_toml_str(&named).is_err());
    }
    #[test]
    fn toml_round_trip_preserves_values() {
        let config = Config::default();
        let text = config.to_toml_string().expect("serialize default");
        let parsed = Config::from_toml_str(&text).expect("parse default");
        assert_eq!(config, parsed);
    }
    #[test]
    fn invalid_mode_rejected() {
        let config = Config {
            mode: "cloud".to_string(),
            ..Config::default()
        };
        assert!(config.validate().is_err());
    }
    #[test]
    fn invalid_busy_timeout_rejected() {
        let config = Config {
            storage: StorageConfig {
                busy_timeout_ms: 0,
                ..StorageConfig::default()
            },
            ..Config::default()
        };
        assert!(config.validate().is_err());
    }
    #[test]
    fn network_allowed_rejected() {
        let config = Config {
            privacy: PrivacyConfig {
                allow_network: true,
                ..PrivacyConfig::default()
            },
            ..Config::default()
        };
        assert!(config.validate().is_err());
    }
}
