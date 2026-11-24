use super::*;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Focus {
    #[default]
    Composer,
    Conversation,
    Sessions,
    Inspector,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Panel {
    #[default]
    Overview,
    Changes,
    Context,
    Handoff,
    Permissions,
    Providers,
}

impl Panel {
    pub(super) fn next(self) -> Self {
        match self {
            Self::Overview => Self::Changes,
            Self::Changes => Self::Context,
            Self::Context => Self::Handoff,
            Self::Handoff => Self::Permissions,
            Self::Permissions => Self::Providers,
            Self::Providers => Self::Overview,
        }
    }

    pub(super) fn title(self) -> &'static str {
        match self {
            Self::Overview => "Session",
            Self::Changes => "Changes",
            Self::Context => "Context",
            Self::Handoff => "Handoff",
            Self::Permissions => "Permissions",
            Self::Providers => "Providers",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Overlay {
    Help,
    Commands,
    Sessions,
    Quit,
    Cancel,
}

#[derive(Debug, Clone)]
pub(crate) struct UiState {
    pub output: ConversationOutput,
    pub stderr_seen: bool,
    pub wrap_width: u16,
    pub viewport_height: usize,
    pub terminal_size: (u16, u16),
    pub execution_uncertain: bool,
    pub focus: Focus,
    pub panel: Panel,
    pub overlay: Option<Overlay>,
    pub query: String,
    pub menu_index: usize,
    pub cursor: usize,
    pub inspector_scroll: u16,
    pub diff: Value,
    pub context: Value,
    pub handoff: Value,
    pub session_status: String,
    pub notice: Option<String>,
    pub drafts: HashMap<String, (String, usize, Vec<PathBuf>)>,
    pub recovery_drafts: HashMap<String, (String, usize, Vec<PathBuf>)>,
    pub last_submission: Option<PendingPromptSubmission>,
    pub cancel_session_id: Option<String>,
    pub reviewed_permission_id: Option<String>,
    pub drafts_persisted: bool,
    pub draft_error: Option<String>,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            output: ConversationOutput::default(),
            stderr_seen: false,
            wrap_width: 75,
            viewport_height: 16,
            terminal_size: (112, 36),
            execution_uncertain: false,
            focus: Focus::Composer,
            panel: Panel::Overview,
            overlay: None,
            query: String::new(),
            menu_index: 0,
            cursor: usize::MAX,
            inspector_scroll: 0,
            diff: Value::Null,
            context: Value::Null,
            handoff: Value::Null,
            session_status: "New".to_string(),
            notice: None,
            drafts: HashMap::new(),
            recovery_drafts: HashMap::new(),
            last_submission: None,
            cancel_session_id: None,
            reviewed_permission_id: None,
            drafts_persisted: false,
            draft_error: None,
        }
    }
}

pub(super) fn open_overlay(state: &mut TuiState, overlay: Overlay) {
    state.ui.overlay = Some(overlay);
    state.ui.query.clear();
    state.ui.menu_index = 0;
}

pub(super) fn session_matches(state: &TuiState) -> Vec<usize> {
    let query = state.ui.query.to_lowercase();
    state
        .recent_sessions
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, session)| {
            format!(
                "{} {} {} {}",
                session.id,
                session.objective,
                session.status,
                session.active_provider_id.as_deref().unwrap_or("")
            )
            .to_lowercase()
            .contains(&query)
        })
        .map(|(index, _)| index)
        .collect()
}

pub(super) fn save_draft(state: &mut TuiState) {
    remember_recovery_draft(state);
    let key = state.session_id.clone().unwrap_or_default();
    if let Some(draft) = current_draft(state) {
        state.ui.drafts.insert(key, draft);
    } else {
        state.ui.drafts.remove(&key);
    }
}

pub(super) fn restore_draft(state: &mut TuiState) {
    let key = state.session_id.clone().unwrap_or_default();
    let (input, cursor, images) = state.ui.drafts.remove(&key).unwrap_or_default();
    state.input = input;
    state.ui.cursor = cursor;
    state.image_paths = images;
    restore_recovery_submission(state);
}

