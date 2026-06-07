use axum::extract::{Path as AxumPath, State};
use axum::http::StatusCode;
use axum::Json;
use baize_core::{
    HandoffStatus, ProviderId, RouteDecision, RouteDecisionId, RoutingMode, TaskSession,
    TaskSessionId, TaskSessionStatus, TaskType,
};
use chrono::Utc;

use crate::helpers::{
    bad_request, format_error_chain, infer_provider_limit, infer_task_type, internal_error,
    json_result, json_result_option, ok_json, select_provider, with_store,
};
use crate::state::{
    AppState, CreateSessionRequest, HandoffsQuery, PaginationQuery, PromptRequest, RoutesQuery,
    SessionsQuery,
};

pub async fn sessions(
    State(state): State<AppState>,
    axum::extract::Query(query): axum::extract::Query<SessionsQuery>,
) -> (StatusCode, Json<serde_json::Value>) {
    let status = match query.status.as_deref().map(parse_session_status) {
        Some(Some(status)) => Some(status),
        Some(None) => return bad_request("invalid session status"),
        None => None,
    };
    let provider_id = query
        .active_provider_id
        .as_ref()
        .map(|id| ProviderId(id.clone()));
    let sessions = match with_store(&state, |store| match (&status, &provider_id) {
        (Some(status), _) => store.list_task_sessions_by_status(
            session_status_label(status),
            query.limit,
            query.offset,
        ),
        (None, Some(provider_id)) => {
            store.list_task_sessions_by_active_provider(provider_id, query.limit, query.offset)
        }
        (None, None) => store.list_task_sessions(query.limit, query.offset),
    }) {
        Ok(sessions) => sessions,
        Err(error) => return internal_error(error.to_string()),
    };
    let workspace_id = query.workspace_id.as_deref();
    let sessions = sessions
        .into_iter()
        .filter(|session| {
            status
                .as_ref()
                .is_none_or(|status| session_status_eq(&session.status, status))
        })
        .filter(|session| {
            provider_id.as_ref().is_none_or(|provider_id| {
                session
                    .active_provider_id
                    .as_ref()
                    .is_some_and(|session_provider| session_provider.0 == provider_id.0)
            })
        })
        .filter(|session| {
            workspace_id.is_none_or(|workspace_id| session.workspace_id.0 == workspace_id)
        })
        .collect::<Vec<_>>();

    ok_json(serde_json::json!({ "sessions": sessions }))
}

fn parse_session_status(value: &str) -> Option<TaskSessionStatus> {
    match value.to_ascii_lowercase().replace('_', "").as_str() {
        "running" => Some(TaskSessionStatus::Running),
        "waitingforpermission" => Some(TaskSessionStatus::WaitingForPermission),
        "completed" => Some(TaskSessionStatus::Completed),
        "failed" => Some(TaskSessionStatus::Failed),
        "canceled" | "cancelled" => Some(TaskSessionStatus::Canceled),
        _ => None,
    }
}

fn session_status_label(status: &TaskSessionStatus) -> &'static str {
    match status {
        TaskSessionStatus::Running => "Running",
        TaskSessionStatus::WaitingForPermission => "WaitingForPermission",
        TaskSessionStatus::Completed => "Completed",
        TaskSessionStatus::Failed => "Failed",
        TaskSessionStatus::Canceled => "Canceled",
    }
}

fn session_status_eq(left: &TaskSessionStatus, right: &TaskSessionStatus) -> bool {
    matches!(
        (left, right),
        (TaskSessionStatus::Running, TaskSessionStatus::Running)
            | (
                TaskSessionStatus::WaitingForPermission,
                TaskSessionStatus::WaitingForPermission
            )
            | (TaskSessionStatus::Completed, TaskSessionStatus::Completed)
            | (TaskSessionStatus::Failed, TaskSessionStatus::Failed)
            | (TaskSessionStatus::Canceled, TaskSessionStatus::Canceled)
    )
}

