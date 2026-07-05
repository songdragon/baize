use axum::extract::{Path as AxumPath, State};
use axum::http::StatusCode;
use axum::Json;
use baize_core::{HandoffFacts, HandoffId, HandoffStatus, HandoffSummary, RoutingMode};
use chrono::Utc;

use crate::helpers::{
    bad_request, infer_task_type, internal_error, json_result_option, ok_json, with_store,
};
use crate::state::AppState;

const HANDOFF_FACT_LIMIT: usize = 8;
const HANDOFF_LINE_LIMIT: usize = 180;

pub async fn create_handoff(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<crate::state::HandoffRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    let session_id = baize_core::TaskSessionId(id);
    let session = match with_store(&state, |store| store.get_task_session(&session_id)) {
        Ok(Some(session)) => session,
        Ok(None) => return crate::helpers::not_found("session not found"),
        Err(error) => return internal_error(error.to_string()),
    };
    let from_provider_id = session
        .active_provider_id
        .clone()
        .unwrap_or_else(|| baize_core::ProviderId("unknown".to_string()));
    let to_provider_id = baize_core::ProviderId(request.to_provider_id);
    let workspace = match with_store(&state, |store| store.get_workspace(&session.workspace_id)) {
        Ok(Some(workspace)) => workspace,
        _ => return crate::helpers::not_found("workspace not found"),
    };
    let project = match with_store(&state, |store| store.get_primary_project(&workspace)) {
        Ok(Some(project)) => project,
        _ => return crate::helpers::not_found("project not found"),
    };
    let status = baize_workspace::inspect(&project.root).ok();
    let changed_files = status
        .as_ref()
        .map(|status| status.changed_files.clone())
        .unwrap_or_default();
    let user_constraints = request.user_constraints.unwrap_or_default();
    let handoff_id = HandoffId::new();
    let checkpoint_refs = checkpoint_refs_for_handoff(
        &state.config.workspace.checkpoint_policy,
        &session.id,
        &handoff_id,
    );
    let facts = match build_handoff_facts(
        &state,
        &session,
        changed_files,
        checkpoint_refs,
        user_constraints,
    ) {
        Ok(facts) => facts,
        Err(error) => return internal_error(error.to_string()),
    };
    let summary_markdown =
        render_handoff_summary(&session, &from_provider_id, &to_provider_id, &facts);
    let handoff = HandoffSummary {
        id: handoff_id,
        session_id: session.id.clone(),
        from_provider_id: from_provider_id.clone(),
        to_provider_id: to_provider_id.clone(),
        summary_markdown,
        mechanical_facts: facts,
        status: HandoffStatus::Draft,
        created_at: Utc::now(),
    };
    let artifact_path = match with_store(&state, |store| {
        store.insert_handoff(&handoff)?;
        store.write_handoff_artifact(&handoff)
    }) {
        Ok(path) => path.display().to_string(),
        Err(error) => return internal_error(error.to_string()),
    };
    let mut event = baize_core::BaizeEvent::new(
        "handoff.created",
        serde_json::json!({
            "handoff": handoff,
            "artifact_path": artifact_path,
        }),
    );
    event.workspace_id = Some(session.workspace_id);
    event.session_id = Some(session.id);
    event.provider_id = Some(to_provider_id);
    state.record_event(event);
    ok_json(serde_json::json!({
        "handoff": handoff,
        "artifact_path": artifact_path,
    }))
}

fn checkpoint_refs_for_handoff(
    checkpoint_policy: &str,
    session_id: &baize_core::TaskSessionId,
    handoff_id: &HandoffId,
) -> Vec<String> {
    if checkpoint_policy == "before_handoff" {
        vec![format!("before_handoff:{}:{}", session_id.0, handoff_id.0)]
    } else {
        Vec::new()
    }
}

