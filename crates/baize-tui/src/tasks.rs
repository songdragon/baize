use super::*;

pub(super) fn poll_prompt_task(state: &mut TuiState, prompt_task: &mut Option<PromptTask>) {
    let Some(task) = prompt_task.as_ref() else {
        return;
    };
    loop {
        match task.receiver.try_recv() {
            Ok(PromptTaskMessage::Started(started)) => apply_prompt_started(state, started),
            Ok(PromptTaskMessage::StreamEvent(event)) => apply_prompt_stream_event(state, &event),
            Ok(PromptTaskMessage::StreamUnavailable(error)) => {
                state.ui.notice = Some(format!(
                "Live stream unavailable: {error}. Waiting for the recorded result; do not resend."
            ))
            }
            Ok(PromptTaskMessage::Completed(completion)) => {
                apply_prompt_completion(state, completion);
                *prompt_task = None;
                break;
            }
            Ok(PromptTaskMessage::Failed(error)) => {
                flush_pending_assistant_stream(state);
                state.ui.execution_uncertain = state.session_id.is_some();
                state.activity_status = "failed".to_string();
                state.streaming_assistant_open = false;
                state.ui.notice=Some("Execution result uncertain. Inspect changes and history before sending again; recover the prompt via Ctrl-K.".into());
                state.push_message(format!("error: {error}"));
                *prompt_task = None;
                break;
            }
            Err(TryRecvError::Empty) => break,
            Err(TryRecvError::Disconnected) => {
                flush_pending_assistant_stream(state);
                state.ui.execution_uncertain = state.session_id.is_some();
                state.activity_status = "failed".to_string();
                state.streaming_assistant_open = false;
                state.push_message("error: prompt worker disconnected".to_string());
                *prompt_task = None;
                break;
            }
        }
    }
}

pub(super) fn poll_ui_task(state: &mut TuiState, ui_task: &mut Option<UiTask>) {
    let Some(task) = ui_task.as_ref() else {
        return;
    };
    match task.receiver.try_recv() {
        Ok(Ok(completion)) => {
            apply_ui_task_completion(state, completion);
            state.background_status = None;
            *ui_task = None;
        }
        Ok(Err(error)) => {
            state.background_status = None;
            state.ui.notice = Some(format!(
                "Request failed: {error}. Refresh with Ctrl-R; do not repeat an uncertain action."
            ));
            state.push_message(format!("background task failed: {error}"));
            *ui_task = None;
        }
        Err(TryRecvError::Empty) => {}
        Err(TryRecvError::Disconnected) => {
            state.background_status = None;
            state.push_message("background task disconnected".to_string());
            *ui_task = None;
        }
    }
}

pub(super) fn start_ui_task_if_idle(
    state: &mut TuiState,
    ui_task: &mut Option<UiTask>,
    label: &'static str,
    build_task: impl FnOnce() -> UiTask,
) {
    if ui_task.is_some() {
        let current = state
            .background_status
            .as_deref()
            .unwrap_or("background task");
        state.push_message(format!("{current} already running"));
        return;
    }

    state.background_status = Some(label.to_string());
    state.push_message(format!("{label}..."));
    *ui_task = Some(build_task());
}

pub(super) fn start_initial_load_task() -> UiTask {
    start_ui_task(|| {
        Ok(UiTaskCompletion::InitialLoad(InitialLoadResult {
            providers: load_provider_ids().map_err(|error| format!("{error:#}")),
            diagnostics: load_provider_diagnostics()
                .or_else(|_| {
                    load_provider_health().map(|health| {
                        health
                            .into_iter()
                            .map(diagnostic_from_health)
                            .collect::<Vec<_>>()
                    })
                })
                .map_err(|error| format!("{error:#}")),
            permissions: load_pending_permissions().map_err(|error| format!("{error:#}")),
            runtime_policy: load_runtime_policy().map_err(|error| format!("{error:#}")),
        }))
    })
}

pub(super) fn start_permission_refresh_task() -> UiTask {
    start_ui_task(|| {
        Ok(UiTaskCompletion::PermissionRefresh(
            load_pending_permissions()?,
        ))
    })
}

pub(super) fn start_selected_session_load_task(state: &mut TuiState) -> Option<UiTask> {
    let Some(session_id) = state
        .recent_sessions
        .get(state.selected_session_index)
        .map(|session| session.id.clone())
    else {
        state.push_message("load sessions with Ctrl-L before selecting one");
        return None;
    };

    Some(start_ui_task(move || {
        Ok(UiTaskCompletion::SelectedSessionLoad(
            load_selected_session_data(&session_id)?,
        ))
    }))
}