fn parse_task_type(value: &str) -> Option<TaskType> {
    match value.to_ascii_lowercase().as_str() {
        "testing" | "test" => Some(TaskType::Testing),
        "debugging" | "debug" => Some(TaskType::Debugging),
        "refactor" => Some(TaskType::Refactor),
        "documentation" | "docs" | "doc" => Some(TaskType::Documentation),
        "implementation" | "implement" => Some(TaskType::Implementation),
        _ => None,
    }
}

fn task_type_label(task_type: &TaskType) -> &'static str {
    match task_type {
        TaskType::Testing => "Testing",
        TaskType::Debugging => "Debugging",
        TaskType::Refactor => "Refactor",
        TaskType::Documentation => "Documentation",
        TaskType::Implementation => "Implementation",
    }
}

fn task_type_eq(left: &TaskType, right: &TaskType) -> bool {
    matches!(
        (left, right),
        (TaskType::Testing, TaskType::Testing)
            | (TaskType::Debugging, TaskType::Debugging)
            | (TaskType::Refactor, TaskType::Refactor)
            | (TaskType::Documentation, TaskType::Documentation)
            | (TaskType::Implementation, TaskType::Implementation)
    )
}

fn parse_routing_mode(value: &str) -> Option<RoutingMode> {
    match value.to_ascii_lowercase().as_str() {
        "manual" => Some(RoutingMode::Manual),
        "assisted" => Some(RoutingMode::Assisted),
        "autopilot" => Some(RoutingMode::Autopilot),
        _ => None,
    }
}

fn routing_mode_label(mode: &RoutingMode) -> &'static str {
    match mode {
        RoutingMode::Manual => "Manual",
        RoutingMode::Assisted => "Assisted",
        RoutingMode::Autopilot => "Autopilot",
    }
}

fn routing_mode_eq(left: &RoutingMode, right: &RoutingMode) -> bool {
    matches!(
        (left, right),
        (RoutingMode::Manual, RoutingMode::Manual)
            | (RoutingMode::Assisted, RoutingMode::Assisted)
            | (RoutingMode::Autopilot, RoutingMode::Autopilot)
    )
}

fn parse_handoff_status(value: &str) -> Option<HandoffStatus> {
    match value.to_ascii_lowercase().as_str() {
        "draft" => Some(HandoffStatus::Draft),
        "accepted" => Some(HandoffStatus::Accepted),
        "failed" => Some(HandoffStatus::Failed),
        _ => None,
    }
}

fn handoff_status_label(status: &HandoffStatus) -> &'static str {
    match status {
        HandoffStatus::Draft => "Draft",
        HandoffStatus::Accepted => "Accepted",
        HandoffStatus::Failed => "Failed",
    }
}

fn handoff_status_eq(left: &HandoffStatus, right: &HandoffStatus) -> bool {
    matches!(
        (left, right),
        (HandoffStatus::Draft, HandoffStatus::Draft)
            | (HandoffStatus::Accepted, HandoffStatus::Accepted)
            | (HandoffStatus::Failed, HandoffStatus::Failed)
    )
}

pub async fn session(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    let session = with_store(&state, |store| store.get_task_session(&TaskSessionId(id)));
    json_result_option("session", session)
}

