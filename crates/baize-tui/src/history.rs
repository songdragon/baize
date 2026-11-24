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
    // Loading runs in the background while the composer remains editable.
    // Stage the latest draft and the entire restored view before changing IDs.
    let mut restored = state.clone();
    save_draft(&mut restored);
    if let Err(error) = apply_loaded_session(&mut restored, &load.session) {
        state.ui.notice = Some(format!("Cannot load session: {error:#}"));
        return;
    }
    restored.ui.last_submission = None;
    restore_recovery_submission(&mut restored);
    restored.workspace = load.workspace_display;
    restored.ui.session_status = load.session["status"].as_str().unwrap_or("Unknown").into();
    restored.ui.diff = load.diff_response["diff"].clone();
    restored.ui.context = Value::Null;
    restored.ui.handoff = Value::Null;
    if let Err(error) = replay_session(&mut restored, &load.events_response) {
        state.ui.notice = Some(format!("Cannot restore conversation: {error:#}"));
        return;
    }
    apply_continuity_status(&mut restored, &load.session);
    append_route_history(&mut restored, &load.routes_response);
    restore_draft(&mut restored);
    restored.ui.focus = Focus::Composer;
    *state = restored;
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
    reset_stream_output(state);
    state.streaming_assistant_open = false;
    state.ui.diff = Value::Null;
    state.ui.context = Value::Null;
    state.ui.handoff = Value::Null;
    state.ui.session_status = "New".into();
    state.ui.uncertain_turn_id = None;
    state.ui.execution_uncertain = false;
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
    finish_output_turn(state, "canceled", None);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn load(id: &str, events: Value) -> SelectedSessionLoad {
        SelectedSessionLoad {
            workspace_display: "/workspace".into(),
            session: json!({"id":id,"workspace_id":"ws_1","status":"Running","active_provider_id":"codex"}),
            events_response: events,
            routes_response: json!({"routes":[]}),
            diff_response: json!({"diff":{}}),
        }
    }

    #[test]
    fn invalid_history_never_commits_a_partial_session_or_discards_its_draft() {
        let mut state = TuiState {
            session_id: Some("old".into()),
            workspace_id: Some("ws_old".into()),
            input: "draft 中文".into(),
            image_paths: vec![PathBuf::from("image.png")],
            ..TuiState::default()
        };
        state.ui.cursor = 3;
        state.ui.execution_uncertain = true;
        let transcript = state.transcript.clone();
        let draft_map = state.ui.drafts.clone();
        apply_selected_session_load(&mut state, load("new", json!({"error":"unavailable"})));
        assert_eq!(state.session_id.as_deref(), Some("old"));
        assert_eq!(state.workspace_id.as_deref(), Some("ws_old"));
        assert_eq!(state.input, "draft 中文");
        assert_eq!(state.image_paths, [PathBuf::from("image.png")]);
        assert_eq!(state.ui.cursor, 3);
        assert!(state.ui.execution_uncertain);
        assert_eq!(state.transcript, transcript);
        assert_eq!(state.ui.drafts, draft_map);
        assert!(state
            .ui
            .notice
            .as_deref()
            .unwrap()
            .contains("Cannot restore"));
        let mut invalid = load("new", json!({"events":[]}));
        invalid.session = json!({"id":"new"});
        apply_selected_session_load(&mut state, invalid);
        assert_eq!(state.session_id.as_deref(), Some("old"));
    }

    #[test]
    fn switching_saves_edits_made_after_request_started_and_same_session_reload_keeps_input() {
        let mut state = TuiState {
            session_id: Some("old".into()),
            input: "draft".into(),
            ..TuiState::default()
        };
        save_draft(&mut state);
        state.input.push_str(" edited while loading");
        state.image_paths.push(PathBuf::from("new.png"));
        state.ui.cursor = 5;
        apply_selected_session_load(&mut state, load("new", json!({"events":[]})));
        assert_eq!(state.session_id.as_deref(), Some("new"));
        assert!(state.input.is_empty());
        apply_selected_session_load(&mut state, load("old", json!({"events":[]})));
        assert_eq!(state.input, "draft edited while loading");
        assert_eq!(state.image_paths, [PathBuf::from("new.png")]);
        assert_eq!(state.ui.cursor, 5);
        state.input.push('!');
        apply_selected_session_load(&mut state, load("old", json!({"events":[]})));
        assert_eq!(state.input, "draft edited while loading!");
    }
}
