pub use super::prompt_context::{context_preview, context_preview_post};
pub use super::prompts::prompt_session;
use axum::extract::{Path as AxumPath, State};
use axum::http::StatusCode;
use axum::Json;
use baize_core::{
    HandoffStatus, ProviderId, RouteDecision, RouteDecisionId, RoutingMode, TaskSession,
    TaskSessionId, TaskSessionStatus, TaskType,
};
use chrono::Utc;

use crate::helpers::{
    bad_request, infer_task_type, internal_error, json_result, json_result_option, ok_json,
    select_provider, with_store,
};
use crate::state::{
    AppState, CreateSessionRequest, HandoffsQuery, PaginationQuery, RoutesQuery, SessionsQuery,
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

pub(super) fn session_status_label(status: &TaskSessionStatus) -> &'static str {
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
        provider_native_session_ids: Default::default(),
        continuity: Default::default(),
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

pub(crate) fn session_was_canceled(state: &AppState, session_id: &TaskSessionId) -> bool {
    with_store(state, |store| store.get_task_session(session_id))
        .ok()
        .flatten()
        .is_some_and(|session| matches!(session.status, TaskSessionStatus::Canceled))
}

pub async fn cancel_session(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    request_stop(state, id, true)
}

pub async fn stop_execution(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    request_stop(state, id, false)
}

fn request_stop(
    state: AppState,
    id: String,
    cancel_session: bool,
) -> (StatusCode, Json<serde_json::Value>) {
    let session_id = TaskSessionId(id);
    let canceled = with_store(&state, |store| {
        let Some(mut session) = store.get_task_session(&session_id)? else {
            return Ok(None);
        };
        if !cancel_session && session.continuity.active_turn.is_none() {
            return Ok(Some((session, None, None)));
        }
        if cancel_session {
            session.status = TaskSessionStatus::Canceled;
        }
        session.updated_at = Utc::now();
        if let Some(attempt) = session.continuity.active_turn.as_ref() {
            session.continuity.last_stop = Some(baize_core::ExecutionStop {
                attempt_id: attempt.id.clone(),
                provider_id: attempt.provider_id.clone(),
                status: if attempt.uncertain {
                    baize_core::ExecutionStopStatus::Unconfirmed
                } else {
                    baize_core::ExecutionStopStatus::Stopping
                },
                detail: "Stop requested; waiting for native execution evidence.".into(),
            });
        }
        let mut event = baize_core::BaizeEvent::new(
            if cancel_session {
                "session.agent.completed"
            } else {
                "session.execution.stop_requested"
            },
            serde_json::json!({"status":if cancel_session {"canceled"}else{"stopping"},"continuity":session.continuity}),
        );
        event.workspace_id = Some(session.workspace_id.clone());
        event.session_id = Some(session.id.clone());
        event.provider_id = session.active_provider_id.clone();
        store.save_session_event(&session, &event)?;
        let control = match session.continuity.active_turn.as_ref() {
            Some(turn) => state.executions.get(&turn.id)?,
            None => None,
        };
        Ok(Some((session, Some(event), control)))
    });
    match canceled {
        Ok(Some((session, Some(event), control))) => {
            if let Some(control) = control {
                control.cancel();
            }
            let _ = state.events.send(event);
            ok_json(
                serde_json::json!({"session":session,"execution_stop_requested":!cancel_session}),
            )
        }
        Ok(Some((session, None, _))) => {
            if session
                .continuity
                .last_stop
                .as_ref()
                .is_some_and(|stop| stop.status == baize_core::ExecutionStopStatus::Stopped)
            {
                ok_json(
                    serde_json::json!({"session":session,"execution_stop_requested":true,"already_stopped":true}),
                )
            } else {
                bad_request("No active execution to stop")
            }
        }
        Ok(None) => not_found("session not found"),
        Err(error) => internal_error(error.to_string()),
    }
}

pub async fn complete_session(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
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

    if session.continuity.active_turn.is_some() {
        return bad_request("Execution is running or uncertain; reconcile before completing");
    }
    let expected_updated_at = session.updated_at;
    let changed = !matches!(session.status, TaskSessionStatus::Completed);
    session.status = TaskSessionStatus::Completed;
    session.updated_at = Utc::now();
    if let Err(error) = with_store(&state, |store| {
        let latest = store
            .get_task_session(&session_id)?
            .ok_or_else(|| anyhow::anyhow!("session not found"))?;
        if latest.updated_at != expected_updated_at || latest.continuity.active_turn.is_some() {
            anyhow::bail!("Session changed; reconcile before completing");
        }
        store.upsert_task_session(&session)
    }) {
        return internal_error(error.to_string());
    }

    let mut completed = baize_core::BaizeEvent::new(
        "session.completed",
        serde_json::json!({ "status": "Completed" }),
    );
    completed.workspace_id = Some(session.workspace_id.clone());
    completed.session_id = Some(session.id.clone());
    completed.provider_id = session.active_provider_id.clone();
    state.record_event(completed);

    if changed {
        let mut status_event = baize_core::BaizeEvent::new(
            "session.status.changed",
            serde_json::json!({ "status": "Completed" }),
        );
        status_event.workspace_id = Some(session.workspace_id.clone());
        status_event.session_id = Some(session.id.clone());
        status_event.provider_id = session.active_provider_id.clone();
        state.record_event(status_event);
    }

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