pub async fn session_routes(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    axum::extract::Query(query): axum::extract::Query<RoutesQuery>,
) -> (StatusCode, Json<serde_json::Value>) {
    let session_id = TaskSessionId(id);
    let task_type = match query.task_type.as_deref().map(parse_task_type) {
        Some(Some(task_type)) => Some(task_type),
        Some(None) => return bad_request("invalid route task type"),
        None => None,
    };
    let mode = match query.mode.as_deref().map(parse_routing_mode) {
        Some(Some(mode)) => Some(mode),
        Some(None) => return bad_request("invalid route mode"),
        None => None,
    };
    let provider_id = query
        .selected_provider_id
        .as_ref()
        .map(|id| ProviderId(id.clone()));
    let routes = match with_store(&state, |store| match (&provider_id, &task_type, &mode) {
        (Some(provider_id), _, _) => {
            store.list_route_decisions_for_session_by_selected_provider(&session_id, provider_id)
        }
        (None, Some(task_type), _) => store
            .list_route_decisions_for_session_by_task_type(&session_id, task_type_label(task_type)),
        (None, None, Some(mode)) => {
            store.list_route_decisions_for_session_by_mode(&session_id, routing_mode_label(mode))
        }
        (None, None, None) => store.list_route_decisions_for_session(&session_id),
    }) {
        Ok(routes) => routes,
        Err(error) => return internal_error(error.to_string()),
    };
    let routes = routes
        .into_iter()
        .filter(|route| {
            provider_id
                .as_ref()
                .is_none_or(|provider_id| route.selected_provider_id.0 == provider_id.0)
        })
        .filter(|route| {
            task_type.as_ref().is_none_or(|task_type| {
                route
                    .task_type
                    .as_ref()
                    .is_some_and(|route_task_type| task_type_eq(route_task_type, task_type))
            })
        })
        .filter(|route| {
            mode.as_ref()
                .is_none_or(|mode| routing_mode_eq(&route.mode, mode))
        })
        .collect::<Vec<_>>();

    ok_json(serde_json::json!({ "routes": routes }))
}

pub async fn session_handoffs(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    axum::extract::Query(query): axum::extract::Query<HandoffsQuery>,
) -> (StatusCode, Json<serde_json::Value>) {
    let session_id = TaskSessionId(id);
    let status = match query.status.as_deref().map(parse_handoff_status) {
        Some(Some(status)) => Some(status),
        Some(None) => return bad_request("invalid handoff status"),
        None => None,
    };
    let to_provider_id = query
        .to_provider_id
        .as_ref()
        .map(|id| ProviderId(id.clone()));
    let handoffs = match with_store(&state, |store| match (&status, &to_provider_id) {
        (Some(status), _) => {
            store.list_handoffs_for_session_by_status(&session_id, handoff_status_label(status))
        }
        (None, Some(provider_id)) => {
            store.list_handoffs_for_session_by_to_provider(&session_id, provider_id)
        }
        (None, None) => store.list_handoffs_for_session(&session_id),
    }) {
        Ok(handoffs) => handoffs,
        Err(error) => return internal_error(error.to_string()),
    };
    let handoffs = handoffs
        .into_iter()
        .filter(|handoff| {
            status
                .as_ref()
                .is_none_or(|status| handoff_status_eq(&handoff.status, status))
        })
        .filter(|handoff| {
            to_provider_id
                .as_ref()
                .is_none_or(|provider_id| handoff.to_provider_id.0 == provider_id.0)
        })
        .collect::<Vec<_>>();

    ok_json(serde_json::json!({ "handoffs": handoffs }))
}

pub async fn session_permissions(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    axum::extract::Query(query): axum::extract::Query<PaginationQuery>,
) -> (StatusCode, Json<serde_json::Value>) {
    let permissions = with_store(&state, |store| {
        store.list_permissions_for_session(&TaskSessionId(id), query.limit, query.offset)
    });
    json_result("permissions", permissions)
}