pub(super) fn replay_session(state: &mut TuiState, events: &Value) -> Result<()> {
    let events = events
        .get("events")
        .and_then(Value::as_array)
        .context("missing session events")?;
    state.transcript.clear();
    state.seen_event_ids.clear();
    reset_stream_output(state);
    state.streaming_assistant_open = false;
    state.ui.execution_uncertain = false;
    let mut terminal_result = false;
    for event in events {
        if !mark_event_seen(state, event) {
            continue;
        }
        let payload = &event["payload"];
        let provider = event["provider_id"].as_str().unwrap_or("agent");
        match event["event_type"].as_str().unwrap_or("") {
            "session.agent.started" => {
                state.ui.execution_uncertain = true;
                terminal_result = false;
                if let Some(prompt) = payload["prompt"].as_str() {
                    push_transcript_gap(state);
                    state.push_message(format!("> {prompt}"));
                }
                begin_output_turn(state, provider, event_time(event));
                render_agent_event(state, event);
            }
            "session.agent.completed" if payload["status"].as_str() != Some("canceled") => {
                state.ui.execution_uncertain = false;
                terminal_result = true;
                render_agent_event(state, event);
            }
            "session.recovered"
            | "session.agent.failed"
            | "session.agent.ignored"
            | "session.completed" => {
                state.ui.execution_uncertain = false;
                terminal_result = true;
                render_agent_event(state, event);
                state.push_message(status_event_line(
                    event["event_type"].as_str().unwrap_or("status"),
                    payload,
                ));
            }
            "session.agent.output"
            | "session.agent.tool_call"
            | "session.agent.diagnostic"
            | "session.agent.status" => render_agent_event(state, event),
            "session.canceled" => {
                render_agent_event(state, event);
                state.push_message(status_event_line(
                    event["event_type"].as_str().unwrap_or("status"),
                    payload,
                ));
            }
            _ => {}
        }
    }
    if terminal_result {
        state
            .ui
            .recovery_drafts
            .remove(state.session_id.as_deref().unwrap_or_default());
    }
    state.activity_status = if state.ui.execution_uncertain {
        "execution unconfirmed"
    } else {
        "idle"
    }
    .to_string();
    if state.ui.execution_uncertain {
        state.ui.notice = Some(
            "A turn may still be running. F5 reconciles recorded results; do not resubmit yet."
                .into(),
        );
    }
    state.scroll_to_bottom();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_markers_do_not_unlock_new_sessions_until_execution_finishes() {
        let mut state = TuiState {
            session_id: Some("task_old".into()),
            input: "draft".into(),
            ..TuiState::default()
        };
        state.ui.session_status = "Canceled".into();
        let mut events = vec![
            json!({"id":"started","event_type":"session.agent.started","payload":{"prompt":"running"}}),
            json!({"id":"cancel","event_type":"session.agent.completed","payload":{"status":"canceled"}}),
        ];
        replay_session(&mut state, &json!({"events":events})).unwrap();
        let (mut prompt, mut task) = (None, None);
        run_command(&mut state, CommandId::New, &mut prompt, &mut task).unwrap();
        assert!(state.ui.execution_uncertain);
        assert_eq!(state.session_id.as_deref(), Some("task_old"));
        assert_eq!(state.input, "draft");
        events.push(json!({"id":"finished","event_type":"session.agent.ignored","payload":{"reason":"provider returned after cancel"}}));
        replay_session(&mut state, &json!({"events":events})).unwrap();
        assert!(!state.ui.execution_uncertain);
        run_command(&mut state, CommandId::New, &mut prompt, &mut task).unwrap();
        assert!(state.session_id.is_none());
        assert_eq!(state.ui.drafts["task_old"].0, "draft");
    }

    #[test]
    fn replay_retains_roles_provider_order_and_deduplicates() {
        let mut state = TuiState::default();
        let events = json!({"events": [
            {"id":"1","event_type":"session.agent.started","provider_id":"codex","payload":{"prompt":"keep API"}},
            {"id":"2","event_type":"session.agent.output","payload":{"text":"first response"}},
            {"id":"3","event_type":"session.agent.started","provider_id":"opencode","payload":{"prompt":"continue"}},
            {"id":"4","event_type":"session.agent.output","payload":{"text":"second response"}},
            {"id":"4","event_type":"session.agent.output","payload":{"text":"duplicate"}}
        ]});
        replay_session(&mut state, &events).unwrap();
        let text = state.transcript_text();
        assert!(text.find("keep API").unwrap() < text.find("continue").unwrap());
        assert!(text.contains("codex · CLI") && text.contains("opencode · CLI"));
        assert!(!text.contains("duplicate"));
        assert!(replay_session(&mut state, &json!({})).is_err());
    }

    #[test]
    fn drafts_and_search_are_session_scoped() {
        let mut state = TuiState {
            input: "draft 中文".to_string(),
            ..TuiState::default()
        };
        save_draft(&mut state);
        state.session_id = Some("other".to_string());
        restore_draft(&mut state);
        assert!(state.input.is_empty());
        state.session_id = None;
        restore_draft(&mut state);
        assert_eq!(state.input, "draft 中文");
        state.set_recent_sessions(vec![SessionView {
            id: "task_1".into(),
            workspace_id: "ws_1".into(),
            objective: "修复 Auth".into(),
            active_provider_id: Some("codex".into()),
            status: "Running".into(),
        }]);
        state.ui.query = "AUTH".into();
        assert_eq!(session_matches(&state), vec![0]);
        state.ui.query = "missing".into();
        assert!(session_matches(&state).is_empty());
    }
}