fn build_handoff_facts(
    state: &AppState,
    session: &baize_core::TaskSession,
    changed_files: Vec<String>,
    checkpoint_refs: Vec<String>,
    user_constraints: Vec<String>,
) -> anyhow::Result<HandoffFacts> {
    let (events, routes) = with_store(state, |store| {
        Ok((
            store.list_events_for_session(&session.id, Some(200), None)?,
            store.list_route_decisions_for_session(&session.id)?,
        ))
    })?;

    let commands_run = recent_limited(
        events
            .iter()
            .filter(|event| event.event_type == "session.agent.tool_call")
            .filter_map(|event| event_detail(&event.payload)),
    );
    let test_result = events.iter().rev().find_map(|event| {
        event_detail(&event.payload).and_then(|detail| {
            if looks_like_test_result(&detail) {
                Some(one_line_limit(&detail))
            } else {
                None
            }
        })
    });
    let provider_errors = recent_limited(events.iter().filter_map(provider_error_detail));
    let route_history = recent_limited(routes.iter().map(|route| {
        let previous = route
            .previous_provider_id
            .as_ref()
            .map(|provider| provider.0.as_str())
            .unwrap_or("none");
        one_line_limit(&format!(
            "{previous} -> {}: {}",
            route.selected_provider_id.0, route.reason
        ))
    }));

    Ok(HandoffFacts {
        changed_files: changed_files.into_iter().take(HANDOFF_FACT_LIMIT).collect(),
        commands_run,
        test_result,
        route_history,
        provider_errors,
        checkpoint_refs,
        user_constraints,
    })
}

fn render_handoff_summary(
    session: &baize_core::TaskSession,
    from_provider_id: &baize_core::ProviderId,
    to_provider_id: &baize_core::ProviderId,
    facts: &HandoffFacts,
) -> String {
    let mut summary = String::new();
    summary.push_str("# Handoff\n\n");
    summary.push_str(&format!(
        "Objective: {}\n\n",
        one_line_limit(&session.objective)
    ));
    summary.push_str(&format!(
        "Current status: {}\n\n",
        session_status(&session.status)
    ));
    summary.push_str(&format!(
        "Provider path: {} -> {}\n\n",
        from_provider_id.0, to_provider_id.0
    ));
    push_markdown_list(&mut summary, "Changed files", &facts.changed_files);
    push_markdown_list(&mut summary, "Recent commands", &facts.commands_run);
    if let Some(test_result) = &facts.test_result {
        summary.push_str(&format!(
            "Latest test signal: {}\n\n",
            one_line_limit(test_result)
        ));
    } else {
        summary.push_str("Latest test signal: none recorded\n\n");
    }
    push_markdown_list(&mut summary, "Provider errors", &facts.provider_errors);
    push_markdown_list(&mut summary, "Route history", &facts.route_history);
    push_markdown_list(&mut summary, "User constraints", &facts.user_constraints);
    push_markdown_list(&mut summary, "Checkpoint refs", &facts.checkpoint_refs);
    summary.push_str(&format!(
        "Recommended next step: Continue with {} using the facts above.\n",
        to_provider_id.0
    ));
    summary
}

fn push_markdown_list(summary: &mut String, label: &str, values: &[String]) {
    summary.push_str(label);
    summary.push_str(":\n");
    if values.is_empty() {
        summary.push_str("- none\n\n");
        return;
    }
    for value in values.iter().take(HANDOFF_FACT_LIMIT) {
        summary.push_str("- ");
        summary.push_str(&one_line_limit(value));
        summary.push('\n');
    }
    summary.push('\n');
}

