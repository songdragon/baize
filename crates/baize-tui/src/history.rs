use super::*;

pub(super) fn load_selected_session_data(session_id: &str) -> Result<SelectedSessionLoad> {
    let response = get_json(&format!("/sessions/{session_id}"))?;
    let session = response
        .get("session")
        .cloned()
        .ok_or_else(|| response_error("session", &response))?;
    Ok(SelectedSessionLoad {
        workspace_display: workspace_display_for(&session)?,
        events_response: load_session_events(session_id)?,
        session,
        routes_response: get_json(&format!("/sessions/{session_id}/routes"))?,
        diff_response: get_json(&format!("/sessions/{session_id}/diff"))?,
    })
}

pub(super) fn apply_selected_session_load(state: &mut TuiState, load: SelectedSessionLoad) {
    if let Err(error) = apply_loaded_session(state, &load.session) {
        state.ui.notice = Some(format!("Cannot load session: {error:#}"));
        return;
    }
    state.ui.last_submission = None;
    state.workspace = load.workspace_display;
    state.ui.session_status = load.session["status"].as_str().unwrap_or("Unknown").into();
    state.ui.diff = load.diff_response["diff"].clone();
    state.ui.context = Value::Null;
    state.ui.handoff = Value::Null;
    if let Err(error) = replay_session(state, &load.events_response) {
        state.ui.notice = Some(format!("Cannot restore conversation: {error:#}"));
    }
    append_route_history(state, &load.routes_response);
    restore_draft(state);
    state.ui.focus = Focus::Composer;
}

pub(super) fn workspace_display_for(session: &Value) -> Result<String> {
    let workspace = session["workspace_id"]
        .as_str()
        .context("missing workspace ID")?;
    let projects = get_json(&format!("/workspaces/{workspace}/projects"))?;
    let root = projects["projects"]
        .as_array()
        .and_then(|items| items.first())
        .and_then(|p| p["root"].as_str())
        .context("missing workspace project root")?;
    Ok(root.to_string())
}

pub(super) fn start_new_session(state: &mut TuiState) {
    state.transcript.clear();
    state.seen_event_ids.clear();
    clear_pending_assistant_stream(state);
    state.streaming_assistant_open = false;
    state.ui.diff = Value::Null;
    state.ui.context = Value::Null;
    state.ui.handoff = Value::Null;
    state.ui.session_status = "New".into();
    state.ui.last_submission = None;
    state.session_id = None;
    state.active_provider = None;
    state.route_reason = None;
    state.pending_handoff_id = None;
    state.pending_handoff_session_id = None;
    state.input.clear();
    state.image_paths.clear();
    state.activity_status = "idle".to_string();
    state.push_message("new session: next prompt will create a fresh task");
}

pub(super) fn append_cancel_response(state: &mut TuiState, response: &Value) {
    let session = response.get("session").unwrap_or(&Value::Null);
    let id = session
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let status = session
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("unknown");

    state.session_id = session
        .get("id")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    state.active_provider = session
        .get("active_provider_id")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    state.route_reason = None;
    state.pending_handoff_id = None;
    state.pending_handoff_session_id = None;
    state.push_message(format!("session {id}: {status}"));
}

pub(super) fn append_complete_response(state: &mut TuiState, response: &Value) {
    let session = response.get("session").unwrap_or(&Value::Null);
    let id = session
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let status = session
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("unknown");

    state.session_id = session
        .get("id")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    state.active_provider = session
        .get("active_provider_id")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    state.route_reason = None;
    state.pending_handoff_id = None;
    state.pending_handoff_session_id = None;
    state.push_message(format!("session {id}: {status}"));
}

pub(super) fn parse_session_views(response: &Value) -> Result<Vec<SessionView>> {
    let sessions = response
        .get("sessions")
        .and_then(Value::as_array)
        .ok_or_else(|| response_error("sessions", response))?;

    Ok(sessions
        .iter()
        .filter_map(|session| {
            let id = session.get("id").and_then(task_session_id)?;
            let workspace_id = session.get("workspace_id").and_then(workspace_id)?;
            let objective = session
                .get("objective")
                .and_then(Value::as_str)
                .unwrap_or("session");
            let status = session
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("Unknown");
            Some(SessionView {
                id: id.to_string(),
                workspace_id: workspace_id.to_string(),
                objective: objective.to_string(),
                active_provider_id: session
                    .get("active_provider_id")
                    .and_then(provider_id)
                    .map(ToOwned::to_owned),
                status: status.to_string(),
            })
        })
        .collect())
}

pub(super) fn apply_loaded_session(state: &mut TuiState, session: &Value) -> Result<()> {
    let id = session
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| response_error("session.id", session))?;
    let workspace_id = session
        .get("workspace_id")
        .and_then(Value::as_str)
        .ok_or_else(|| response_error("session.workspace_id", session))?;
    let objective = session
        .get("objective")
        .and_then(Value::as_str)
        .unwrap_or("session");
    let provider = session
        .get("active_provider_id")
        .and_then(Value::as_str)
        .unwrap_or("unknown");

    state.session_id = Some(id.to_string());
    state.workspace_id = Some(workspace_id.to_string());
    state.active_provider = Some(provider.to_string());
    if let Some(index) = state.providers.iter().position(|p| p == provider) {
        state.selected_provider_index = index;
    }
    state.route_reason = None;
    state.pending_handoff_id = None;
    state.pending_handoff_session_id = None;
    state.workspace = format!("Workspace: loaded\n{workspace_id}");
    state.select_recent_session(id);
    state.push_message(format!("loaded session: {id} ({provider})"));
    state.push_message(format!("objective: {}", one_line(objective)));
    Ok(())
}
