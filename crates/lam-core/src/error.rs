use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum LamError {
    #[error("{code}: {message}")]
    Typed { code: String, message: String },
    #[error("storage error: {0}")]
    Storage(String),
    #[error("io error: {0}")]
    Io(String),
    #[error("config error: {0}")]
    Config(String),
}

impl LamError {
    pub fn validation(code: &str, message: String) -> Self {
        Self::Typed {
            code: code.to_string(),
            message,
        }
    }
    pub fn storage(message: String) -> Self {
        Self::Storage(message)
    }
    pub fn io(message: String) -> Self {
        Self::Io(message)
    }
    pub fn config(message: String) -> Self {
        Self::Config(message)
    }
    pub fn code(&self) -> &str {
        match self {
            Self::Typed { code, .. } => code,
            Self::Storage(_) => "E_STORAGE",
            Self::Io(_) => "E_IO",
            Self::Config(_) => "E_CONFIG",
        }
    }
    pub fn to_body(&self) -> ErrorBody {
        match self {
            Self::Typed { code, message } => ErrorBody {
                code: code.clone(),
                message: message.clone(),
                details: None,
            },
            Self::Storage(message) => ErrorBody {
                code: "E_STORAGE".to_string(),
                message: message.clone(),
                details: None,
            },
            Self::Io(message) => ErrorBody {
                code: "E_IO".to_string(),
                message: message.clone(),
                details: None,
            },
            Self::Config(message) => ErrorBody {
                code: "E_CONFIG".to_string(),
                message: message.clone(),
                details: None,
            },
        }
    }
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self.to_body()).unwrap_or(serde_json::json!({
            "code": "E_SERIALIZE",
            "message": "failed to serialize error"
        }))
    }
}

impl From<serde_json::Error> for LamError {
    fn from(value: serde_json::Error) -> Self {
        Self::validation("E_JSON_INVALID", value.to_string())
    }
}

impl From<std::io::Error> for LamError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value.to_string())
    }
}