fn recent_limited(values: impl Iterator<Item = String>) -> Vec<String> {
    let values = values.collect::<Vec<_>>();
    values
        .into_iter()
        .rev()
        .take(HANDOFF_FACT_LIMIT)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

fn provider_error_detail(event: &baize_core::BaizeEvent) -> Option<String> {
    if event.event_type != "session.agent.failed" {
        return None;
    }
    event_detail(&event.payload)
        .or_else(|| nested_string(&event.payload, &["provider_error", "message"]))
        .or_else(|| {
            event
                .payload
                .get("limit_inference")
                .map(|limit| one_line_limit(&format!("provider limit inferred: {limit}")))
        })
}

fn event_detail(payload: &serde_json::Value) -> Option<String> {
    payload
        .get("text")
        .and_then(serde_json::Value::as_str)
        .or_else(|| payload.get("error").and_then(serde_json::Value::as_str))
        .or_else(|| payload.get("stderr").and_then(serde_json::Value::as_str))
        .map(one_line_limit)
}

fn nested_string(payload: &serde_json::Value, path: &[&str]) -> Option<String> {
    let mut value = payload;
    for key in path {
        value = value.get(*key)?;
    }
    value.as_str().map(one_line_limit)
}

fn looks_like_test_result(detail: &str) -> bool {
    let lower = detail.to_ascii_lowercase();
    lower.contains("cargo test")
        || lower.contains("pytest")
        || lower.contains("test result")
        || lower.contains("tests passed")
        || lower.contains("test failed")
}

fn one_line_limit(text: &str) -> String {
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= HANDOFF_LINE_LIMIT {
        return one_line;
    }
    let keep = HANDOFF_LINE_LIMIT.saturating_sub(3);
    format!("{}...", one_line.chars().take(keep).collect::<String>())
}

fn session_status(status: &baize_core::TaskSessionStatus) -> &'static str {
    match status {
        baize_core::TaskSessionStatus::Running => "Running",
        baize_core::TaskSessionStatus::WaitingForPermission => "WaitingForPermission",
        baize_core::TaskSessionStatus::Completed => "Completed",
        baize_core::TaskSessionStatus::Failed => "Failed",
        baize_core::TaskSessionStatus::Canceled => "Canceled",
    }
}

pub async fn accept_handoff(
    State(state): State<AppState>,
    AxumPath((id, handoff_id)): AxumPath<(String, String)>,
) -> (StatusCode, Json<serde_json::Value>) {
    let session_id = baize_core::TaskSessionId(id);
    let mut handoff = match with_store(&state, |store| store.get_handoff(&HandoffId(handoff_id))) {
        Ok(Some(handoff)) => handoff,
        Ok(None) => return crate::helpers::not_found("handoff not found"),
        Err(error) => return internal_error(error.to_string()),
    };
    if handoff.session_id != session_id {
        return bad_request("handoff does not belong to session");
    }

    let mut session = match with_store(&state, |store| store.get_task_session(&session_id)) {
        Ok(Some(session)) => session,
        Ok(None) => return crate::helpers::not_found("session not found"),
        Err(error) => return internal_error(error.to_string()),
    };

    let previous_provider_id = session.active_provider_id.clone();
    let selected_provider_id = handoff.to_provider_id.clone();
    let now = Utc::now();
    session.active_provider_id = Some(selected_provider_id.clone());
    session.updated_at = now;
    handoff.status = HandoffStatus::Accepted;

    let decision = baize_core::RouteDecision {
        id: baize_core::RouteDecisionId::new(),
        session_id: session.id.clone(),
        selected_provider_id: selected_provider_id.clone(),
        previous_provider_id,
        reason: format!(
            "Accepted handoff {} to {}.",
            handoff.id.0, selected_provider_id.0
        ),
        task_type: Some(infer_task_type(&session.objective)),
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
        return internal_error(error.to_string());
    }

    let mut accepted = baize_core::BaizeEvent::new(
        "handoff.accepted",
        serde_json::json!({
            "handoff": handoff,
            "session": session,
            "route_decision": decision,
        }),
    );
    accepted.workspace_id = Some(session.workspace_id.clone());
    accepted.session_id = Some(session.id.clone());
    accepted.provider_id = Some(selected_provider_id.clone());
    state.record_event(accepted);

    let mut routed = baize_core::BaizeEvent::new(
        "session.route.decided",
        serde_json::json!({ "decision": decision }),
    );
    routed.workspace_id = Some(session.workspace_id.clone());
    routed.session_id = Some(session.id.clone());
    routed.provider_id = Some(selected_provider_id);
    state.record_event(routed);

    ok_json(serde_json::json!({
        "handoff": handoff,
        "session": session,
        "route_decision": decision,
    }))
}

pub async fn handoff(
    State(state): State<AppState>,
    AxumPath((_session_id, handoff_id)): AxumPath<(String, String)>,
) -> (StatusCode, Json<serde_json::Value>) {
    let handoff = with_store(&state, |store| store.get_handoff(&HandoffId(handoff_id)));
    json_result_option("handoff", handoff)
}
