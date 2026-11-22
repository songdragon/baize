use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandId {
    New,
    Open,
    Latest,
    Refresh,
    Details,
    Process,
    Handoff,
    Accept,
    Permissions,
    Approve,
    Deny,
    Complete,
    Cancel,
    RecoverDraft,
    Help,
    Quit,
}

pub(super) const COMMANDS: &[(CommandId, &str, &str)] = &[
    (CommandId::New, "New conversation", "Ctrl-N"),
    (CommandId::Open, "Search and open session", "Ctrl-O"),
    (
        CommandId::Latest,
        "Resume latest workspace session",
        "Ctrl-L",
    ),
    (
        CommandId::Refresh,
        "Refresh providers / reconnect",
        "Ctrl-R",
    ),
    (CommandId::Details, "Refresh changes and context", "F5"),
    (CommandId::Process, "Fold / expand latest process", "F9"),
    (
        CommandId::Handoff,
        "Prepare handoff to target provider",
        "Ctrl-H",
    ),
    (CommandId::Accept, "Accept reviewed handoff", "Ctrl-Y"),
    (
        CommandId::Permissions,
        "Review pending permissions",
        "Ctrl-P",
    ),
    (CommandId::Approve, "Approve selected permission", "Ctrl-A"),
    (CommandId::Deny, "Deny selected permission", "Ctrl-D"),
    (CommandId::Complete, "Mark session complete", "Ctrl-E"),
    (
        CommandId::Cancel,
        "Cancel Baize session (provider may continue)",
        "Ctrl-X",
    ),
    (
        CommandId::RecoverDraft,
        "Recover last submitted prompt for editing",
        "",
    ),
    (CommandId::Help, "Keyboard help", "F1"),
    (CommandId::Quit, "Quit terminal client", "Ctrl-Q / Ctrl-C"),
];

pub(super) fn command_matches(state: &TuiState) -> Vec<usize> {
    let query = state.ui.query.to_lowercase();
    COMMANDS
        .iter()
        .enumerate()
        .filter(|(_, (_, label, key))| format!("{label} {key}").to_lowercase().contains(&query))
        .map(|(i, _)| i)
        .collect()
}