pub async fn create_session(
    State(state): State<AppState>,
    Json(request): Json<CreateSessionRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    let now = Utc::now();
    let workspace_id = baize_core::WorkspaceId(request.workspace_id);
    let task_type = infer_task_type(&request.objective);
    if let Some(provider_id) = request.provider_id.as_deref() {
        if !baize_adapters::is_prompt_runtime_supported(&baize_core::ProviderId(
            provider_id.to_string(),
        )) {
            return crate::helpers::bad_request(&format!(
                "provider {provider_id} does not support Baize prompt execution yet"
            ));
        }
    }
    let routing = select_provider(
        &state,
        request.provider_id,
        Some(&workspace_id),
        request.provider_reason,
    );
    let session = TaskSession {
        id: TaskSessionId::new(),
        workspace_id: workspace_id.clone(),
        objective: request.objective,
        active_provider_id: Some(routing.provider_id.clone()),
        status: TaskSessionStatus::Running,
        created_at: now,
        updated_at: now,
    };
    let decision = RouteDecision {
        id: RouteDecisionId::new(),
        session_id: session.id.clone(),
        selected_provider_id: routing.provider_id.clone(),
        previous_provider_id: routing.previous_provider_id,
        reason: format!("{} Task hint: {:?}.", routing.reason, task_type),
        task_type: Some(task_type),
        confidence: routing.confidence,
        mode: RoutingMode::Assisted,
        created_at: now,
    };

    let save = with_store(&state, |store| {
        store.upsert_task_session(&session)?;
        store.insert_route_decision(&decision)?;
        Ok(())
    });
    if let Err(error) = save {
        return internal_error(error.to_string());
    }

    let mut created =
        baize_core::BaizeEvent::new("session.created", serde_json::json!({ "session": session }));
    created.workspace_id = Some(workspace_id.clone());
    created.session_id = Some(session.id.clone());
    created.provider_id = Some(routing.provider_id.clone());
    state.record_event(created);

    let mut routed = baize_core::BaizeEvent::new(
        "session.route.decided",
        serde_json::json!({ "decision": decision }),
    );
    routed.workspace_id = Some(workspace_id);
    routed.session_id = Some(session.id.clone());
    routed.provider_id = Some(routing.provider_id);
    state.record_event(routed);

    ok_json(serde_json::json!({ "session": session, "route_decision": decision }))
}

