use axum::extract::{Path as AxumPath, State};
use axum::http::StatusCode;
use axum::Json;
use baize_core::{
    ProviderId, RouteDecision, RouteDecisionId, RoutingMode, TaskSession, TaskSessionId,
    TaskSessionStatus,
};
use chrono::Utc;

use crate::helpers::{
    bad_request, format_error_chain, infer_provider_limit, infer_task_type, internal_error,
    json_result, json_result_option, ok_json, select_provider, with_store,
};
use crate::state::{AppState, CreateSessionRequest, PaginationQuery, PromptRequest};

pub async fn sessions(
    State(state): State<AppState>,
    axum::extract::Query(query): axum::extract::Query<PaginationQuery>,
) -> (StatusCode, Json<serde_json::Value>) {
    let sessions = with_store(&state, |store| {
        store.list_task_sessions(query.limit, query.offset)
    });
    json_result("sessions", sessions)
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
) -> (StatusCode, Json<serde_json::Value>) {
    let routes = with_store(&state, |store| {
        store.list_route_decisions_for_session(&TaskSessionId(id))
    });
    json_result("routes", routes)
}

pub async fn session_handoffs(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    let handoffs = with_store(&state, |store| {
        store.list_handoffs_for_session(&TaskSessionId(id))
    });
    json_result("handoffs", handoffs)
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
    let provider_id = session
        .active_provider_id
        .clone()
        .unwrap_or_else(|| ProviderId("codex".to_string()));
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
                    "session_id": session.id.0,
                    "provider_id": provider_id,
                    "events": result.events,
                    "exit_code": result.exit_code,
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
                    "session_id": session.id.0,
                    "provider_id": provider_id,
                    "error": error,
                    "limit_inference": limit_inference,
                })),
            )
        }
    }
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
    match baize_workspace::inspect(project.root) {
        Ok(status) => ok_json(serde_json::json!({ "diff": {
            "dirty": status.dirty,
            "changed_files": status.changed_files,
        }})),
        Err(error) => internal_error(error.to_string()),
    }
}

use crate::helpers::not_found;
