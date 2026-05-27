use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WorkspaceId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProjectId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TaskSessionId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProviderId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EventId(pub String);

impl WorkspaceId {
    pub fn new() -> Self {
        Self(format!("ws_{}", Uuid::new_v4()))
    }
}

impl ProjectId {
    pub fn new() -> Self {
        Self(format!("prj_{}", Uuid::new_v4()))
    }
}

impl TaskSessionId {
    pub fn new() -> Self {
        Self(format!("task_{}", Uuid::new_v4()))
    }
}

impl EventId {
    pub fn new() -> Self {
        Self(format!("evt_{}", Uuid::new_v4()))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub name: String,
    pub primary_project_id: ProjectId,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: ProjectId,
    pub workspace_id: WorkspaceId,
    pub root: PathBuf,
    pub kind: ProjectKind,
    pub vcs: VcsKind,
    pub active_branch: Option<String>,
    pub trust_level: TrustLevel,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProjectKind {
    GitRepo,
    Directory,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum VcsKind {
    Git,
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TrustLevel {
    Trusted,
    Untrusted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderProfile {
    pub id: ProviderId,
    pub kind: ProviderKind,
    pub display_name: String,
    pub priority: u32,
    pub transports: Vec<ProviderTransport>,
    pub capabilities: ProviderCapabilities,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProviderKind {
    Codex,
    Gemini,
    Copilot,
    OpenCode,
    Other(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProviderTransport {
    Acp { command: String, args: Vec<String> },
    Native,
    Cli { command: String, args: Vec<String> },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderCapabilities {
    pub interactive_chat: bool,
    pub non_interactive_prompt: bool,
    pub patch_mode: bool,
    pub shell_access: bool,
    pub structured_output: bool,
    pub usage_telemetry: bool,
    pub acp: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderHealth {
    pub provider_id: ProviderId,
    pub status: HealthStatus,
    pub latency_ms: Option<u64>,
    pub last_error: Option<String>,
    pub checked_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Unavailable,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaState {
    pub provider_id: ProviderId,
    pub remaining_percent: Option<f32>,
    pub reset_eta_seconds: Option<u64>,
    pub confidence: QuotaConfidence,
    pub source: QuotaSource,
    pub observed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QuotaConfidence {
    Exact,
    Estimated,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QuotaSource {
    ProviderApi,
    ErrorInference,
    UserBudget,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSession {
    pub id: TaskSessionId,
    pub workspace_id: WorkspaceId,
    pub objective: String,
    pub active_provider_id: Option<ProviderId>,
    pub status: TaskSessionStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TaskSessionStatus {
    Running,
    WaitingForPermission,
    Completed,
    Failed,
    Canceled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BaizeEvent {
    pub id: EventId,
    pub event_type: String,
    pub timestamp: DateTime<Utc>,
    pub workspace_id: Option<WorkspaceId>,
    pub session_id: Option<TaskSessionId>,
    pub provider_id: Option<ProviderId>,
    pub payload: Value,
}

impl BaizeEvent {
    pub fn new(event_type: impl Into<String>, payload: Value) -> Self {
        Self {
            id: EventId::new(),
            event_type: event_type.into(),
            timestamp: Utc::now(),
            workspace_id: None,
            session_id: None,
            provider_id: None,
            payload,
        }
    }
}
