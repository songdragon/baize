use crate::{
    helpers::{infer_task_type, internal_error, not_found, ok_json, with_store},
    state::{AppState, ContextPreviewQuery, PromptRequest},
};
use axum::{
    extract::{Path as AxumPath, State},
    http::StatusCode,
    Json,
};
use baize_core::{
    ProviderId, RouteDecision, RouteDecisionId, RoutingMode, TaskSession, TaskSessionId,
};
use chrono::Utc;

pub(super) struct PreparedContextPack {
    pub(super) project: baize_core::Project,
    pub(super) result: baize_core::ContextPackBuildResult,
    pub(super) summary: crate::context::ContextPackSummary,
}

pub async fn context_preview_post(
    state: State<AppState>,
    id: AxumPath<String>,
    Json(request): Json<PromptRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    context_preview(
        state,
        id,
        axum::extract::Query(ContextPreviewQuery {
            provider_id: request.provider_id,
            prompt: Some(request.prompt),
        }),
    )
    .await
}

pub async fn context_preview(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    axum::extract::Query(query): axum::extract::Query<ContextPreviewQuery>,
) -> (StatusCode, Json<serde_json::Value>) {
    let session_id = TaskSessionId(id);
    let session = match with_store(&state, |store| store.get_task_session(&session_id)) {
        Ok(Some(session)) => session,
        Ok(None) => return not_found("session not found"),
        Err(error) => return internal_error(error.to_string()),
    };
    let provider_id = query
        .provider_id
        .as_ref()
        .map(|provider| ProviderId(provider.clone()))
        .or_else(|| session.active_provider_id.clone())
        .unwrap_or_else(|| ProviderId("codex".to_string()));
    let prompt = query
        .prompt
        .as_deref()
        .unwrap_or("<context preview request>");
    let prepared = match prepare_context_pack(&state, &session, &provider_id, prompt) {
        Ok(prepared) => prepared,
        Err(error) => return internal_error(error.to_string()),
    };

    ok_json(serde_json::json!({
        "context": prepared.summary,
        "prompt_preview": crate::context::prompt_preview(&prepared.result.prompt),
        "continuity": session.continuity,
        "preview_only": true,
    }))
}

pub(super) fn record_adapter_event(
    state: &AppState,
    workspace_id: &baize_core::WorkspaceId,
    session_id: &TaskSessionId,
    provider_id: &ProviderId,
    adapter_event: &baize_adapters::AgentExecutionEvent,
) {
    let event_type = match adapter_event.kind {
        baize_adapters::AgentExecutionEventKind::Output => "session.agent.output",
        baize_adapters::AgentExecutionEventKind::Diagnostic => "session.agent.diagnostic",
        baize_adapters::AgentExecutionEventKind::Raw => "session.agent.status",
        baize_adapters::AgentExecutionEventKind::ToolCall => "session.agent.tool_call",
    };
    let mut event = baize_core::BaizeEvent::new(
        event_type,
        serde_json::json!({
            "text": adapter_event.text,
            "raw": adapter_event.raw,
            "stream": adapter_event.stream,
        }),
    );
    event.workspace_id = Some(workspace_id.clone());
    event.session_id = Some(session_id.clone());
    event.provider_id = Some(provider_id.clone());
    state.record_event(event);
}

pub(super) fn prepare_context_pack(
    state: &AppState,
    session: &TaskSession,
    provider_id: &ProviderId,
    current_prompt: &str,
) -> anyhow::Result<PreparedContextPack> {
    let (workspace, project, events, route_decisions, handoffs) = with_store(state, |store| {
        let workspace = store
            .get_workspace(&session.workspace_id)?
            .ok_or_else(|| anyhow::anyhow!("workspace not found"))?;
        let project = store
            .get_primary_project(&workspace)?
            .ok_or_else(|| anyhow::anyhow!("project not found"))?;
        let events = store.list_events_for_session(&session.id, Some(i64::MAX as u64), None)?;
        let route_decisions = store.list_route_decisions_for_session(&session.id)?;
        let handoffs = store.list_handoffs_for_session(&session.id)?;
        Ok((workspace, project, events, route_decisions, handoffs))
    })?;
    let _workspace = workspace;
    let workspace_status = baize_workspace::inspect(&project.root).ok();
    let diff_hunks = baize_workspace::diff_hunks(&project.root).unwrap_or_default();
    let profile = crate::context::provider_context_profile(provider_id, &state.config);
    let result = crate::context::build_context_pack(crate::context::ContextPackInput {
        session,
        target_provider: provider_id,
        current_prompt,
        profile: &profile,
        workspace_status: workspace_status.as_ref(),
        project: &project,
        events: &events,
        route_decisions: &route_decisions,
        handoffs: &handoffs,
        diff_hunks: &diff_hunks,
    });
    let summary = crate::context::summarize_context_pack(provider_id, &result);
    Ok(PreparedContextPack {
        project,
        result,
        summary,
    })
}

pub(super) fn validate_prompt_image_paths(
    image_paths: &[std::path::PathBuf],
) -> Result<(), String> {
    for image_path in image_paths {
        if image_path.as_os_str().is_empty() {
            return Err("image attachment path cannot be empty".to_string());
        }
        let metadata = image_path.metadata().map_err(|error| {
            format!(
                "image attachment {} is not readable: {error}",
                image_path.display()
            )
        })?;
        if !metadata.is_file() {
            return Err(format!(
                "image attachment {} is not a file",
                image_path.display()
            ));
        }
    }
    Ok(())
}

pub(super) fn apply_prompt_provider_target(
    state: &AppState,
    session: &mut TaskSession,
    request: &PromptRequest,
) -> Result<ProviderId, (StatusCode, Json<serde_json::Value>)> {
    let current_provider = session
        .active_provider_id
        .clone()
        .unwrap_or_else(|| ProviderId("codex".to_string()));
    let requested_provider = request
        .provider_id
        .as_ref()
        .map(|id| ProviderId(id.clone()))
        .unwrap_or(current_provider);
    if session.active_provider_id.as_ref() == Some(&requested_provider) {
        return Ok(requested_provider);
    }
    if !baize_adapters::is_prompt_runtime_supported(&requested_provider) {
        return Err(crate::helpers::bad_request(&format!(
            "provider {} does not support Baize prompt execution yet",
            requested_provider.0
        )));
    }
    if !request.image_paths.is_empty()
        && !baize_adapters::supports_file_attachment(&requested_provider)
    {
        return Err(crate::helpers::bad_request(&format!(
            "provider {} does not support image attachments",
            requested_provider.0
        )));
    }

    let now = Utc::now();
    let task_type = infer_task_type(&request.prompt);
    let decision = RouteDecision {
        id: RouteDecisionId::new(),
        session_id: session.id.clone(),
        selected_provider_id: requested_provider.clone(),
        previous_provider_id: session.active_provider_id.clone(),
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
        let latest = store
            .get_task_session(&session.id)?
            .ok_or_else(|| anyhow::anyhow!("session not found"))?;
        if matches!(latest.status, baize_core::TaskSessionStatus::Canceled)
            || latest.continuity.active_turn.as_ref().map(|turn| &turn.id)
                != session.continuity.active_turn.as_ref().map(|turn| &turn.id)
        {
            anyhow::bail!("Session changed before route update");
        }
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
