use super::{
    prompt_context::{
        apply_prompt_provider_target, prepare_context_pack, record_adapter_event,
        validate_prompt_image_paths,
    },
    sessions::{session_status_label, session_was_canceled},
};
use crate::{
    helpers::{
        format_error_chain, infer_provider_limit, internal_error, not_found, ok_json, with_store,
    },
    state::{AppState, PromptRequest},
};
use axum::{
    extract::{Path as AxumPath, State},
    http::StatusCode,
    Json,
};
use baize_core::{TaskSessionId, TaskSessionStatus};
use chrono::Utc;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

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
    if let Some(command) = crate::continuity::parse_command(&request.prompt) {
        if !request.image_paths.is_empty() {
            return reject_prompt("Local continuity commands do not accept image attachments");
        }
        return match command.and_then(|command| {
            crate::continuity::apply_command(&state, &session_id, command, &request.prompt)
        }) {
            Ok(response) => ok_json(response),
            Err(error) => (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error":error.to_string(),"not_submitted":true})),
            ),
        };
    }
    if request.prompt.trim().is_empty() {
        return reject_prompt("prompt must not be empty");
    }
    if matches!(session.status, TaskSessionStatus::Canceled) {
        return reject_prompt("session is canceled");
    }
    let provider_id = request
        .provider_id
        .as_ref()
        .map(|id| baize_core::ProviderId(id.clone()))
        .or_else(|| session.active_provider_id.clone())
        .unwrap_or_else(|| baize_core::ProviderId("codex".into()));
    if !baize_adapters::is_prompt_runtime_supported(&provider_id) {
        return reject_prompt(&format!(
            "provider {} does not support Baize prompt execution yet",
            provider_id.0
        ));
    }
    if !request.image_paths.is_empty() && !baize_adapters::supports_file_attachment(&provider_id) {
        return reject_prompt(&format!(
            "provider {} does not support image attachments",
            provider_id.0
        ));
    }
    if let Err(error) = validate_prompt_image_paths(&request.image_paths) {
        return reject_prompt(&error);
    }

    let mut turn = match crate::continuity::TurnGuard::begin(&state, &mut session) {
        Ok(turn) => turn,
        Err(error) => {
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({"error":error.to_string(),"not_submitted":true})),
            )
        }
    };
    if let Err(error) = crate::continuity::import_requirements(&state, &mut session) {
        return internal_error(error.to_string());
    }

    if let Some(attempt) = session.continuity.active_turn.as_mut() {
        attempt.provider_id = provider_id.clone();
    }
    session.status = TaskSessionStatus::Running;
    session.updated_at = Utc::now();

    let native_resume_session_id = session
        .provider_native_session_ids
        .get(&provider_id.0)
        .cloned();
    let prepared_context =
        match prepare_context_pack(&state, &session, &provider_id, &request.prompt) {
            Ok(prepared) => prepared,
            Err(error) => return internal_error(error.to_string()),
        };
    if prepared_context.result.blocked {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(
                serde_json::json!({"error":"Required context exceeds budget; inspect Context preview before retrying", "context":prepared_context.summary,"not_submitted":true}),
            ),
        );
    }
    let profile = crate::context::provider_context_profile(&provider_id, &state.config);
    let execution_identity =
        crate::context::execution_identity(&profile, &prepared_context.project);
    if session
        .continuity
        .checkpoints
        .get(&provider_id.0)
        .is_some_and(|checkpoint| checkpoint.execution_identity != execution_identity)
    {
        return (
            StatusCode::CONFLICT,
            Json(
                serde_json::json!({"error":format!("Execution entry changed. Review and /reset-provider {} before starting a new native session",provider_id.0),"not_submitted":true}),
            ),
        );
    }
    if let Err(response) = apply_prompt_provider_target(&state, &mut session, &request) {
        return response;
    }
    let context_summary = prepared_context.summary.clone();
    let prompt_for_agent = prepared_context.result.prompt.clone();

    let mut context_event = baize_core::BaizeEvent::new(
        "session.context.built",
        serde_json::json!({ "context": context_summary, "prompt_preview":crate::context::prompt_preview(&prompt_for_agent) }),
    );
    context_event.workspace_id = Some(session.workspace_id.clone());
    context_event.session_id = Some(session.id.clone());
    context_event.provider_id = Some(provider_id.clone());
    state.record_event(context_event);

    let project = prepared_context.project;

    let mut started = baize_core::BaizeEvent::new(
        "session.agent.started",
        serde_json::json!({
            "prompt": request.prompt,
            "provider_id": provider_id.0,
            "image_paths": request.image_paths.clone(),
        }),
    );
    started.workspace_id = Some(session.workspace_id.clone());
    started.session_id = Some(session.id.clone());
    started.provider_id = Some(provider_id.clone());
    started.id = turn.id.clone();
    match crate::continuity::save_turn(&state, &session, &started, &turn.id) {
        Ok(true) => {}
        Ok(false) => {
            return (
                StatusCode::CONFLICT,
                Json(
                    serde_json::json!({"error":"Session changed before dispatch","not_submitted":true}),
                ),
            )
        }
        Err(error) => return internal_error(error.to_string()),
    }
    let _ = state.events.send(started);
    turn.dispatched = true;

    let streamed_event_count = Arc::new(AtomicUsize::new(0));
    let agent_request = baize_adapters::AgentPromptRequest {
        provider_id: provider_id.clone(),
        prompt: prompt_for_agent,
        cwd: project.root,
        session_id: native_resume_session_id.clone(),
        image_paths: request.image_paths,
        timeout_seconds: request.timeout_seconds.or(Some(120)),
        execution_policy: baize_adapters::AgentExecutionPolicy::from_command_policy(
            &state.config.workspace.command_policy,
        ),
    };
    let run = {
        let executor = state.executor.clone();
        let stream_state = state.clone();
        let stream_workspace_id = session.workspace_id.clone();
        let stream_session_id = session.id.clone();
        let stream_provider_id = provider_id.clone();
        let stream_count = streamed_event_count.clone();
        match tokio::task::spawn_blocking(move || {
            let mut event_sink = |adapter_event: baize_adapters::AgentExecutionEvent| {
                stream_count.fetch_add(1, Ordering::SeqCst);
                record_adapter_event(
                    &stream_state,
                    &stream_workspace_id,
                    &stream_session_id,
                    &stream_provider_id,
                    &adapter_event,
                );
            };
            executor.run_prompt(agent_request, Some(&mut event_sink))
        })
        .await
        {
            Ok(run) => run,
            Err(error) => Err(anyhow::anyhow!("agent worker join failed: {error}")),
        }
    };
    let streamed_event_count = streamed_event_count.load(Ordering::SeqCst);

    match run {
        Ok(result) => {
            if session_was_canceled(&state, &session.id) {
                let mut ignored = baize_core::BaizeEvent::new(
                    "session.agent.ignored",
                    serde_json::json!({
                        "reason": "session canceled before provider result completed",
                        "provider_id": provider_id.0,
                    }),
                );
                ignored.workspace_id = Some(session.workspace_id.clone());
                ignored.session_id = Some(session.id.clone());
                ignored.provider_id = Some(provider_id.clone());
                state.record_event(ignored);
                turn.dispatched = false;
                return (
                    StatusCode::CONFLICT,
                    Json(serde_json::json!({
                        "status": "canceled",
                        "turn_status": "canceled",
                        "session_status": "Canceled",
                        "session_id": session.id.0,
                        "provider_id": provider_id,
                        "error": "session was canceled before provider result completed",
                    })),
                );
            }
            for adapter_event in result.events.iter().skip(streamed_event_count) {
                record_adapter_event(
                    &state,
                    &session.workspace_id,
                    &session.id,
                    &provider_id,
                    adapter_event,
                );
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
            let receipt_event_id = final_event.id.clone();

            session.status = new_status.clone();
            if result.success {
                if let Some(native_session_id) = native_session_id.as_deref() {
                    session
                        .provider_native_session_ids
                        .insert(provider_id.0.clone(), native_session_id.to_string());
                }
            }
            let result_uncertain = !result.success
                && (result.exit_code.is_none()
                    || result.error.as_ref().is_some_and(|error| {
                        error.kind == baize_adapters::AgentErrorKind::Timeout
                    }));
            if result.success {
                if let Some(native_id) = native_session_id
                    .as_ref()
                    .or(native_resume_session_id.as_ref())
                {
                    session.continuity.checkpoints.insert(
                        provider_id.0.clone(),
                        baize_core::ProviderCheckpoint {
                            native_session_id: native_id.clone(),
                            through_event_id: receipt_event_id,
                            pending_event_ids: prepared_context.result.omitted_event_ids.clone(),
                            requirement_revision: session.continuity.revision,
                            execution_identity,
                        },
                    );
                }
            }
            if result_uncertain {
                if let Some(attempt) = session.continuity.active_turn.as_mut() {
                    attempt.uncertain = true;
                }
            } else {
                session.continuity.active_turn = None;
            }
            session.updated_at = Utc::now();
            final_event.payload["active_turn"] = serde_json::json!(session.continuity.active_turn);
            match crate::continuity::save_turn(&state, &session, &final_event, &turn.id) {
                Ok(true) => {
                    let _ = state.events.send(final_event);
                }
                Ok(false) => {
                    turn.dispatched = false;
                    return (
                        StatusCode::CONFLICT,
                        Json(
                            serde_json::json!({"status":"canceled","error":"Session changed before result commit; no context receipt advanced"}),
                        ),
                    );
                }
                Err(error) => return internal_error(error.to_string()),
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
                    "continuity": session.continuity,
                    "exit_code": result.exit_code,
                    "native_session_id": native_session_id,
                    "stderr": stderr,
                    "provider_error": provider_error,
                    "limit_inference": limit_inference,
                    "context": context_summary,
                    "recovery_hint": if !result.success && native_resume_session_id.is_some() { Some(format!("No automatic retry. If the native reference is invalid, inspect workspace and /reset-provider {} before resubmitting.",provider_id.0)) } else {None},
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
                    "active_turn": {"id":turn.id,"provider_id":provider_id,"uncertain":true},
                }),
            );
            failed.workspace_id = Some(session.workspace_id.clone());
            failed.session_id = Some(session.id.clone());
            failed.provider_id = Some(provider_id.clone());
            if let Some(attempt) = session.continuity.active_turn.as_mut() {
                attempt.uncertain = true;
            }
            session.status = TaskSessionStatus::Failed;
            session.updated_at = Utc::now();
            match crate::continuity::save_turn(&state, &session, &failed, &turn.id) {
                Ok(true) => {
                    let _ = state.events.send(failed);
                }
                Ok(false) => {
                    turn.dispatched = false;
                    return (
                        StatusCode::CONFLICT,
                        Json(
                            serde_json::json!({"status":"canceled","error":"Session changed; uncertain result retained without a receipt"}),
                        ),
                    );
                }
                Err(error) => return internal_error(error.to_string()),
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
                    "continuity": session.continuity,
                    "limit_inference": limit_inference,
                    "context": context_summary,
                })),
            )
        }
    }
}

fn reject_prompt(error: &str) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({"error":error,"not_submitted":true})),
    )
}
