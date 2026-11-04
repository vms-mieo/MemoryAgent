use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Global,
    Project,
    AgentPrivate,
    Session,
}

impl Scope {
    pub fn as_str(&self) -> &'static str {
        match self {
            Scope::Global => "global",
            Scope::Project => "project",
            Scope::AgentPrivate => "agent-private",
            Scope::Session => "session",
        }
    }
    pub fn all() -> &'static [Scope] {
        &[
            Scope::Global,
            Scope::Project,
            Scope::AgentPrivate,
            Scope::Session,
        ]
    }
}

impl std::str::FromStr for Scope {
    type Err = crate::error::LamError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "global" => Ok(Scope::Global),
            "project" => Ok(Scope::Project),
            "agent-private" => Ok(Scope::AgentPrivate),
            "session" => Ok(Scope::Session),
            _ => Err(crate::error::LamError::validation(
                "E_SCOPE_INVALID",
                format!("unknown scope: {s}"),
            )),
        }
    }
}

impl std::fmt::Display for Scope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}
