use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
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

impl Default for WorkspaceId {
    fn default() -> Self {
        Self::new()
    }
}

impl ProjectId {
    pub fn new() -> Self {
        Self(format!("prj_{}", Uuid::new_v4()))
    }
}

impl Default for ProjectId {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskSessionId {
    pub fn new() -> Self {
        Self(format!("task_{}", Uuid::new_v4()))
    }
}

impl Default for TaskSessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl EventId {
    pub fn new() -> Self {
        Self(format!("evt_{}", Uuid::new_v4()))
    }
}

impl Default for EventId {
    fn default() -> Self {
        Self::new()
    }
}

impl RouteDecisionId {
    pub fn new() -> Self {
        Self(format!("route_{}", Uuid::new_v4()))
    }
}

impl Default for RouteDecisionId {
    fn default() -> Self {
        Self::new()
    }
}

impl HandoffId {
    pub fn new() -> Self {
        Self(format!("handoff_{}", Uuid::new_v4()))
    }
}

impl Default for HandoffId {
    fn default() -> Self {
        Self::new()
    }
}

impl PermissionId {
    pub fn new() -> Self {
        Self(format!("perm_{}", Uuid::new_v4()))
    }
}

impl Default for PermissionId {
    fn default() -> Self {
        Self::new()
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
    Antigravity,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum QuotaConfidence {
    Exact,
    Estimated,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    #[serde(default)]
    pub provider_native_session_ids: BTreeMap<String, String>,
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
    #[serde(default)]
    pub task_type: Option<TaskType>,
    pub confidence: f32,
    pub mode: RoutingMode,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskType {
    Testing,
    Debugging,
    Refactor,
    Documentation,
    Implementation,
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
    #[serde(default)]
    pub risk: PermissionRiskAssessment,
    pub status: PermissionStatus,
    pub created_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionRiskAssessment {
    pub level: PermissionRiskLevel,
    pub reasons: Vec<String>,
    pub recommendation: String,
}

impl Default for PermissionRiskAssessment {
    fn default() -> Self {
        Self {
            level: PermissionRiskLevel::Low,
            reasons: Vec::new(),
            recommendation: "Approve if the command matches the requested task.".to_string(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionRiskLevel {
    #[default]
    Low,
    Medium,
    High,
    Blocked,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PermissionStatus {
    Pending,
    Approved,
    Denied,
}

pub fn assess_permission_command(command: &str) -> PermissionRiskAssessment {
    let normalized = command.to_ascii_lowercase();
    let mut reasons = Vec::new();

    if contains_any(
        &normalized,
        &[
            "rm -rf /", "mkfs", "dd if=", "dd of=", ":(){", "shutdown", "reboot",
        ],
    ) {
        reasons.push("destructive system-level command pattern".to_string());
        return PermissionRiskAssessment {
            level: PermissionRiskLevel::Blocked,
            reasons,
            recommendation:
                "Deny unless the user explicitly requested this exact destructive operation."
                    .to_string(),
        };
    }

    if contains_any(
        &normalized,
        &[
            "sudo ",
            "chmod -r",
            "chmod 777",
            "chown -r",
            "git reset --hard",
            "git clean",
            " rm ",
            "rm -",
        ],
    ) {
        reasons.push("command can delete, overwrite or elevate privileges".to_string());
        return PermissionRiskAssessment {
            level: PermissionRiskLevel::High,
            reasons,
            recommendation: "Ask for explicit user confirmation before approving.".to_string(),
        };
    }

    if contains_any(
        &normalized,
        &[
            "cargo test",
            "cargo fmt",
            "cargo clippy",
            "git status",
            "git diff",
            "ls",
        ],
    ) {
        return PermissionRiskAssessment::default();
    }

    if contains_any(
        &normalized,
        &["cargo run", "npm ", "pnpm ", "yarn ", "python "],
    ) {
        reasons.push("command may execute project code or install/run scripts".to_string());
        return PermissionRiskAssessment {
            level: PermissionRiskLevel::Medium,
            reasons,
            recommendation: "Approve when the command is expected for the current task."
                .to_string(),
        };
    }

    PermissionRiskAssessment {
        level: PermissionRiskLevel::Medium,
        reasons: vec!["unknown command pattern".to_string()],
        recommendation: "Review the command before approving.".to_string(),
    }
}

fn contains_any(text: &str, patterns: &[&str]) -> bool {
    patterns.iter().any(|pattern| text.contains(pattern))
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
    fn default_ids_have_expected_prefixes() {
        assert!(WorkspaceId::default().0.starts_with("ws_"));
        assert!(ProjectId::default().0.starts_with("prj_"));
        assert!(TaskSessionId::default().0.starts_with("task_"));
        assert!(EventId::default().0.starts_with("evt_"));
        assert!(RouteDecisionId::default().0.starts_with("route_"));
        assert!(HandoffId::default().0.starts_with("handoff_"));
        assert!(PermissionId::default().0.starts_with("perm_"));
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

    #[test]
    fn task_session_deserializes_without_native_provider_session_ids() {
        let raw = json!({
            "id": "task_legacy",
            "workspace_id": "ws_legacy",
            "objective": "legacy",
            "active_provider_id": "codex",
            "status": "Running",
            "created_at": "2026-06-07T00:00:00Z",
            "updated_at": "2026-06-07T00:00:00Z"
        });

        let session: TaskSession = serde_json::from_value(raw).expect("legacy session");

        assert!(session.provider_native_session_ids.is_empty());
        assert_eq!(session.active_provider_id.expect("provider").0, "codex");
    }

    #[test]
    fn task_session_native_provider_session_ids_round_trip() {
        let mut native_ids = std::collections::BTreeMap::new();
        native_ids.insert("codex".to_string(), "codex_native_1".to_string());
        let now = Utc::now();
        let session = TaskSession {
            id: TaskSessionId("task_roundtrip".to_string()),
            workspace_id: WorkspaceId("ws_roundtrip".to_string()),
            objective: "round trip".to_string(),
            active_provider_id: Some(ProviderId("codex".to_string())),
            provider_native_session_ids: native_ids,
            status: TaskSessionStatus::Running,
            created_at: now,
            updated_at: now,
        };

        let value = serde_json::to_value(&session).expect("serialize session");
        let restored: TaskSession = serde_json::from_value(value).expect("deserialize session");

        assert_eq!(
            restored.provider_native_session_ids.get("codex"),
            Some(&"codex_native_1".to_string())
        );
    }

    #[test]
    fn route_decision_deserializes_without_task_type() {
        let raw = json!({
            "id": "route_1",
            "session_id": "task_1",
            "selected_provider_id": "codex",
            "previous_provider_id": null,
            "reason": "legacy record",
            "confidence": 0.75,
            "mode": "Assisted",
            "created_at": Utc::now(),
        });

        let decision: RouteDecision = serde_json::from_value(raw).expect("route decision");

        assert!(decision.task_type.is_none());
    }

    #[test]
    fn assesses_permission_command_risk_levels() {
        let low = assess_permission_command("cargo test --workspace");
        assert_eq!(low.level, PermissionRiskLevel::Low);
        assert!(low.reasons.is_empty());

        let medium = assess_permission_command("npm run build");
        assert_eq!(medium.level, PermissionRiskLevel::Medium);
        assert!(!medium.reasons.is_empty());

        let high = assess_permission_command("sudo chmod 777 /tmp/file");
        assert_eq!(high.level, PermissionRiskLevel::High);
        assert!(!high.reasons.is_empty());

        let blocked = assess_permission_command("rm -rf /");
        assert_eq!(blocked.level, PermissionRiskLevel::Blocked);
        assert!(!blocked.reasons.is_empty());
    }

    #[test]
    fn permission_deserializes_without_risk_for_legacy_records() {
        let raw = json!({
            "id": "perm_1",
            "workspace_id": null,
            "session_id": null,
            "command": "cargo test",
            "reason": "verify",
            "status": "Pending",
            "created_at": Utc::now(),
            "resolved_at": null
        });

        let permission: PermissionRequest = serde_json::from_value(raw).expect("legacy permission");

        assert_eq!(permission.risk.level, PermissionRiskLevel::Low);
        assert!(permission.risk.reasons.is_empty());
    }
}
