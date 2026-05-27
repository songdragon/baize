use anyhow::Result;
use axum::extract::{Path as AxumPath, State};
use axum::response::sse::{Event, Sse};
use axum::routing::{get, post};
use axum::{Json, Router};
use baize_adapters::{check_provider, default_provider_profiles};
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
        let (events, _) = broadcast::channel(256);
        Self {
            config,
            store: Arc::new(Mutex::new(store)),
            events,
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
        .route("/providers/check", post(check_providers))
        .route("/workspaces/status", get(workspace_status))
        .route("/sessions", get(sessions).post(create_session))
        .route("/sessions/:id", get(session))
        .route("/sessions/:id/prompt", post(prompt_session))
        .route("/sessions/:id/cancel", post(cancel_session))
        .route("/sessions/:id/handoff", post(create_handoff))
        .route("/sessions/:id/events", get(session_events))
        .route("/sessions/:id/diff", get(session_diff))
        .route("/sessions/:id/handoff/:handoff_id", get(handoff))
        .route("/permissions", post(create_permission))
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

async fn providers() -> Json<serde_json::Value> {
    Json(json!({ "providers": default_provider_profiles() }))
}

async fn provider_health(AxumPath(id): AxumPath<String>) -> Json<serde_json::Value> {
    let providers = default_provider_profiles();
    let Some(provider) = providers.into_iter().find(|provider| provider.id.0 == id) else {
        return Json(json!({ "error": "provider not found" }));
    };
    Json(json!({ "health": check_provider(&provider) }))
}

async fn check_providers(State(state): State<AppState>) -> Json<serde_json::Value> {
    let providers = default_provider_profiles();
    let health = providers.iter().map(check_provider).collect::<Vec<_>>();
    let event = BaizeEvent::new("provider.health.changed", json!({ "health": health }));
    state.record_event(event);
    Json(json!({ "health": health }))
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

    for (event_type, payload) in [
        (
            "session.agent.started",
            json!({ "prompt": request.prompt, "provider_id": provider_id.0 }),
        ),
        (
            "session.agent.output",
            json!({ "text": "MVP prompt accepted; real provider execution is not enabled yet." }),
        ),
        ("session.agent.completed", json!({ "status": "completed" })),
    ] {
        let mut event = BaizeEvent::new(event_type, payload);
        event.workspace_id = Some(session.workspace_id.clone());
        event.session_id = Some(session.id.clone());
        event.provider_id = Some(provider_id.clone());
        state.record_event(event);
    }

    Json(json!({
        "status": "accepted",
        "provider_id": provider_id,
        "message": "MVP prompt accepted; real provider execution is not enabled yet."
    }))
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::{Method, Request, StatusCode};
    use tower::ServiceExt;

    fn test_app() -> (Router, tempfile::TempDir, tempfile::TempDir) {
        let data_dir = tempfile::tempdir().expect("data dir");
        let project_dir = tempfile::tempdir().expect("project dir");
        let store = EventStore::open(data_dir.path().join("baize.db")).expect("store");
        let state = AppState::new(BaizeConfig::default(), store);
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
        assert_eq!(prompt["status"], "accepted");

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
            .any(|event| event["event_type"] == "session.agent.completed"));
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
            app,
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
    }
}
