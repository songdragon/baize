use anyhow::Result;
use axum::extract::{Path as AxumPath, State};
use axum::response::sse::{Event, Sse};
use axum::routing::{get, post};
use axum::{Json, Router};
use baize_adapters::{
    check_provider, default_provider_profiles, run_agent_prompt, validate_all_providers,
    validate_provider,
};
use baize_adapters::{AgentExecutionEventKind, AgentPromptRequest, AgentRunResult};
use baize_config::BaizeConfig;
use baize_core::{
    BaizeEvent, HandoffFacts, HandoffId, HandoffStatus, HandoffSummary, PermissionId,
    PermissionRequest, PermissionStatus, Project, ProjectId, ProjectKind, ProviderId,
    RouteDecision, RouteDecisionId, RoutingMode, TaskSession, TaskSessionId, TaskSessionStatus,
    TrustLevel, VcsKind, Workspace, WorkspaceId,
};
use baize_storage::EventStore;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;
use tower_http::cors::CorsLayer;

#[derive(Clone)]
pub struct AppState {
    config: BaizeConfig,
    store: Arc<Mutex<EventStore>>,
    events: broadcast::Sender<BaizeEvent>,
    executor: Arc<dyn AgentExecutor>,
}

pub trait AgentExecutor: Send + Sync {
    fn run_prompt(&self, request: AgentPromptRequest) -> Result<AgentRunResult>;
}

#[derive(Clone)]
struct RealAgentExecutor;

impl AgentExecutor for RealAgentExecutor {
    fn run_prompt(&self, request: AgentPromptRequest) -> Result<AgentRunResult> {
        run_agent_prompt(request)
    }
}