pub(super) fn start_ui_task(
    work: impl FnOnce() -> Result<UiTaskCompletion> + Send + 'static,
) -> UiTask {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let result = work().map_err(|error| format!("{error:#}"));
        let _ = sender.send(result);
    });
    UiTask { receiver }
}

pub(super) fn apply_ui_task_completion(state: &mut TuiState, completion: UiTaskCompletion) {
    match completion {
        UiTaskCompletion::InitialLoad(result) => apply_initial_load_result(state, result),
        UiTaskCompletion::WorkspaceLoad {
            workspace_id,
            display,
            sessions,
        } => {
            state.workspace_id = Some(workspace_id);
            state.workspace = display;
            state.set_recent_sessions(sessions);
        }
        UiTaskCompletion::Action(result) => apply_action(state, *result),
        UiTaskCompletion::PermissionRefresh(permissions) => {
            state.set_pending_permissions(permissions);
            state.push_message(format!(
                "permissions refreshed: {} pending",
                state.pending_permissions.len()
            ));
        }
        UiTaskCompletion::SelectedSessionLoad(load) => apply_selected_session_load(state, load),
    }
}

pub(super) fn apply_initial_load_result(state: &mut TuiState, result: InitialLoadResult) {
    if result.providers.is_ok() {
        state.daemon_status = daemon_connected_message();
    }
    match result.providers {
        Ok(providers) if !providers.is_empty() => {
            state.providers = providers;
            state.selected_provider_index = 0;
        }
        Ok(_) => state.push_message("daemon returned no enabled providers; using defaults"),
        Err(error) => state.push_message(format!("provider load failed: {error}")),
    }
    match result.diagnostics {
        Ok(diagnostics) => {
            if let Some((from, to)) = state.set_provider_diagnostics(diagnostics) {
                state.push_message(format!("provider auto-selected: {from} -> {to}"));
            }
            if state.detail_mode {
                append_provider_diagnostic_issues(state);
            }
        }
        Err(error) => state.push_message(format!("provider diagnostics failed: {error}")),
    }
    match result.permissions {
        Ok(permissions) => state.set_pending_permissions(permissions),
        Err(error) => state.push_message(format!("permission load failed: {error}")),
    }
    match result.runtime_policy {
        Ok(policy) => state.runtime_policy = Some(policy),
        Err(error) => {
            state.runtime_policy = Some(default_runtime_policy());
            state.push_message(format!(
                "runtime policy unavailable; using safe default ask mode ({})",
                runtime_policy_error_hint(&anyhow!(error.clone()))
            ));
        }
    }
    if state.detail_mode {
        state.push_message("status load complete");
    }
}

pub(super) fn detach_prompt_task_after_cancel(
    state: &mut TuiState,
    prompt_task: &mut Option<PromptTask>,
) {
    if prompt_task.take().is_some() {
        state.push_message("prompt worker detached after cancel");
    }
}

pub(super) fn start_prompt_task(submission: PendingPromptSubmission) -> Result<PromptTask> {
    let (sender, receiver) = mpsc::channel();
    let stop_stream = Arc::new(AtomicBool::new(false));
    let worker_stop_stream = stop_stream.clone();
    thread::spawn(move || {
        if let Err(error) = run_prompt_submission(&submission, sender.clone(), worker_stop_stream) {
            let _ = sender.send(PromptTaskMessage::Failed(format!("{error:#}")));
        }
    });
    Ok(PromptTask {
        receiver,
        stop_stream,
    })
}

pub(super) fn run_prompt_submission(
    submission: &PendingPromptSubmission,
    sender: Sender<PromptTaskMessage>,
    stop_stream: Arc<AtomicBool>,
) -> Result<()> {
    let (workspace_id, workspace_display) = prepare_workspace_for_submission(submission)?;
    let session = prepare_session_for_submission(submission, &workspace_id)?;
    let started = PromptTaskStarted {
        workspace_id: Some(workspace_id.clone()),
        workspace_display: workspace_display.clone(),
        session_id: session.session_id.clone(),
        active_provider: Some(submission.provider_id.clone()),
        route_reason: session.route_reason.clone(),
        created_session: session.created,
    };
    let _ = sender.send(PromptTaskMessage::Started(started));

    start_prompt_sse_reader(
        sender.clone(),
        session.session_id.clone(),
        submission.provider_id.clone(),
        stop_stream.clone(),
    );
    let prompt_response = match post_json_with_timeout(
        &format!("/sessions/{}/prompt", session.session_id),
        prompt_request_body(
            &submission.provider_id,
            &submission.prompt,
            &submission.image_paths,
        ),
        Duration::from_secs(PROMPT_TIMEOUT_SECONDS + PROMPT_RESPONSE_GRACE_SECONDS),
    ) {
        Ok(response) => response,
        Err(error) => {
            stop_stream.store(true, Ordering::SeqCst);
            return Err(error);
        }
    };
    stop_stream.store(true, Ordering::SeqCst);
    let events_response = load_session_events(&session.session_id)?;
    let routes_response = get_json(&format!("/sessions/{}/routes", session.session_id))?;
    let diff_response = get_json(&format!("/sessions/{}/diff", session.session_id))?;

    let completion = PromptTaskCompletion {
        workspace_id: Some(workspace_id),
        workspace_display,
        session_id: session.session_id,
        active_provider: Some(submission.provider_id.clone()),
        route_reason: session.route_reason,
        prompt_response,
        events_response,
        routes_response,
        diff_response,
    };
    let _ = sender.send(PromptTaskMessage::Completed(completion));
    Ok(())
}

