use super::*;

pub(super) fn poll_prompt_task(state: &mut TuiState, prompt_task: &mut Option<PromptTask>) {
    let Some(task) = prompt_task.as_ref() else {
        return;
    };
    loop {
        match task.receiver.try_recv() {
            Ok(PromptTaskMessage::Started(started)) => apply_prompt_started(state, started),
            Ok(PromptTaskMessage::StreamEvent(event)) => apply_prompt_stream_event(state, &event),
            Ok(PromptTaskMessage::StreamRecovered) => {
                if state
                    .ui
                    .notice
                    .as_deref()
                    .is_some_and(|notice| notice.starts_with("Live stream"))
                {
                    state.ui.notice = None;
                }
            }
            Ok(PromptTaskMessage::NotSubmitted(error)) => {
                state.activity_status = "blocked".into();
                state.ui.execution_uncertain = false;
                if state.input.is_empty() {
                    if let Some(last) = &state.ui.last_submission {
                        state.input = last.prompt.clone();
                        state.image_paths = last.image_paths.clone();
                        state.ui.cursor = usize::MAX;
                    }
                }
                state.ui.notice = Some(format!(
                    "Prompt not submitted: {error}. Your draft can be reviewed and sent again."
                ));
                *prompt_task = None;
                break;
            }
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

pub(super) fn apply_prompt_completion(state: &mut TuiState, completion: PromptTaskCompletion) {
    apply_prompt_completion_context(state, &completion);
    let events_response = unseen_events_response(state, &completion.events_response);
    append_recent_events(state, &events_response);
    append_prompt_response(state, &completion.prompt_response);
    if state.detail_mode {
        append_route_history(state, &completion.routes_response);
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
    reset_stream_output(state);
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
    render_agent_event(state, event);
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
    for line in lines {
        state.push_message(format!("  {line}"));
    }
    true
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
    fn failed_subscription_restores_unsent_prompt_without_marking_execution_uncertain() {
        let mut state = TuiState {
            session_id: Some("task_1".into()),
            ..TuiState::default()
        };
        state.ui.last_submission = Some(PendingPromptSubmission {
            prompt: "draft to restore".into(),
            provider_id: "codex".into(),
            image_paths: vec![],
            workspace_id: Some("ws_1".into()),
            session_id: Some("task_1".into()),
        });
        let (sender, receiver) = mpsc::channel();
        sender
            .send(PromptTaskMessage::NotSubmitted("offline".into()))
            .unwrap();
        let mut task = Some(PromptTask {
            receiver,
            stop_stream: Arc::new(AtomicBool::new(false)),
        });
        poll_prompt_task(&mut state, &mut task);
        assert!(task.is_none());
        assert!(!state.ui.execution_uncertain);
        assert_eq!(state.input, "draft to restore");
        assert!(state
            .ui
            .notice
            .as_deref()
            .unwrap()
            .contains("not submitted"));
    }
    #[test]
    fn completion_merges_tail_history_before_final_stderr_summary_without_duplicates() {
        let mut state = TuiState::default();
        let early = json!({"id":"e1","provider_id":"codex","event_type":"session.agent.output","payload":{"text":"early ","stream":{"item_id":"m","mode":"delta","channel":"stdout"}}});
        apply_prompt_stream_event(&mut state, &early);
        apply_prompt_completion(
            &mut state,
            PromptTaskCompletion {
                workspace_id: None,
                workspace_display: None,
                session_id: "task_1".into(),
                active_provider: Some("codex".into()),
                route_reason: None,
                prompt_response: json!({"status":"failed","turn_status":"failed","session_status":"Failed","stderr":"already streamed"}),
                events_response: json!({"events":[early,{"id":"e2","provider_id":"codex","event_type":"session.agent.output","payload":{"text":"tail","stream":{"item_id":"m","mode":"delta","channel":"stdout"}}},{"id":"e3","provider_id":"codex","event_type":"session.agent.diagnostic","payload":{"text":"already streamed","stream":{"item_id":"stderr","mode":"delta","channel":"stderr"}}}]}),
                routes_response: json!({"routes":[]}),
                diff_response: json!({"diff":{"dirty":false}}),
            },
        );
        let text = state.transcript_text();
        assert_eq!(text.matches("early tail").count(), 1);
        assert_eq!(text.matches("already streamed").count(), 1);
    }

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
