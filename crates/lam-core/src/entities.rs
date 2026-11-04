use chrono::DateTime;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;

use crate::ids::EntityId;
use crate::ids::MemoryId;
use crate::ids::ProjectId;
use crate::ids::RelationId;
use crate::ids::RevisionId;
use crate::ids::SessionId;
use crate::scope::Scope;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryStatus {
    Proposed,
    Verified,
    Disputed,
    Superseded,
    Archived,
}

impl MemoryStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            MemoryStatus::Proposed => "proposed",
            MemoryStatus::Verified => "verified",
            MemoryStatus::Disputed => "disputed",
            MemoryStatus::Superseded => "superseded",
            MemoryStatus::Archived => "archived",
        }
    }
    pub fn is_active(&self) -> bool {
        matches!(self, MemoryStatus::Verified)
    }
}

impl std::str::FromStr for MemoryStatus {
    type Err = crate::error::LamError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "proposed" => Ok(MemoryStatus::Proposed),
            "verified" => Ok(MemoryStatus::Verified),
            "disputed" => Ok(MemoryStatus::Disputed),
            "superseded" => Ok(MemoryStatus::Superseded),
            "archived" => Ok(MemoryStatus::Archived),
            _ => Err(crate::error::LamError::validation(
                "E_MEMORY_STATUS_INVALID",
                format!("unknown memory status: {s}"),
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    ArchitectureDecision,
    Lesson,
    Fact,
    Episode,
    Convention,
    Incident,
    Task,
    Note,
}

impl MemoryKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            MemoryKind::ArchitectureDecision => "architecture_decision",
            MemoryKind::Lesson => "lesson",
            MemoryKind::Fact => "fact",
            MemoryKind::Episode => "episode",
            MemoryKind::Convention => "convention",
            MemoryKind::Incident => "incident",
            MemoryKind::Task => "task",
            MemoryKind::Note => "note",
        }
    }
}

impl std::str::FromStr for MemoryKind {
    type Err = crate::error::LamError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "architecture_decision" | "decision" => Ok(MemoryKind::ArchitectureDecision),
            "lesson" => Ok(MemoryKind::Lesson),
            "fact" => Ok(MemoryKind::Fact),
            "episode" => Ok(MemoryKind::Episode),
            "convention" => Ok(MemoryKind::Convention),
            "incident" => Ok(MemoryKind::Incident),
            "task" => Ok(MemoryKind::Task),
            "note" => Ok(MemoryKind::Note),
            _ => Err(crate::error::LamError::validation(
                "E_MEMORY_KIND_INVALID",
                format!("unknown memory kind: {s}"),
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    Codex,
    ClaudeCode,
    Opencode,
    Cli,
}

impl AgentKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            AgentKind::Codex => "codex",
            AgentKind::ClaudeCode => "claude-code",
            AgentKind::Opencode => "opencode",
            AgentKind::Cli => "cli",
        }
    }
}