pub(super) fn run_command(
    state: &mut TuiState,
    command: CommandId,
    prompt: &mut Option<PromptTask>,
    ui: &mut Option<UiTask>,
) -> Result<bool> {
    let busy = prompt.is_some() || ui.is_some() || state.ui.execution_uncertain;
    if matches!(
        command,
        CommandId::New
            | CommandId::Latest
            | CommandId::Handoff
            | CommandId::Accept
            | CommandId::Complete
    ) && busy
    {
        state.ui.notice = Some(
            "Wait for the current turn or reconcile cancellation before changing session state"
                .into(),
        );
        return Ok(false);
    }
    match command {
        CommandId::New => {
            save_draft(state);
            start_new_session(state);
            restore_draft(state);
            state.ui.focus = Focus::Composer;
        }
        CommandId::Open => {
            open_overlay(state, Overlay::Sessions);
            if ui.is_none() {
                request_action(state, ui, Action::Sessions);
            }
        }
        CommandId::Latest => {
            state.selected_session_index = state.recent_sessions.len().saturating_sub(1);
            open_selected(state, prompt, ui);
        }
        CommandId::Refresh => {
            start_ui_task_if_idle(state, ui, "Refreshing connection", start_initial_load_task);
        }
        CommandId::Details => request_action(state, ui, Action::Details),
        CommandId::Process => {
            process_key(state, KeyEvent::new(KeyCode::F(9), KeyModifiers::NONE));
        }
        CommandId::Handoff => request_action(state, ui, Action::PreviewHandoff),
        CommandId::Accept => request_action(state, ui, Action::AcceptHandoff),
        CommandId::Permissions => {
            state.detail_mode = true;
            state.ui.panel = Panel::Permissions;
            state.ui.focus = Focus::Inspector;
            start_ui_task_if_idle(
                state,
                ui,
                "Refreshing permissions",
                start_permission_refresh_task,
            );
        }
        CommandId::Approve | CommandId::Deny => {
            if permission_review_ready(state) {
                request_action(
                    state,
                    ui,
                    if command == CommandId::Approve {
                        Action::Approve
                    } else {
                        Action::Deny
                    },
                );
            }
        }
        CommandId::Complete => request_action(state, ui, Action::Complete),
        CommandId::Cancel => request_cancel_confirmation(state),
        CommandId::RecoverDraft => {
            if let Some(last) = state.ui.last_submission.clone() {
                if !state.input.is_empty() {
                    state.ui.notice = Some(
                        "Clear the current draft before recovering the last submission".into(),
                    );
                } else {
                    state.input = last.prompt;
                    state.image_paths = last.image_paths;
                    state.ui.cursor = usize::MAX;
                    state.ui.notice=Some("Draft restored for review. Check the previous result before sending again.".into());
                }
            }
        }
        CommandId::Help => open_overlay(state, Overlay::Help),
        CommandId::Quit => open_overlay(state, Overlay::Quit),
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn idle_quit_shortcuts_and_menu_always_require_explicit_confirmation() {
        for shortcut in ['c', 'q'] {
            for overlay in [
                None,
                Some(Overlay::Help),
                Some(Overlay::Commands),
                Some(Overlay::Sessions),
                Some(Overlay::Quit),
            ] {
                let mut state = TuiState::default();
                state.ui.overlay = overlay;
                let (mut prompt, mut task) = (None, None);
                for _ in 0..3 {
                    assert!(!handle_key(
                        &mut state,
                        KeyEvent::new(KeyCode::Char(shortcut), KeyModifiers::CONTROL),
                        &mut prompt,
                        &mut task
                    )
                    .unwrap());
                    assert_eq!(state.ui.overlay, Some(Overlay::Quit));
                }
                assert!(!handle_key(
                    &mut state,
                    KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
                    &mut prompt,
                    &mut task
                )
                .unwrap());
                assert!(state.ui.overlay.is_none());
            }
        }
        let mut state = TuiState::default();
        let (mut prompt, mut task) = (None, None);
        assert!(!run_command(&mut state, CommandId::Quit, &mut prompt, &mut task).unwrap());
        assert!(handle_key(
            &mut state,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut prompt,
            &mut task
        )
        .unwrap());
        state.ui.query = "ctrl-c".into();
        assert!(command_matches(&state)
            .iter()
            .any(|index| COMMANDS[*index].0 == CommandId::Quit));
    }

    #[test]
    fn quit_confirmation_preserves_running_tasks_and_all_drafts_until_enter() {
        let mut state = TuiState {
            input: "未发送的草稿".into(),
            image_paths: vec![PathBuf::from("/tmp/attachment.png")],
            session_id: Some("task_1".into()),
            ..TuiState::default()
        };
        state.ui.execution_uncertain = true;
        state
            .ui
            .drafts
            .insert("other".into(), ("saved draft".into(), 0, vec![]));
        let (_sender, receiver) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let mut prompt = Some(PromptTask {
            receiver,
            stop_stream: stop.clone(),
        });
        let (_ui_sender, receiver) = mpsc::channel();
        let mut task = Some(UiTask { receiver });
        assert!(!handle_key(
            &mut state,
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            &mut prompt,
            &mut task
        )
        .unwrap());
        assert!(!handle_key(
            &mut state,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &mut prompt,
            &mut task
        )
        .unwrap());
        assert!(state.ui.overlay.is_none());
        assert_eq!(state.input, "未发送的草稿");
        assert_eq!(state.image_paths, [PathBuf::from("/tmp/attachment.png")]);
        assert_eq!(state.ui.drafts["other"].0, "saved draft");
        assert_eq!(state.session_id.as_deref(), Some("task_1"));
        assert!(state.ui.execution_uncertain && prompt.is_some() && task.is_some());
        assert!(!stop.load(Ordering::SeqCst));
        assert!(!handle_key(
            &mut state,
            KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL),
            &mut prompt,
            &mut task
        )
        .unwrap());
        assert!(handle_key(
            &mut state,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut prompt,
            &mut task
        )
        .unwrap());
        assert!(!stop.load(Ordering::SeqCst));
    }
    #[test]
    fn approval_first_opens_review_and_hidden_drafts_require_exit_confirmation() {
        let mut state = TuiState::default();
        let (mut prompt, mut task) = (None, None);
        run_command(&mut state, CommandId::Approve, &mut prompt, &mut task).unwrap();
        assert_eq!(state.ui.focus, Focus::Inspector);
        assert_eq!(state.ui.panel, Panel::Permissions);
        assert!(task.is_none());
        state
            .ui
            .drafts
            .insert("other".into(), ("unsent".into(), 0, vec![]));
        assert!(!run_command(&mut state, CommandId::Quit, &mut prompt, &mut task).unwrap());
        assert_eq!(state.ui.overlay, Some(Overlay::Quit));
    }
    #[test]
    fn unconfirmed_execution_blocks_new_conversation_without_repeating_work() {
        let mut state = TuiState {
            session_id: Some("task_1".into()),
            ..TuiState::default()
        };
        state.ui.execution_uncertain = true;
        let (mut prompt, mut task) = (None, None);
        run_command(&mut state, CommandId::New, &mut prompt, &mut task).unwrap();
        assert_eq!(state.session_id.as_deref(), Some("task_1"));
        assert!(task.is_none());
        assert!(state.ui.notice.is_some());
    }
}
