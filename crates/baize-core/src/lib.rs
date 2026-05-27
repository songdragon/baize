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

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RouteDecisionId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HandoffId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PermissionId(pub String);

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

impl RouteDecisionId {
    pub fn new() -> Self {
        Self(format!("route_{}", Uuid::new_v4()))
    }
}

impl HandoffId {
    pub fn new() -> Self {
        Self(format!("handoff_{}", Uuid::new_v4()))
    }
}

impl PermissionId {
    pub fn new() -> Self {
        Self(format!("perm_{}", Uuid::new_v4()))
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
    pub session_resume: bool,
    pub mcp_server: bool,
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
pub struct RouteDecision {
    pub id: RouteDecisionId,
    pub session_id: TaskSessionId,
    pub selected_provider_id: ProviderId,
    pub previous_provider_id: Option<ProviderId>,
    pub reason: String,
    pub confidence: f32,
    pub mode: RoutingMode,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RoutingMode {
    Manual,
    Assisted,
    Autopilot,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandoffSummary {
    pub id: HandoffId,
    pub session_id: TaskSessionId,
    pub from_provider_id: ProviderId,
    pub to_provider_id: ProviderId,
    pub summary_markdown: String,
    pub mechanical_facts: HandoffFacts,
    pub status: HandoffStatus,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HandoffFacts {
    pub changed_files: Vec<String>,
    pub commands_run: Vec<String>,
    pub test_result: Option<String>,
    pub route_history: Vec<String>,
    pub provider_errors: Vec<String>,
    pub checkpoint_refs: Vec<String>,
    pub user_constraints: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HandoffStatus {
    Draft,
    Accepted,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRequest {
    pub id: PermissionId,
    pub workspace_id: Option<WorkspaceId>,
    pub session_id: Option<TaskSessionId>,
    pub command: String,
    pub reason: String,
    pub status: PermissionStatus,
    pub created_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PermissionStatus {
    Pending,
    Approved,
    Denied,
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn generated_ids_have_expected_prefixes() {
        assert!(WorkspaceId::new().0.starts_with("ws_"));
        assert!(ProjectId::new().0.starts_with("prj_"));
        assert!(TaskSessionId::new().0.starts_with("task_"));
        assert!(EventId::new().0.starts_with("evt_"));
        assert!(RouteDecisionId::new().0.starts_with("route_"));
        assert!(HandoffId::new().0.starts_with("handoff_"));
        assert!(PermissionId::new().0.starts_with("perm_"));
    }

    #[test]
    fn event_constructor_sets_required_fields() {
        let event = BaizeEvent::new("session.created", json!({ "objective": "test" }));

        assert_eq!(event.event_type, "session.created");
        assert!(event.id.0.starts_with("evt_"));
        assert_eq!(event.payload["objective"], "test");
        assert!(event.workspace_id.is_none());
        assert!(event.session_id.is_none());
        assert!(event.provider_id.is_none());
    }
}