#[cfg(test)]
pub(super) fn session_events_path(session_id: &str, provider_id: &str) -> String {
    format!("/sessions/{session_id}/events?provider_id={provider_id}")
}

pub(super) fn apply_prompt_completion(state: &mut TuiState, completion: PromptTaskCompletion) {
    apply_prompt_completion_context(state, &completion);
    flush_pending_assistant_stream(state);
    append_prompt_response(state, &completion.prompt_response);
    let events_response = unseen_events_response(state, &completion.events_response);
    if state.detail_mode {
        append_recent_events_with_sections(state, &events_response);
        append_route_history(state, &completion.routes_response);
    } else {
        append_assistant_events(state, &events_response);
        append_test_result_section(state, &events_response);
    }
    state.ui.execution_uncertain = false;
    state.ui.session_status = completion.prompt_response["session_status"]
        .as_str()
        .unwrap_or("Running")
        .to_string();
    state.ui.context = json!({"context":completion.prompt_response["context"]});
    state.ui.diff = completion.diff_response["diff"].clone();
    if let Some(session) = state
        .recent_sessions
        .iter_mut()
        .find(|s| s.id == completion.session_id)
    {
        session.active_provider_id = state.active_provider.clone();
        session.status = state.ui.session_status.clone();
    }
    append_session_diff(state, &completion.diff_response);
    state.streaming_assistant_open = false;
    state.activity_status = prompt_activity_after_response(&completion.prompt_response).to_string();
}

pub(super) fn apply_prompt_started(state: &mut TuiState, started: PromptTaskStarted) {
    if let Some(workspace_id) = &started.workspace_id {
        state.workspace_id = Some(workspace_id.clone());
    }
    if let Some(workspace_display) = &started.workspace_display {
        state.workspace = workspace_display.clone();
    }
    state.session_id = Some(started.session_id.clone());
    if let Some(last) = state.ui.last_submission.as_mut() {
        last.session_id = Some(started.session_id.clone());
    }
    if started.created_session {
        state.recent_sessions.push(SessionView {
            id: started.session_id.clone(),
            workspace_id: state.workspace_id.clone().unwrap_or_default(),
            objective: state
                .ui
                .last_submission
                .as_ref()
                .map(|s| s.prompt.clone())
                .unwrap_or_else(|| "Conversation".into()),
            active_provider_id: started.active_provider.clone(),
            status: "Running".into(),
        });
        state.selected_session_index = state.recent_sessions.len().saturating_sub(1);
    }
    if let Some(provider) = &started.active_provider {
        state.push_message(format!("{provider} · CLI"));
    }
    if let Some(provider) = &started.active_provider {
        state.active_provider = Some(provider.clone());
    }
    if let Some(reason) = &started.route_reason {
        state.route_reason = Some(reason.clone());
    }
    // Keep IDs across turns so the final history fetch cannot replay earlier replies.
    if started.created_session {
        state.seen_event_ids.clear();
    }
    state.streaming_assistant_open = false;
    clear_pending_assistant_stream(state);
    if started.created_session {
        let provider = started.active_provider.as_deref().unwrap_or("unknown");
        if state.detail_mode {
            state.push_message(format!("session: {} ({provider})", started.session_id));
            if let Some(reason) = &started.route_reason {
                state.push_message(format!("route: {reason}"));
            }
        }
    }
}