pub async fn prompt_session(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<PromptRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    let session_id = TaskSessionId(id);
    let mut session = match with_store(&state, |store| store.get_task_session(&session_id)) {
        Ok(Some(session)) => session,
        Ok(None) => return not_found("session not found"),
        Err(error) => return internal_error(error.to_string()),
    };
    if matches!(session.status, TaskSessionStatus::Canceled) {
        return bad_request("session is canceled");
    }
    let provider_id = match apply_prompt_provider_target(&state, &mut session, &request) {
        Ok(provider_id) => provider_id,
        Err(response) => return response,
    };
    if !baize_adapters::is_prompt_runtime_supported(&provider_id) {
        return crate::helpers::bad_request(&format!(
            "provider {} does not support Baize prompt execution yet",
            provider_id.0
        ));
    }
    let workspace = match with_store(&state, |store| store.get_workspace(&session.workspace_id)) {
        Ok(Some(workspace)) => workspace,
        _ => return not_found("workspace not found"),
    };
    let project = match with_store(&state, |store| store.get_primary_project(&workspace)) {
        Ok(Some(project)) => project,
        _ => return not_found("project not found"),
    };

    session.status = TaskSessionStatus::Running;
    session.updated_at = Utc::now();
    if let Err(error) = with_store(&state, |store| store.upsert_task_session(&session)) {
        return internal_error(error.to_string());
    }

    let mut started = baize_core::BaizeEvent::new(
        "session.agent.started",
        serde_json::json!({ "prompt": request.prompt, "provider_id": provider_id.0 }),
    );
    started.workspace_id = Some(session.workspace_id.clone());
    started.session_id = Some(session.id.clone());
    started.provider_id = Some(provider_id.clone());
    state.record_event(started);

    let run = state
        .executor
        .run_prompt(baize_adapters::AgentPromptRequest {
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
                    baize_adapters::AgentExecutionEventKind::Output
                    | baize_adapters::AgentExecutionEventKind::Raw => "session.agent.output",
                    baize_adapters::AgentExecutionEventKind::ToolCall => "session.agent.tool_call",
                };
                let mut event = baize_core::BaizeEvent::new(
                    event_type,
                    serde_json::json!({
                        "text": adapter_event.text,
                        "raw": adapter_event.raw,
                    }),
                );
                event.workspace_id = Some(session.workspace_id.clone());
                event.session_id = Some(session.id.clone());
                event.provider_id = Some(provider_id.clone());
                state.record_event(event);
            }

            let new_status = if result.success {
                TaskSessionStatus::Running
            } else {
                TaskSessionStatus::Failed
            };
            let final_event_type = if result.success {
                "session.agent.completed"
            } else {
                "session.agent.failed"
            };
            let stderr = result.stderr.clone();
            let provider_error = result.error.clone();
            let native_session_id = result.native_session_id.clone();
            let limit_inference = if result.success {
                None
            } else {
                infer_provider_limit(&stderr)
            };
            let mut final_event = baize_core::BaizeEvent::new(
                final_event_type,
                serde_json::json!({
                    "success": result.success,
                    "exit_code": result.exit_code,
                    "native_session_id": native_session_id,
                    "stderr": stderr,
                    "provider_error": provider_error,
                    "limit_inference": limit_inference,
                }),
            );
            final_event.workspace_id = Some(session.workspace_id.clone());
            final_event.session_id = Some(session.id.clone());
            final_event.provider_id = Some(provider_id.clone());
            state.record_event(final_event);

            session.status = new_status.clone();
            session.updated_at = Utc::now();
            if let Err(error) = with_store(&state, |store| store.upsert_task_session(&session)) {
                return internal_error(error.to_string());
            }

            let mut status_event = baize_core::BaizeEvent::new(
                "session.status.changed",
                serde_json::json!({ "status": new_status }),
            );
            status_event.workspace_id = Some(session.workspace_id.clone());
            status_event.session_id = Some(session.id.clone());
            status_event.provider_id = Some(provider_id.clone());
            state.record_event(status_event);

            let code = if result.success {
                StatusCode::OK
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            (
                code,
                Json(serde_json::json!({
                    "status": if result.success { "running" } else { "failed" },
                    "turn_status": if result.success { "completed" } else { "failed" },
                    "session_status": session_status_label(&new_status),
                    "session_id": session.id.0,
                    "provider_id": provider_id,
                    "events": result.events,
                    "exit_code": result.exit_code,
                    "native_session_id": native_session_id,
                    "stderr": stderr,
                    "provider_error": provider_error,
                    "limit_inference": limit_inference,
                })),
            )
        }
        Err(error) => {
            let error = format_error_chain(error.as_ref());
            let limit_inference = infer_provider_limit(&error);
            let mut failed = baize_core::BaizeEvent::new(
                "session.agent.failed",
                serde_json::json!({
                    "error": error,
                    "limit_inference": limit_inference,
                }),
            );
            failed.workspace_id = Some(session.workspace_id.clone());
            failed.session_id = Some(session.id.clone());
            failed.provider_id = Some(provider_id.clone());
            state.record_event(failed);

            session.status = TaskSessionStatus::Failed;
            session.updated_at = Utc::now();
            if let Err(store_error) =
                with_store(&state, |store| store.upsert_task_session(&session))
            {
                return internal_error(store_error.to_string());
            }

            let mut status_event = baize_core::BaizeEvent::new(
                "session.status.changed",
                serde_json::json!({ "status": "Failed" }),
            );
            status_event.workspace_id = Some(session.workspace_id.clone());
            status_event.session_id = Some(session.id.clone());
            status_event.provider_id = Some(provider_id.clone());
            state.record_event(status_event);

            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "status": "failed",
                    "turn_status": "failed",
                    "session_status": "Failed",
                    "session_id": session.id.0,
                    "provider_id": provider_id,
                    "error": error,
                    "limit_inference": limit_inference,
                })),
            )
        }
    }
}