impl std::str::FromStr for AgentKind {
    type Err = crate::error::LamError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "codex" => Ok(AgentKind::Codex),
            "claude-code" => Ok(AgentKind::ClaudeCode),
            "opencode" => Ok(AgentKind::Opencode),
            "cli" => Ok(AgentKind::Cli),
            _ => Err(crate::error::LamError::validation(
                "E_AGENT_KIND_INVALID",
                format!("unknown agent kind: {s}"),
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityType {
    Project,
    Service,
    Component,
    Database,
    Interface,
    Library,
    Environment,
    Decision,
    Incident,
    Issue,
    File,
    TeamConvention,
    Tool,
    Concept,
}

impl EntityType {
    pub fn as_str(&self) -> &'static str {
        match self {
            EntityType::Project => "project",
            EntityType::Service => "service",
            EntityType::Component => "component",
            EntityType::Database => "database",
            EntityType::Interface => "interface",
            EntityType::Library => "library",
            EntityType::Environment => "environment",
            EntityType::Decision => "decision",
            EntityType::Incident => "incident",
            EntityType::Issue => "issue",
            EntityType::File => "file",
            EntityType::TeamConvention => "team_convention",
            EntityType::Tool => "tool",
            EntityType::Concept => "concept",
        }
    }
}

impl std::str::FromStr for EntityType {
    type Err = crate::error::LamError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim() {
            "project" => Ok(EntityType::Project),
            "service" => Ok(EntityType::Service),
            "component" => Ok(EntityType::Component),
            "database" => Ok(EntityType::Database),
            "interface" => Ok(EntityType::Interface),
            "library" => Ok(EntityType::Library),
            "environment" => Ok(EntityType::Environment),
            "decision" => Ok(EntityType::Decision),
            "incident" => Ok(EntityType::Incident),
            "issue" => Ok(EntityType::Issue),
            "file" => Ok(EntityType::File),
            "team_convention" => Ok(EntityType::TeamConvention),
            "tool" => Ok(EntityType::Tool),
            "concept" => Ok(EntityType::Concept),
            _ => Err(crate::error::LamError::validation(
                "E_ENTITY_TYPE_INVALID",
                format!("unknown entity type: {s}"),
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationType {
    DependsOn,
    Uses,
    Replaces,
    OwnedBy,
    ImplementedIn,
    CausedBy,
    MitigatedBy,
    RelatesTo,
    DeployedOn,
    Supersedes,
    Contradicts,
}

impl RelationType {
    pub fn as_str(&self) -> &'static str {
        match self {
            RelationType::DependsOn => "depends_on",
            RelationType::Uses => "uses",
            RelationType::Replaces => "replaces",
            RelationType::OwnedBy => "owned_by",
            RelationType::ImplementedIn => "implemented_in",
            RelationType::CausedBy => "caused_by",
            RelationType::MitigatedBy => "mitigated_by",
            RelationType::RelatesTo => "relates_to",
            RelationType::DeployedOn => "deployed_on",
            RelationType::Supersedes => "supersedes",
            RelationType::Contradicts => "contradicts",
        }
    }
}

impl std::str::FromStr for RelationType {
    type Err = crate::error::LamError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim() {
            "depends_on" => Ok(RelationType::DependsOn),
            "uses" => Ok(RelationType::Uses),
            "replaces" => Ok(RelationType::Replaces),
            "owned_by" => Ok(RelationType::OwnedBy),
            "implemented_in" => Ok(RelationType::ImplementedIn),
            "caused_by" => Ok(RelationType::CausedBy),
            "mitigated_by" => Ok(RelationType::MitigatedBy),
            "relates_to" => Ok(RelationType::RelatesTo),
            "deployed_on" => Ok(RelationType::DeployedOn),
            "supersedes" => Ok(RelationType::Supersedes),
            "contradicts" => Ok(RelationType::Contradicts),
            _ => Err(crate::error::LamError::validation(
                "E_RELATION_TYPE_INVALID",
                format!("unknown relation type: {s}"),
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Document,
    Session,
    Episode,
    User,
    Config,
    Test,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub id: ProjectId,
    pub name: String,
    pub canonical_root: PathBuf,
    pub identity_hash: String,
    pub created_at: DateTime<Utc>,
}

impl Project {
    pub fn new(name: String, canonical_root: PathBuf, created_at: DateTime<Utc>) -> Self {
        let identity_hash = blake3::hash(canonical_root.to_string_lossy().as_bytes())
            .to_hex()
            .to_string();
        Self {
            id: ProjectId::new(),
            name,
            canonical_root,
            identity_hash,
            created_at,
        }
    }
    pub fn validate(&self) -> Result<(), crate::error::LamError> {
        if self.name.trim().is_empty() {
            return Err(crate::error::LamError::validation(
                "E_PROJECT_NAME_INVALID",
                "project name must not be empty".to_string(),
            ));
        }
        if self.name.len() > 256 {
            return Err(crate::error::LamError::validation(
                "E_PROJECT_NAME_INVALID",
                "project name exceeds 256 chars".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub id: SessionId,
    pub project_id: ProjectId,
    pub agent_kind: AgentKind,
    pub external_session_id: Option<String>,
    pub branch: Option<String>,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Memory {
    pub id: MemoryId,
    pub project_id: ProjectId,
    pub scope: Scope,
    pub kind: MemoryKind,
    pub current_revision: Option<RevisionId>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryRevision {
    pub id: RevisionId,
    pub memory_id: MemoryId,
    pub content: String,
    pub status: MemoryStatus,
    pub version: u64,
    pub recorded_at: DateTime<Utc>,
    pub actor: String,
    pub reason: Option<String>,
}

impl MemoryRevision {
    pub fn validate(&self) -> Result<(), crate::error::LamError> {
        if self.content.trim().is_empty() {
            return Err(crate::error::LamError::validation(
                "E_MEMORY_CONTENT_INVALID",
                "memory content must not be empty".to_string(),
            ));
        }
        if self.content.len() > 65536 {
            return Err(crate::error::LamError::validation(
                "E_MEMORY_CONTENT_INVALID",
                "memory content exceeds 65536 bytes".to_string(),
            ));
        }
        if self.version == 0 {
            return Err(crate::error::LamError::validation(
                "E_MEMORY_VERSION_INVALID",
                "revision version must start at 1".to_string(),
            ));
        }
        if self.actor.trim().is_empty() {
            return Err(crate::error::LamError::validation(
                "E_MEMORY_ACTOR_INVALID",
                "actor must not be empty".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceEvidence {
    pub source_kind: SourceKind,
    pub source_ref: String,
    pub line_start: Option<u32>,
    pub line_end: Option<u32>,
    pub commit_sha: Option<String>,
    pub content_hash: Option<String>,
}

impl SourceEvidence {
    pub fn validate(&self) -> Result<(), crate::error::LamError> {
        if self.source_ref.trim().is_empty() {
            return Err(crate::error::LamError::validation(
                "E_SOURCE_REF_INVALID",
                "source_ref must not be empty".to_string(),
            ));
        }
        if let (Some(a), Some(b)) = (self.line_start, self.line_end) {
            if a > b {
                return Err(crate::error::LamError::validation(
                    "E_SOURCE_LINES_INVALID",
                    "line_start must not exceed line_end".to_string(),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FactAssertion {
    pub id: String,
    pub memory_revision_id: RevisionId,
    pub subject: String,
    pub predicate: String,
    pub value_json: serde_json::Value,
    pub valid_from: DateTime<Utc>,
    pub valid_until: Option<DateTime<Utc>>,
    pub recorded_at: DateTime<Utc>,
    pub retracted_at: Option<DateTime<Utc>>,
    pub confidence: f64,
}

impl FactAssertion {
    pub fn validate(&self) -> Result<(), crate::error::LamError> {
        if self.subject.trim().is_empty() {
            return Err(crate::error::LamError::validation(
                "E_FACT_SUBJECT_INVALID",
                "subject must not be empty".to_string(),
            ));
        }
        if self.predicate.trim().is_empty() {
            return Err(crate::error::LamError::validation(
                "E_FACT_PREDICATE_INVALID",
                "predicate must not be empty".to_string(),
            ));
        }
        if !(0.0..=1.0).contains(&self.confidence) {
            return Err(crate::error::LamError::validation(
                "E_FACT_CONFIDENCE_INVALID",
                "confidence must be within 0.0 and 1.0".to_string(),
            ));
        }
        if let Some(until) = self.valid_until {
            if self.valid_from > until {
                return Err(crate::error::LamError::validation(
                    "E_FACT_VALIDITY_INVALID",
                    "valid_from must not exceed valid_until".to_string(),
                ));
            }
        }
        Ok(())
    }
    pub fn is_active_at(&self, at: DateTime<Utc>) -> bool {
        self.valid_from <= at
            && self.valid_until.map(|u| at < u).unwrap_or(true)
            && self.retracted_at.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entity {
    pub id: EntityId,
    pub project_id: ProjectId,
    pub canonical_name: String,
    pub entity_type: EntityType,
    pub metadata: serde_json::Value,
}

impl Entity {
    pub fn validate(&self) -> Result<(), crate::error::LamError> {
        if self.canonical_name.trim().is_empty() {
            return Err(crate::error::LamError::validation(
                "E_ENTITY_NAME_INVALID",
                "canonical_name must not be empty".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Relation {
    pub id: RelationId,
    pub source_entity_id: EntityId,
    pub relation_type: RelationType,
    pub target_entity_id: EntityId,
    pub valid_from: DateTime<Utc>,
    pub valid_until: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FactRecord {
    pub schema_version: u32,
    pub memory_id: MemoryId,
    pub project_id: ProjectId,
    pub scope: Scope,
    pub kind: MemoryKind,
    pub subject: String,
    pub predicate: String,
    pub value: serde_json::Value,
    pub status: MemoryStatus,
    pub confidence: f64,
    pub valid_from: DateTime<Utc>,
    pub valid_until: Option<DateTime<Utc>>,
    pub recorded_at: DateTime<Utc>,
    pub source: SourceEvidence,
}

impl FactRecord {
    pub fn validate(&self) -> Result<(), crate::error::LamError> {
        if self.schema_version != 1 {
            return Err(crate::error::LamError::validation(
                "E_SCHEMA_VERSION_INVALID",
                "unsupported schema_version, expected 1".to_string(),
            ));
        }
        if self.subject.trim().is_empty() || self.predicate.trim().is_empty() {
            return Err(crate::error::LamError::validation(
                "E_FACT_RECORD_INVALID",
                "subject and predicate must not be empty".to_string(),
            ));
        }
        if !(0.0..=1.0).contains(&self.confidence) {
            return Err(crate::error::LamError::validation(
                "E_FACT_CONFIDENCE_INVALID",
                "confidence must be within 0.0 and 1.0".to_string(),
            ));
        }
        self.source.validate()?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NormalizedEvent {
    pub schema_version: u32,
    pub host: String,
    pub event_type: String,
    pub external_session_id: String,
    pub event_id: String,
    pub cwd: String,
    pub git_branch: Option<String>,
    pub timestamp: DateTime<Utc>,
    pub payload: HashMap<String, serde_json::Value>,
}

impl NormalizedEvent {
    pub fn validate(&self) -> Result<(), crate::error::LamError> {
        if self.schema_version != 1 {
            return Err(crate::error::LamError::validation(
                "E_SCHEMA_VERSION_INVALID",
                "unsupported schema_version, expected 1".to_string(),
            ));
        }
        for (field, value) in [
            ("host", &self.host),
            ("event_type", &self.event_type),
            ("external_session_id", &self.external_session_id),
            ("event_id", &self.event_id),
            ("cwd", &self.cwd),
        ] {
            if value.trim().is_empty() {
                return Err(crate::error::LamError::validation(
                    "E_EVENT_FIELD_INVALID",
                    format!("{field} must not be empty"),
                ));
            }
        }
        Ok(())
    }
    pub fn idempotency_key(&self) -> String {
        let raw = format!(
            "{}:{}:{}:{}",
            self.host, self.external_session_id, self.event_type, self.event_id
        );
        blake3::hash(raw.as_bytes()).to_hex().to_string()
    }
}