pub(super) fn apply_prompt_completion_context(
    state: &mut TuiState,
    completion: &PromptTaskCompletion,
) {
    if let Some(workspace_id) = &completion.workspace_id {
        state.workspace_id = Some(workspace_id.clone());
    }
    if let Some(workspace_display) = &completion.workspace_display {
        state.workspace = workspace_display.clone();
    }
    state.session_id = Some(completion.session_id.clone());
    if let Some(provider) = &completion.active_provider {
        state.active_provider = Some(provider.clone());
    }
    if let Some(reason) = &completion.route_reason {
        state.route_reason = Some(reason.clone());
    }
}

pub(super) fn apply_prompt_stream_event(state: &mut TuiState, event: &Value) {
    if !mark_event_seen(state, event) {
        return;
    }
    let event_type = event
        .get("event_type")
        .and_then(Value::as_str)
        .unwrap_or("event");
    let payload = event.get("payload").unwrap_or(&Value::Null);
    match event_type {
        "session.agent.output" if !append_stream_assistant_event(state, payload) => {
            append_stream_status_event(state, payload);
        }
        "session.agent.output" => {}
        "session.agent.tool_call" => append_stream_tool_event(state, payload),
        "session.agent.failed" => {
            state.activity_status = "failed".to_string();
            push_transcript_section(state, "status:");
            state.push_message(format!("  {}", status_event_line(event_type, payload)));
        }
        _ => {}
    }
}

pub(super) fn append_stream_assistant_event(state: &mut TuiState, payload: &Value) -> bool {
    let lines = assistant_event_lines(payload);
    if lines.is_empty() {
        return false;
    }
    if !state.streaming_assistant_open {
        push_transcript_section(state, "assistant:");
        state.streaming_assistant_open = true;
    }
    if should_progressively_render_assistant(state, &lines) {
        state.pending_assistant_lines.extend(lines);
        advance_pending_assistant_stream(state);
    } else {
        for line in lines {
            state.push_message(format!("  {line}"));
        }
    }
    true
}

pub(super) fn append_stream_tool_event(state: &mut TuiState, payload: &Value) {
    let detail = event_detail(payload)
        .or_else(|| raw_stream_event_detail(payload))
        .unwrap_or_else(|| "tool call".to_string());
    if state.detail_mode {
        push_transcript_section(state, "command/tool events:");
        state.push_message(format!("  {detail}"));
    } else {
        push_transcript_section(state, "status:");
        state.push_message(format!("  stream: {detail}"));
    }
}

pub(super) fn append_stream_status_event(state: &mut TuiState, payload: &Value) {
    let Some(detail) = raw_stream_event_detail(payload) else {
        return;
    };
    push_transcript_section(state, "status:");
    state.push_message(format!("  stream: {detail}"));
}

pub(super) fn raw_stream_event_detail(payload: &Value) -> Option<String> {
    let raw = payload.get("raw")?;
    raw.get("part")
        .and_then(|part| part.get("type"))
        .and_then(Value::as_str)
        .or_else(|| raw.get("type").and_then(Value::as_str))
        .or_else(|| raw.get("status").and_then(Value::as_str))
        .or_else(|| raw.get("tool").and_then(Value::as_str))
        .map(transcript_detail)
}

pub(super) fn unseen_events_response(state: &mut TuiState, response: &Value) -> Value {
    let Some(events) = response.get("events").and_then(Value::as_array) else {
        return response.clone();
    };
    let unseen = events
        .iter()
        .filter(|event| mark_event_seen(state, event))
        .cloned()
        .collect::<Vec<_>>();
    json!({ "events": unseen })
}

pub(super) fn mark_event_seen(state: &mut TuiState, event: &Value) -> bool {
    let Some(event_id) = event.get("id").and_then(Value::as_str) else {
        return true;
    };
    state.seen_event_ids.insert(event_id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn later_turn_keeps_event_ids_and_provider_source_in_same_conversation() {
        let mut state = TuiState {
            session_id: Some("task_1".into()),
            ..TuiState::default()
        };
        state.seen_event_ids.insert("event_old".into());
        apply_prompt_started(
            &mut state,
            PromptTaskStarted {
                workspace_id: Some("ws_1".into()),
                workspace_display: None,
                session_id: "task_1".into(),
                active_provider: Some("opencode".into()),
                route_reason: None,
                created_session: false,
            },
        );
        apply_prompt_stream_event(
            &mut state,
            &json!({"id":"event_old","event_type":"session.agent.output","payload":{"text":"duplicated answer"}}),
        );
        assert!(!state.transcript_text().contains("duplicated answer"));
        assert!(state.transcript_text().contains("opencode · CLI"));
        assert_eq!(state.session_id.as_deref(), Some("task_1"));
        assert!(state.seen_event_ids.contains("event_old"));
    }
}