#[derive(Debug, Deserialize)]
pub struct WorkspaceStatusQuery {
    pub path: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
struct CreateWorkspaceRequest {
    path: PathBuf,
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CreateSessionRequest {
    workspace_id: String,
    objective: String,
    provider_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PromptRequest {
    prompt: String,
    timeout_seconds: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct HandoffRequest {
    to_provider_id: String,
    user_constraints: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct CreatePermissionRequest {
    workspace_id: Option<String>,
    session_id: Option<String>,
    command: String,
    reason: String,
}

#[derive(Debug, Deserialize)]
struct PermissionsQuery {
    status: Option<String>,
    session_id: Option<String>,
}

pub async fn run(config: BaizeConfig) -> Result<()> {
    let addr: SocketAddr = format!("{}:{}", config.daemon.host, config.daemon.port).parse()?;
    let state = AppState::new(config, EventStore::open_default()?);
    let app = router(state);
    let listener = TcpListener::bind(addr).await?;
    tracing::info!("baize daemon listening on http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}

impl AppState {
    pub fn new(config: BaizeConfig, store: EventStore) -> Self {
        Self::with_executor(config, store, Arc::new(RealAgentExecutor))
    }

    pub fn with_executor(
        config: BaizeConfig,
        store: EventStore,
        executor: Arc<dyn AgentExecutor>,
    ) -> Self {
        let (events, _) = broadcast::channel(256);
        Self {
            config,
            store: Arc::new(Mutex::new(store)),
            events,
            executor,
        }
    }

    fn record_event(&self, event: BaizeEvent) {
        if let Ok(store) = self.store.lock() {
            let _ = store.append_event(&event);
        }
        let _ = self.events.send(event);
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/workspaces", get(workspaces).post(create_workspace))
        .route("/workspaces/:id/status", get(workspace_status_by_id))
        .route("/providers", get(providers))
        .route("/providers/:id/health", get(provider_health))
        .route("/providers/:id/validate", get(provider_validate))
        .route("/providers/check", post(check_providers))
        .route("/providers/validate", post(validate_providers))
        .route("/workspaces/status", get(workspace_status))
        .route("/sessions", get(sessions).post(create_session))
        .route("/sessions/:id", get(session))
        .route("/sessions/:id/prompt", post(prompt_session))
        .route("/sessions/:id/cancel", post(cancel_session))
        .route("/sessions/:id/routes", get(session_routes))
        .route("/sessions/:id/handoff", post(create_handoff))
        .route(
            "/sessions/:id/handoff/:handoff_id/accept",
            post(accept_handoff),
        )
        .route("/sessions/:id/events", get(session_events))
        .route("/sessions/:id/diff", get(session_diff))
        .route("/sessions/:id/handoff/:handoff_id", get(handoff))
        .route("/permissions", get(permissions).post(create_permission))
        .route("/permissions/:id/approve", post(approve_permission))
        .route("/permissions/:id/deny", post(deny_permission))
        .route("/events", get(events))
        .with_state(state)
        .layer(CorsLayer::permissive())
}

async fn health(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(json!({
        "status": "ok",
        "daemon": {
            "host": state.config.daemon.host,
            "port": state.config.daemon.port,
        }
    }))
}

async fn providers(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(json!({ "providers": ordered_provider_profiles(&state.config) }))
}

async fn provider_health(AxumPath(id): AxumPath<String>) -> Json<serde_json::Value> {
    let providers = default_provider_profiles();
    let Some(provider) = providers.into_iter().find(|provider| provider.id.0 == id) else {
        return Json(json!({ "error": "provider not found" }));
    };
    Json(json!({ "health": check_provider(&provider) }))
}

async fn provider_validate(AxumPath(id): AxumPath<String>) -> Json<serde_json::Value> {
    let providers = default_provider_profiles();
    let Some(provider) = providers.into_iter().find(|provider| provider.id.0 == id) else {
        return Json(json!({ "error": "provider not found" }));
    };
    Json(json!({ "validation": validate_provider(&provider) }))
}

async fn check_providers(State(state): State<AppState>) -> Json<serde_json::Value> {
    let providers = ordered_provider_profiles(&state.config);
    let health = providers.iter().map(check_provider).collect::<Vec<_>>();
    let event = BaizeEvent::new("provider.health.changed", json!({ "health": health }));
    state.record_event(event);
    Json(json!({ "health": health }))
}

async fn validate_providers(State(state): State<AppState>) -> Json<serde_json::Value> {
    let validations = validate_all_providers();
    let event = BaizeEvent::new(
        "provider.validation.completed",
        json!({ "validations": validations }),
    );
    state.record_event(event);
    Json(json!({ "validations": validations }))
}

async fn workspaces(State(state): State<AppState>) -> Json<serde_json::Value> {
    let workspaces = with_store(&state, |store| store.list_workspaces());
    json_result("workspaces", workspaces)
}

async fn create_workspace(
    State(state): State<AppState>,
    Json(request): Json<CreateWorkspaceRequest>,
) -> Json<serde_json::Value> {
    let status = match baize_workspace::inspect(&request.path) {
        Ok(status) => status,
        Err(error) => return Json(json!({ "error": error.to_string() })),
    };
    let now = Utc::now();
    let workspace_id = WorkspaceId::new();
    let project_id = ProjectId::new();
    let name = request.name.unwrap_or_else(|| {
        status
            .root
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "workspace".to_string())
    });
    let workspace = Workspace {
        id: workspace_id.clone(),
        name,
        primary_project_id: project_id.clone(),
        created_at: now,
        updated_at: now,
    };
    let project = Project {
        id: project_id,
        workspace_id: workspace_id.clone(),
        root: status.git_root.clone().unwrap_or(status.root.clone()),
        kind: if status.git_root.is_some() {
            ProjectKind::GitRepo
        } else {
            ProjectKind::Directory
        },
        vcs: if status.git_root.is_some() {
            VcsKind::Git
        } else {
            VcsKind::None
        },
        active_branch: status.branch,
        trust_level: TrustLevel::Trusted,
        created_at: now,
        updated_at: now,
    };

    let save = with_store(&state, |store| {
        store.upsert_workspace(&workspace)?;
        store.upsert_project(&project)?;
        Ok(())
    });
    if let Err(error) = save {
        return Json(json!({ "error": error.to_string() }));
    }

    let mut event = BaizeEvent::new(
        "workspace.status.changed",
        json!({ "workspace": workspace, "project": project }),
    );
    event.workspace_id = Some(workspace_id);
    state.record_event(event);
    Json(json!({ "workspace": workspace, "project": project }))
}

async fn workspace_status(
    axum::extract::Query(query): axum::extract::Query<WorkspaceStatusQuery>,
) -> Json<serde_json::Value> {
    let path = query.path.unwrap_or_else(|| PathBuf::from("."));
    match baize_workspace::inspect(path) {
        Ok(status) => Json(json!({ "status": status })),
        Err(error) => Json(json!({ "error": error.to_string() })),
    }
}

async fn workspace_status_by_id(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Json<serde_json::Value> {
    let workspace = match with_store(&state, |store| store.get_workspace(&WorkspaceId(id))) {
        Ok(Some(workspace)) => workspace,
        Ok(None) => return Json(json!({ "error": "workspace not found" })),
        Err(error) => return Json(json!({ "error": error.to_string() })),
    };
    let project = match with_store(&state, |store| store.get_primary_project(&workspace)) {
        Ok(Some(project)) => project,
        Ok(None) => return Json(json!({ "error": "project not found" })),
        Err(error) => return Json(json!({ "error": error.to_string() })),
    };
    workspace_status(axum::extract::Query(WorkspaceStatusQuery {
        path: Some(project.root),
    }))
    .await
}

async fn sessions(State(state): State<AppState>) -> Json<serde_json::Value> {
    let sessions = with_store(&state, |store| store.list_task_sessions());
    json_result("sessions", sessions)
}

async fn session(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Json<serde_json::Value> {
    let session = with_store(&state, |store| store.get_task_session(&TaskSessionId(id)));
    json_result("session", session)
}

async fn session_routes(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Json<serde_json::Value> {
    let routes = with_store(&state, |store| {
        store.list_route_decisions_for_session(&TaskSessionId(id))
    });
    json_result("routes", routes)
}

async fn create_session(
    State(state): State<AppState>,
    Json(request): Json<CreateSessionRequest>,
) -> Json<serde_json::Value> {
    let now = Utc::now();
    let workspace_id = WorkspaceId(request.workspace_id);
    let selected_provider_id = select_provider(&state, request.provider_id);
    let session = TaskSession {
        id: TaskSessionId::new(),
        workspace_id: workspace_id.clone(),
        objective: request.objective,
        active_provider_id: Some(selected_provider_id.clone()),
        status: TaskSessionStatus::Running,
        created_at: now,
        updated_at: now,
    };
    let decision = RouteDecision {
        id: RouteDecisionId::new(),
        session_id: session.id.clone(),
        selected_provider_id: selected_provider_id.clone(),
        previous_provider_id: None,
        reason: format!(
            "Selected {} from configured provider priority.",
            selected_provider_id.0
        ),
        confidence: 0.75,
        mode: RoutingMode::Assisted,
        created_at: now,
    };

    let save = with_store(&state, |store| {
        store.upsert_task_session(&session)?;
        store.insert_route_decision(&decision)?;
        Ok(())
    });
    if let Err(error) = save {
        return Json(json!({ "error": error.to_string() }));
    }

    let mut created = BaizeEvent::new("session.created", json!({ "session": session }));
    created.workspace_id = Some(workspace_id.clone());
    created.session_id = Some(session.id.clone());
    created.provider_id = Some(selected_provider_id.clone());
    state.record_event(created);

    let mut routed = BaizeEvent::new("session.route.decided", json!({ "decision": decision }));
    routed.workspace_id = Some(workspace_id);
    routed.session_id = Some(session.id.clone());
    routed.provider_id = Some(selected_provider_id);
    state.record_event(routed);

    Json(json!({ "session": session, "route_decision": decision }))
}

async fn prompt_session(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<PromptRequest>,
) -> Json<serde_json::Value> {
    let session_id = TaskSessionId(id);
    let session = match with_store(&state, |store| store.get_task_session(&session_id)) {
        Ok(Some(session)) => session,
        Ok(None) => return Json(json!({ "error": "session not found" })),
        Err(error) => return Json(json!({ "error": error.to_string() })),
    };
    let provider_id = session
        .active_provider_id
        .clone()
        .unwrap_or_else(|| ProviderId("codex".to_string()));
    let workspace = match with_store(&state, |store| store.get_workspace(&session.workspace_id)) {
        Ok(Some(workspace)) => workspace,
        _ => return Json(json!({ "error": "workspace not found" })),
    };
    let project = match with_store(&state, |store| store.get_primary_project(&workspace)) {
        Ok(Some(project)) => project,
        _ => return Json(json!({ "error": "project not found" })),
    };

    let mut started = BaizeEvent::new(
        "session.agent.started",
        json!({ "prompt": request.prompt, "provider_id": provider_id.0 }),
    );
    started.workspace_id = Some(session.workspace_id.clone());
    started.session_id = Some(session.id.clone());
    started.provider_id = Some(provider_id.clone());
    state.record_event(started);

    let run = state.executor.run_prompt(AgentPromptRequest {
        provider_id: provider_id.clone(),
        prompt: request.prompt,
        cwd: project.root,
        session_id: None,
        timeout_seconds: request.timeout_seconds.or(Some(120)),
    });

    match run {
        Ok(result) => {
            for adapter_event in &result.events {
                let event_type = match adapter_event.kind {
                    AgentExecutionEventKind::Output | AgentExecutionEventKind::Raw => {
                        "session.agent.output"
                    }
                    AgentExecutionEventKind::ToolCall => "session.agent.tool_call",
                };
                let mut event = BaizeEvent::new(
                    event_type,
                    json!({
                        "text": adapter_event.text,
                        "raw": adapter_event.raw,
                    }),
                );
                event.workspace_id = Some(session.workspace_id.clone());
                event.session_id = Some(session.id.clone());
                event.provider_id = Some(provider_id.clone());
                state.record_event(event);
            }

            let final_event_type = if result.success {
                "session.agent.completed"
            } else {
                "session.agent.failed"
            };
            let mut final_event = BaizeEvent::new(
                final_event_type,
                json!({
                    "success": result.success,
                    "exit_code": result.exit_code,
                    "stderr": result.stderr,
                }),
            );
            final_event.workspace_id = Some(session.workspace_id.clone());
            final_event.session_id = Some(session.id.clone());
            final_event.provider_id = Some(provider_id.clone());
            state.record_event(final_event);

            Json(json!({
                "status": if result.success { "completed" } else { "failed" },
                "provider_id": provider_id,
                "events": result.events,
                "exit_code": result.exit_code,
                "stderr": result.stderr,
            }))
        }
        Err(error) => {
            let error = format_error_chain(error.as_ref());
            let mut failed = BaizeEvent::new("session.agent.failed", json!({ "error": error }));
            failed.workspace_id = Some(session.workspace_id.clone());
            failed.session_id = Some(session.id.clone());
            failed.provider_id = Some(provider_id.clone());
            state.record_event(failed);

            Json(json!({
                "status": "failed",
                "provider_id": provider_id,
                "error": error,
            }))
        }
    }
}

async fn cancel_session(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Json<serde_json::Value> {
    let session_id = TaskSessionId(id);
    let mut session = match with_store(&state, |store| store.get_task_session(&session_id)) {
        Ok(Some(session)) => session,
        Ok(None) => return Json(json!({ "error": "session not found" })),
        Err(error) => return Json(json!({ "error": error.to_string() })),
    };
    session.status = TaskSessionStatus::Canceled;
    session.updated_at = Utc::now();
    if let Err(error) = with_store(&state, |store| store.upsert_task_session(&session)) {
        return Json(json!({ "error": error.to_string() }));
    }
    let mut event = BaizeEvent::new("session.agent.completed", json!({ "status": "canceled" }));
    event.workspace_id = Some(session.workspace_id.clone());
    event.session_id = Some(session.id.clone());
    event.provider_id = session.active_provider_id.clone();
    state.record_event(event);
    Json(json!({ "session": session }))
}

async fn create_handoff(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<HandoffRequest>,
) -> Json<serde_json::Value> {
    let session_id = TaskSessionId(id);
    let session = match with_store(&state, |store| store.get_task_session(&session_id)) {
        Ok(Some(session)) => session,
        Ok(None) => return Json(json!({ "error": "session not found" })),
        Err(error) => return Json(json!({ "error": error.to_string() })),
    };
    let from_provider_id = session
        .active_provider_id
        .clone()
        .unwrap_or_else(|| ProviderId("unknown".to_string()));
    let to_provider_id = ProviderId(request.to_provider_id);
    let workspace = match with_store(&state, |store| store.get_workspace(&session.workspace_id)) {
        Ok(Some(workspace)) => workspace,
        _ => return Json(json!({ "error": "workspace not found" })),
    };
    let project = match with_store(&state, |store| store.get_primary_project(&workspace)) {
        Ok(Some(project)) => project,
        _ => return Json(json!({ "error": "project not found" })),
    };
    let status = baize_workspace::inspect(&project.root).ok();
    let changed_files = status
        .as_ref()
        .map(|status| status.changed_files.clone())
        .unwrap_or_default();
    let user_constraints = request.user_constraints.unwrap_or_default();
    let summary_markdown = format!(
        "# Handoff\n\nObjective: {}\n\nFrom: {}\nTo: {}\n\nChanged files: {}\n",
        session.objective,
        from_provider_id.0,
        to_provider_id.0,
        if changed_files.is_empty() {
            "none".to_string()
        } else {
            changed_files.join(", ")
        }
    );
    let handoff = HandoffSummary {
        id: HandoffId::new(),
        session_id: session.id.clone(),
        from_provider_id: from_provider_id.clone(),
        to_provider_id: to_provider_id.clone(),
        summary_markdown,
        mechanical_facts: HandoffFacts {
            changed_files,
            user_constraints,
            ..HandoffFacts::default()
        },
        status: HandoffStatus::Draft,
        created_at: Utc::now(),
    };
    if let Err(error) = with_store(&state, |store| store.insert_handoff(&handoff)) {
        return Json(json!({ "error": error.to_string() }));
    }
    let mut event = BaizeEvent::new("handoff.created", json!({ "handoff": handoff }));
    event.workspace_id = Some(session.workspace_id);
    event.session_id = Some(session.id);
    event.provider_id = Some(to_provider_id);
    state.record_event(event);
    Json(json!({ "handoff": handoff }))
}

async fn accept_handoff(
    State(state): State<AppState>,
    AxumPath((id, handoff_id)): AxumPath<(String, String)>,
) -> Json<serde_json::Value> {
    let session_id = TaskSessionId(id);
    let mut handoff = match with_store(&state, |store| store.get_handoff(&HandoffId(handoff_id))) {
        Ok(Some(handoff)) => handoff,
        Ok(None) => return Json(json!({ "error": "handoff not found" })),
        Err(error) => return Json(json!({ "error": error.to_string() })),
    };
    if handoff.session_id != session_id {
        return Json(json!({ "error": "handoff does not belong to session" }));
    }

    let mut session = match with_store(&state, |store| store.get_task_session(&session_id)) {
        Ok(Some(session)) => session,
        Ok(None) => return Json(json!({ "error": "session not found" })),
        Err(error) => return Json(json!({ "error": error.to_string() })),
    };

    let previous_provider_id = session.active_provider_id.clone();
    let selected_provider_id = handoff.to_provider_id.clone();
    let now = Utc::now();
    session.active_provider_id = Some(selected_provider_id.clone());
    session.updated_at = now;
    handoff.status = HandoffStatus::Accepted;

    let decision = RouteDecision {
        id: RouteDecisionId::new(),
        session_id: session.id.clone(),
        selected_provider_id: selected_provider_id.clone(),
        previous_provider_id,
        reason: format!(
            "Accepted handoff {} to {}.",
            handoff.id.0, selected_provider_id.0
        ),
        confidence: 0.9,
        mode: RoutingMode::Assisted,
        created_at: now,
    };

    let save = with_store(&state, |store| {
        store.upsert_task_session(&session)?;
        store.insert_handoff(&handoff)?;
        store.insert_route_decision(&decision)?;
        Ok(())
    });
    if let Err(error) = save {
        return Json(json!({ "error": error.to_string() }));
    }

    let mut accepted = BaizeEvent::new(
        "handoff.accepted",
        json!({
            "handoff": handoff,
            "session": session,
            "route_decision": decision,
        }),
    );
    accepted.workspace_id = Some(session.workspace_id.clone());
    accepted.session_id = Some(session.id.clone());
    accepted.provider_id = Some(selected_provider_id.clone());
    state.record_event(accepted);

    let mut routed = BaizeEvent::new("session.route.decided", json!({ "decision": decision }));
    routed.workspace_id = Some(session.workspace_id.clone());
    routed.session_id = Some(session.id.clone());
    routed.provider_id = Some(selected_provider_id);
    state.record_event(routed);

    Json(json!({
        "handoff": handoff,
        "session": session,
        "route_decision": decision,
    }))
}

async fn handoff(
    State(state): State<AppState>,
    AxumPath((_session_id, handoff_id)): AxumPath<(String, String)>,
) -> Json<serde_json::Value> {
    let handoff = with_store(&state, |store| store.get_handoff(&HandoffId(handoff_id)));
    json_result("handoff", handoff)
}

async fn session_events(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Json<serde_json::Value> {
    let events = with_store(&state, |store| {
        store.list_events_for_session(&TaskSessionId(id))
    });
    json_result("events", events)
}

async fn session_diff(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Json<serde_json::Value> {
    let session = match with_store(&state, |store| store.get_task_session(&TaskSessionId(id))) {
        Ok(Some(session)) => session,
        Ok(None) => return Json(json!({ "error": "session not found" })),
        Err(error) => return Json(json!({ "error": error.to_string() })),
    };
    let workspace = match with_store(&state, |store| store.get_workspace(&session.workspace_id)) {
        Ok(Some(workspace)) => workspace,
        _ => return Json(json!({ "error": "workspace not found" })),
    };
    let project = match with_store(&state, |store| store.get_primary_project(&workspace)) {
        Ok(Some(project)) => project,
        _ => return Json(json!({ "error": "project not found" })),
    };
    match baize_workspace::inspect(project.root) {
        Ok(status) => Json(json!({ "diff": {
            "dirty": status.dirty,
            "changed_files": status.changed_files,
        }})),
        Err(error) => Json(json!({ "error": error.to_string() })),
    }
}

async fn permissions(
    State(state): State<AppState>,
    axum::extract::Query(query): axum::extract::Query<PermissionsQuery>,
) -> Json<serde_json::Value> {
    let permissions = match with_store(&state, |store| store.list_permissions()) {
        Ok(permissions) => permissions,
        Err(error) => return Json(json!({ "error": error.to_string() })),
    };
    let status = match query.status.as_deref().map(parse_permission_status) {
        Some(Some(status)) => Some(status),
        Some(None) => return Json(json!({ "error": "invalid permission status" })),
        None => None,
    };
    let session_id = query.session_id.as_deref();
    let permissions = permissions
        .into_iter()
        .filter(|permission| {
            status
                .as_ref()
                .is_none_or(|status| permission_status_eq(&permission.status, status))
        })
        .filter(|permission| {
            session_id.is_none_or(|session_id| {
                permission
                    .session_id
                    .as_ref()
                    .is_some_and(|permission_session| permission_session.0 == session_id)
            })
        })
        .collect::<Vec<_>>();

    Json(json!({ "permissions": permissions }))
}

async fn create_permission(
    State(state): State<AppState>,
    Json(request): Json<CreatePermissionRequest>,
) -> Json<serde_json::Value> {
    let permission = PermissionRequest {
        id: PermissionId::new(),
        workspace_id: request.workspace_id.map(WorkspaceId),
        session_id: request.session_id.map(TaskSessionId),
        command: request.command,
        reason: request.reason,
        status: PermissionStatus::Pending,
        created_at: Utc::now(),
        resolved_at: None,
    };
    if let Err(error) = with_store(&state, |store| store.upsert_permission(&permission)) {
        return Json(json!({ "error": error.to_string() }));
    }
    let mut event = BaizeEvent::new("permission.requested", json!({ "permission": permission }));
    event.workspace_id = permission.workspace_id.clone();
    event.session_id = permission.session_id.clone();
    state.record_event(event);
    Json(json!({ "permission": permission }))
}

async fn approve_permission(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Json<serde_json::Value> {
    resolve_permission(state, PermissionId(id), PermissionStatus::Approved)
}

async fn deny_permission(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Json<serde_json::Value> {
    resolve_permission(state, PermissionId(id), PermissionStatus::Denied)
}

fn resolve_permission(
    state: AppState,
    id: PermissionId,
    status: PermissionStatus,
) -> Json<serde_json::Value> {
    let mut permission = match with_store(&state, |store| store.get_permission(&id)) {
        Ok(Some(permission)) => permission,
        Ok(None) => return Json(json!({ "error": "permission not found" })),
        Err(error) => return Json(json!({ "error": error.to_string() })),
    };
    permission.status = status;
    permission.resolved_at = Some(Utc::now());
    if let Err(error) = with_store(&state, |store| store.upsert_permission(&permission)) {
        return Json(json!({ "error": error.to_string() }));
    }
    let mut event = BaizeEvent::new("permission.resolved", json!({ "permission": permission }));
    event.workspace_id = permission.workspace_id.clone();
    event.session_id = permission.session_id.clone();
    state.record_event(event);
    Json(json!({ "permission": permission }))
}

async fn events(
    State(state): State<AppState>,
) -> Sse<impl futures::Stream<Item = Result<Event, Infallible>>> {
    let connected = futures::stream::once(async {
        Ok(Event::default()
            .event("daemon.connected")
            .data(json!({ "status": "connected" }).to_string()))
    });
    let receiver = BroadcastStream::new(state.events.subscribe()).filter_map(|event| match event {
        Ok(event) => Some(Ok(Event::default()
            .event(event.event_type.clone())
            .data(serde_json::to_string(&event).unwrap_or_else(|_| "{}".to_string())))),
        Err(_) => None,
    });
    Sse::new(connected.chain(receiver))
}

fn select_provider(state: &AppState, requested: Option<String>) -> ProviderId {
    if let Some(requested) = requested {
        return ProviderId(requested);
    }
    let providers = default_provider_profiles();
    state
        .config
        .providers
        .order
        .iter()
        .find(|id| {
            providers
                .iter()
                .any(|provider| provider.id.0 == **id && provider.enabled)
        })
        .cloned()
        .map(ProviderId)
        .unwrap_or_else(|| ProviderId("codex".to_string()))
}

fn ordered_provider_profiles(config: &BaizeConfig) -> Vec<baize_core::ProviderProfile> {
    let providers = default_provider_profiles();
    let mut ordered = config
        .providers
        .order
        .iter()
        .filter_map(|id| {
            providers
                .iter()
                .find(|provider| provider.id.0 == *id && provider.enabled)
                .cloned()
        })
        .collect::<Vec<_>>();

    for provider in providers {
        if provider.enabled && !ordered.iter().any(|existing| existing.id == provider.id) {
            ordered.push(provider);
        }
    }

    ordered
}

fn with_store<T>(state: &AppState, f: impl FnOnce(&EventStore) -> Result<T>) -> Result<T> {
    let store = state
        .store
        .lock()
        .map_err(|_| anyhow::anyhow!("storage lock poisoned"))?;
    f(&store)
}

fn json_result<T: Serialize>(key: &str, result: Result<T>) -> Json<serde_json::Value> {
    match result {
        Ok(value) => Json(json!({ key: value })),
        Err(error) => Json(json!({ "error": error.to_string() })),
    }
}

fn parse_permission_status(status: &str) -> Option<PermissionStatus> {
    match status.to_ascii_lowercase().as_str() {
        "pending" => Some(PermissionStatus::Pending),
        "approved" => Some(PermissionStatus::Approved),
        "denied" => Some(PermissionStatus::Denied),
        _ => None,
    }
}

fn permission_status_eq(left: &PermissionStatus, right: &PermissionStatus) -> bool {
    matches!(
        (left, right),
        (PermissionStatus::Pending, PermissionStatus::Pending)
            | (PermissionStatus::Approved, PermissionStatus::Approved)
            | (PermissionStatus::Denied, PermissionStatus::Denied)
    )
}

fn format_error_chain(error: &dyn std::error::Error) -> String {
    let mut parts = vec![error.to_string()];
    let mut source = error.source();
    while let Some(error) = source {
        parts.push(error.to_string());
        source = error.source();
    }
    parts.join(": ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::{Method, Request, StatusCode};
    use tower::ServiceExt;

    #[derive(Clone)]
    struct FakeAgentExecutor {
        result: AgentRunResult,
    }

    impl AgentExecutor for FakeAgentExecutor {
        fn run_prompt(&self, _request: AgentPromptRequest) -> Result<AgentRunResult> {
            Ok(self.result.clone())
        }
    }

    #[derive(Clone)]
    struct FailingAgentExecutor;

    impl AgentExecutor for FailingAgentExecutor {
        fn run_prompt(&self, _request: AgentPromptRequest) -> Result<AgentRunResult> {
            Err(anyhow::anyhow!("inner failure").context("outer failure"))
        }
    }

    fn test_app() -> (Router, tempfile::TempDir, tempfile::TempDir) {
        let data_dir = tempfile::tempdir().expect("data dir");
        let project_dir = tempfile::tempdir().expect("project dir");
        let store = EventStore::open(data_dir.path().join("baize.db")).expect("store");
        let state = AppState::with_executor(
            BaizeConfig::default(),
            store,
            Arc::new(FakeAgentExecutor {
                result: AgentRunResult {
                    provider_id: ProviderId("codex".to_string()),
                    success: true,
                    exit_code: Some(0),
                    events: vec![baize_adapters::AgentExecutionEvent {
                        kind: AgentExecutionEventKind::Output,
                        text: Some("fake output".to_string()),
                        raw: None,
                    }],
                    stderr: String::new(),
                },
            }),
        );
        (router(state), data_dir, project_dir)
    }

    async fn json_response(app: Router, request: Request<Body>) -> serde_json::Value {
        let response = app.oneshot(request).await.expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        serde_json::from_slice(&bytes).expect("json")
    }

    #[tokio::test]
    async fn creates_workspace_session_prompt_and_events() {
        let (app, _data_dir, project_dir) = test_app();
        let workspace = json_response(
            app.clone(),
            Request::builder()
                .method(Method::POST)
                .uri("/workspaces")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "path": project_dir.path(), "name": "test-workspace" }).to_string(),
                ))
                .expect("request"),
        )
        .await;
        let workspace_id = workspace["workspace"]["id"].as_str().expect("workspace id");

        let session = json_response(
            app.clone(),
            Request::builder()
                .method(Method::POST)
                .uri("/sessions")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "workspace_id": workspace_id,
                        "objective": "write tests"
                    })
                    .to_string(),
                ))
                .expect("request"),
        )
        .await;
        let session_id = session["session"]["id"].as_str().expect("session id");
        assert_eq!(session["session"]["active_provider_id"], "codex");

        let prompt = json_response(
            app.clone(),
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{session_id}/prompt"))
                .header("content-type", "application/json")
                .body(Body::from(json!({ "prompt": "hello" }).to_string()))
                .expect("request"),
        )
        .await;
        assert_eq!(prompt["status"], "completed");

        let events = json_response(
            app.clone(),
            Request::builder()
                .uri(format!("/sessions/{session_id}/events"))
                .body(Body::empty())
                .expect("request"),
        )
        .await;
        assert!(events["events"]
            .as_array()
            .expect("events")
            .iter()
            .any(|event| event["event_type"] == "session.agent.completed"));
        assert!(events["events"]
            .as_array()
            .expect("events")
            .iter()
            .any(|event| event["event_type"] == "session.agent.output"
                && event["payload"]["text"] == "fake output"));

        let routes = json_response(
            app.clone(),
            Request::builder()
                .uri(format!("/sessions/{session_id}/routes"))
                .body(Body::empty())
                .expect("request"),
        )
        .await;
        assert_eq!(routes["routes"][0]["selected_provider_id"], "codex");
    }

    #[tokio::test]
    async fn prompt_failure_returns_error_chain() {
        let data_dir = tempfile::tempdir().expect("data dir");
        let project_dir = tempfile::tempdir().expect("project dir");
        let store = EventStore::open(data_dir.path().join("baize.db")).expect("store");
        let state = AppState::with_executor(
            BaizeConfig::default(),
            store,
            Arc::new(FailingAgentExecutor),
        );
        let app = router(state);

        let workspace = json_response(
            app.clone(),
            Request::builder()
                .method(Method::POST)
                .uri("/workspaces")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "path": project_dir.path() }).to_string(),
                ))
                .expect("request"),
        )
        .await;
        let workspace_id = workspace["workspace"]["id"].as_str().expect("workspace id");
        let session = json_response(
            app.clone(),
            Request::builder()
                .method(Method::POST)
                .uri("/sessions")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "workspace_id": workspace_id,
                        "objective": "failure path"
                    })
                    .to_string(),
                ))
                .expect("request"),
        )
        .await;
        let session_id = session["session"]["id"].as_str().expect("session id");

