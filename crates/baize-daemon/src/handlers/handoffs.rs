use axum::extract::{Path as AxumPath, State};
use axum::http::StatusCode;
use axum::Json;
use baize_core::{HandoffFacts, HandoffId, HandoffStatus, HandoffSummary, RoutingMode};
use chrono::Utc;

use crate::helpers::{
    bad_request, infer_task_type, internal_error, json_result_option, ok_json, with_store,
};
use crate::state::AppState;

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
    let handoff_id = HandoffId::new();
    let checkpoint_refs = checkpoint_refs_for_handoff(
        &state.config.workspace.checkpoint_policy,
        &session.id,
        &handoff_id,
    );
    let handoff = HandoffSummary {
        id: handoff_id,
        session_id: session.id.clone(),
        from_provider_id: from_provider_id.clone(),
        to_provider_id: to_provider_id.clone(),
        summary_markdown,
        mechanical_facts: HandoffFacts {
            changed_files,
            checkpoint_refs,
            user_constraints,
            ..HandoffFacts::default()
        },
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