fn apply_prompt_provider_target(
    state: &AppState,
    session: &mut TaskSession,
    request: &PromptRequest,
) -> Result<ProviderId, (StatusCode, Json<serde_json::Value>)> {
    let current_provider = session
        .active_provider_id
        .clone()
        .unwrap_or_else(|| ProviderId("codex".to_string()));
    let Some(requested_provider) = request.provider_id.as_ref() else {
        return Ok(current_provider);
    };
    let requested_provider = ProviderId(requested_provider.clone());
    if requested_provider == current_provider {
        return Ok(current_provider);
    }

    let now = Utc::now();
    let task_type = infer_task_type(&request.prompt);
    let decision = RouteDecision {
        id: RouteDecisionId::new(),
        session_id: session.id.clone(),
        selected_provider_id: requested_provider.clone(),
        previous_provider_id: Some(current_provider),
        reason: format!(
            "Prompt target provider override. Task hint: {:?}.",
            task_type
        ),
        task_type: Some(task_type),
        confidence: 1.0,
        mode: RoutingMode::Manual,
        created_at: now,
    };

    session.active_provider_id = Some(requested_provider.clone());
    session.updated_at = now;
    if let Err(error) = with_store(state, |store| {
        store.upsert_task_session(session)?;
        store.insert_route_decision(&decision)?;
        Ok(())
    }) {
        return Err(internal_error(error.to_string()));
    }

    let mut routed = baize_core::BaizeEvent::new(
        "session.route.decided",
        serde_json::json!({ "decision": decision }),
    );
    routed.workspace_id = Some(session.workspace_id.clone());
    routed.session_id = Some(session.id.clone());
    routed.provider_id = Some(requested_provider.clone());
    state.record_event(routed);

    Ok(requested_provider)
}

pub async fn cancel_session(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    let session_id = TaskSessionId(id);
    let mut session = match with_store(&state, |store| store.get_task_session(&session_id)) {
        Ok(Some(session)) => session,
        Ok(None) => return not_found("session not found"),
        Err(error) => return internal_error(error.to_string()),
    };
    session.status = TaskSessionStatus::Canceled;
    session.updated_at = Utc::now();
    if let Err(error) = with_store(&state, |store| store.upsert_task_session(&session)) {
        return internal_error(error.to_string());
    }
    let mut event = baize_core::BaizeEvent::new(
        "session.agent.completed",
        serde_json::json!({ "status": "canceled" }),
    );
    event.workspace_id = Some(session.workspace_id.clone());
    event.session_id = Some(session.id.clone());
    event.provider_id = session.active_provider_id.clone();
    state.record_event(event);
    ok_json(serde_json::json!({ "session": session }))
}

pub async fn session_events(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    axum::extract::Query(query): axum::extract::Query<PaginationQuery>,
) -> (StatusCode, Json<serde_json::Value>) {
    let events = with_store(&state, |store| {
        store.list_events_for_session(&TaskSessionId(id), query.limit, query.offset)
    });
    json_result("events", events)
}

pub async fn session_diff(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    let session = match with_store(&state, |store| store.get_task_session(&TaskSessionId(id))) {
        Ok(Some(session)) => session,
        Ok(None) => return not_found("session not found"),
        Err(error) => return internal_error(error.to_string()),
    };
    let workspace = match with_store(&state, |store| store.get_workspace(&session.workspace_id)) {
        Ok(Some(workspace)) => workspace,
        _ => return not_found("workspace not found"),
    };
    let project = match with_store(&state, |store| store.get_primary_project(&workspace)) {
        Ok(Some(project)) => project,
        _ => return not_found("project not found"),
    };
    let root = project.root;
    let status = match baize_workspace::inspect(&root) {
        Ok(status) => status,
        Err(error) => return internal_error(error.to_string()),
    };
    let hunks = match baize_workspace::diff_hunks(&root) {
        Ok(hunks) => hunks,
        Err(error) => return internal_error(error.to_string()),
    };
    ok_json(serde_json::json!({ "diff": {
        "dirty": status.dirty,
        "changed_files": status.changed_files,
        "hunks": hunks,
    }}))
}

use crate::helpers::not_found;