        let prompt = json_response(
            app,
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{session_id}/prompt"))
                .header("content-type", "application/json")
                .body(Body::from(json!({ "prompt": "fail" }).to_string()))
                .expect("request"),
        )
        .await;

        assert_eq!(prompt["status"], "failed");
        assert_eq!(prompt["error"], "outer failure: inner failure");
    }

    #[tokio::test]
    async fn creates_handoff_artifact() {
        let (app, _data_dir, project_dir) = test_app();
        let workspace = json_response(
            app.clone(),
            Request::builder()
                .method(Method::POST)
                .uri("/workspaces")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "path": project_dir.path() }).to_string(),
                ))
                .expect("request"),
        )
        .await;
        let workspace_id = workspace["workspace"]["id"].as_str().expect("workspace id");
        let session = json_response(
            app.clone(),
            Request::builder()
                .method(Method::POST)
                .uri("/sessions")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "workspace_id": workspace_id,
                        "objective": "handoff me",
                        "provider_id": "gemini"
                    })
                    .to_string(),
                ))
                .expect("request"),
        )
        .await;
        let session_id = session["session"]["id"].as_str().expect("session id");

        let handoff = json_response(
            app.clone(),
            Request::builder()
                .method(Method::POST)
                .uri(format!("/sessions/{session_id}/handoff"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "to_provider_id": "codex",
                        "user_constraints": ["do not change public API"]
                    })
                    .to_string(),
                ))
                .expect("request"),
        )
        .await;

        assert_eq!(handoff["handoff"]["from_provider_id"], "gemini");
        assert_eq!(handoff["handoff"]["to_provider_id"], "codex");
        assert_eq!(
            handoff["handoff"]["mechanical_facts"]["user_constraints"][0],
            "do not change public API"
        );

        let handoff_id = handoff["handoff"]["id"].as_str().expect("handoff id");
        let accepted = json_response(
            app.clone(),
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "/sessions/{session_id}/handoff/{handoff_id}/accept"
                ))
                .body(Body::empty())
                .expect("request"),
        )
        .await;
        assert_eq!(accepted["handoff"]["status"], "Accepted");
        assert_eq!(accepted["session"]["active_provider_id"], "codex");
        assert_eq!(accepted["route_decision"]["previous_provider_id"], "gemini");
        assert_eq!(accepted["route_decision"]["selected_provider_id"], "codex");

        let session = json_response(
            app.clone(),
            Request::builder()
                .uri(format!("/sessions/{session_id}"))
                .body(Body::empty())
                .expect("request"),
        )
        .await;
        assert_eq!(session["session"]["active_provider_id"], "codex");

        let events = json_response(
            app,
            Request::builder()
                .uri(format!("/sessions/{session_id}/events"))
                .body(Body::empty())
                .expect("request"),
        )
        .await;
        assert!(events["events"]
            .as_array()
            .expect("events")
            .iter()
            .any(|event| event["event_type"] == "handoff.accepted"));
    }

    #[tokio::test]
    async fn lists_and_filters_permissions() {
        let (app, _data_dir, _project_dir) = test_app();
        let first = json_response(
            app.clone(),
            Request::builder()
                .method(Method::POST)
                .uri("/permissions")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "session_id": "task_one",
                        "command": "cargo test",
                        "reason": "verify changes"
                    })
                    .to_string(),
                ))
                .expect("request"),
        )
        .await;
        let first_id = first["permission"]["id"].as_str().expect("permission id");

        let second = json_response(
            app.clone(),
            Request::builder()
                .method(Method::POST)
                .uri("/permissions")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "session_id": "task_two",
                        "command": "cargo fmt",
                        "reason": "format changes"
                    })
                    .to_string(),
                ))
                .expect("request"),
        )
        .await;
        let second_id = second["permission"]["id"].as_str().expect("permission id");

        let _approved = json_response(
            app.clone(),
            Request::builder()
                .method(Method::POST)
                .uri(format!("/permissions/{second_id}/approve"))
                .body(Body::empty())
                .expect("request"),
        )
        .await;

        let pending = json_response(
            app.clone(),
            Request::builder()
                .uri("/permissions?status=pending")
                .body(Body::empty())
                .expect("request"),
        )
        .await;
        let pending_items = pending["permissions"].as_array().expect("permissions");
        assert_eq!(pending_items.len(), 1);
        assert_eq!(pending_items[0]["id"], first_id);

        let session_filtered = json_response(
            app.clone(),
            Request::builder()
                .uri("/permissions?session_id=task_two")
                .body(Body::empty())
                .expect("request"),
        )
        .await;
        let session_items = session_filtered["permissions"]
            .as_array()
            .expect("permissions");
        assert_eq!(session_items.len(), 1);
        assert_eq!(session_items[0]["id"], second_id);
        assert_eq!(session_items[0]["status"], "Approved");

        let invalid = json_response(
            app,
            Request::builder()
                .uri("/permissions?status=maybe")
                .body(Body::empty())
                .expect("request"),
        )
        .await;
        assert_eq!(invalid["error"], "invalid permission status");
    }

    #[tokio::test]
    async fn validates_known_provider() {
        let (app, _data_dir, _project_dir) = test_app();
        let validation = json_response(
            app,
            Request::builder()
                .uri("/providers/gemini/validate")
                .body(Body::empty())
                .expect("request"),
        )
        .await;

        assert_eq!(validation["validation"]["provider_id"], "gemini");
        assert!(validation["validation"]["detected"].is_object());
    }

    #[tokio::test]
    async fn providers_follow_configured_order() {
        let data_dir = tempfile::tempdir().expect("data dir");
        let store = EventStore::open(data_dir.path().join("baize.db")).expect("store");
        let mut config = BaizeConfig::default();
        config.providers.order = vec!["gemini".to_string(), "codex".to_string()];
        let app = router(AppState::with_executor(
            config,
            store,
            Arc::new(FakeAgentExecutor {
                result: AgentRunResult {
                    provider_id: ProviderId("codex".to_string()),
                    success: true,
                    exit_code: Some(0),
                    events: Vec::new(),
                    stderr: String::new(),
                },
            }),
        ));

        let providers = json_response(
            app,
            Request::builder()
                .uri("/providers")
                .body(Body::empty())
                .expect("request"),
        )
        .await;

        assert_eq!(providers["providers"][0]["id"], "gemini");
        assert_eq!(providers["providers"][1]["id"], "codex");
        assert_eq!(providers["providers"][2]["id"], "copilot");
        assert_eq!(providers["providers"][3]["id"], "opencode");
    }

    #[tokio::test]
    async fn provider_health_check_follows_configured_order() {
        let data_dir = tempfile::tempdir().expect("data dir");
        let store = EventStore::open(data_dir.path().join("baize.db")).expect("store");
        let mut config = BaizeConfig::default();
        config.providers.order = vec!["gemini".to_string(), "codex".to_string()];
        let app = router(AppState::with_executor(
            config,
            store,
            Arc::new(FakeAgentExecutor {
                result: AgentRunResult {
                    provider_id: ProviderId("codex".to_string()),
                    success: true,
                    exit_code: Some(0),
                    events: Vec::new(),
                    stderr: String::new(),
                },
            }),
        ));

        let health = json_response(
            app,
            Request::builder()
                .method(Method::POST)
                .uri("/providers/check")
                .body(Body::empty())
                .expect("request"),
        )
        .await;

        assert_eq!(health["health"][0]["provider_id"], "gemini");
        assert_eq!(health["health"][1]["provider_id"], "codex");
    }
}
