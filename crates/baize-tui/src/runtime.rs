use super::*;

struct TerminalGuard;
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = crossterm::execute!(
        stdout(),
        DisableBracketedPaste,
        DisableMouseCapture,
        LeaveAlternateScreen,
        crossterm::cursor::Show
    );
}

pub fn run() -> Result<()> {
    let config = baize_config::load_or_default()?;
    let _ = DAEMON_ADDRESS.set(configured_daemon_address(&config)?);
    let launch = ensure_daemon_running().unwrap_or_else(|error| DaemonLaunch {
        status: format!("daemon: unavailable ({error:#})"),
        guard: None,
    });
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        previous_hook(info);
    }));
    let _guard = TerminalGuard;
    enable_raw_mode()?;
    crossterm::execute!(
        stdout(),
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste
    )?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
    // The persistent daemon outlives this client, including when a turn is still running.
    let _daemon = launch.guard;
    run_app(&mut terminal, launch.status)
}

fn run_app(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    daemon_status: String,
) -> Result<()> {
    let mut state = TuiState {
        daemon_status,
        ..TuiState::default()
    };
    let mut draft_store =
        match DraftStore::open(default_draft_path(), state.workspace.clone(), &mut state) {
            Ok(store) => Some(store),
            Err(error) => {
                state.ui.draft_error = Some(format!("Draft recovery unavailable: {error:#}"));
                None
            }
        };
    let mut prompt_task = None;
    let mut ui_task = Some(start_initial_load_task());
    let mut workspace_loaded = false;
    state.background_status = Some("Connecting".into());
    loop {
        let size = terminal.size()?;
        state.ui.wrap_width = conversation_wrap_width(&state, size.width);
        let (_, cursor_row, _) = editor_rows(&state, size.width.saturating_sub(6));
        let composer_height = (cursor_row as u16 + 4)
            .clamp(4, 8)
            .min(size.height.saturating_sub(6));
        state.ui.viewport_height = usize::from(size.height.saturating_sub(composer_height + 5));
        poll_prompt_task(&mut state, &mut prompt_task);
        poll_ui_task(&mut state, &mut ui_task);
        if state.ui.session_status == "Canceled" && prompt_task.is_some() {
            detach_prompt_task_after_cancel(&mut state, &mut prompt_task);
        }
        if !workspace_loaded && ui_task.is_none() {
            workspace_loaded = true;
            state.background_status = Some("Loading workspace sessions".into());
            ui_task = Some(start_workspace_load_task());
        }
        if let Some(store) = draft_store.as_mut() {
            store.checkpoint(&mut state);
        }
        terminal.draw(|frame| render(frame, &state))?;
        if !event::poll(Duration::from_millis(UI_EVENT_POLL_MS))? {
            continue;
        }
        match event::read()? {
            Event::Key(key)
                if key.kind != KeyEventKind::Release
                    && handle_key(&mut state, key, &mut prompt_task, &mut ui_task)? =>
            {
                if let Some(store) = draft_store.as_mut() {
                    store.checkpoint(&mut state);
                }
                break;
            }
            Event::Paste(text) if state.ui.overlay.is_some() => {
                state
                    .ui
                    .query
                    .extend(text.chars().filter(|c| !c.is_control()));
                state.ui.menu_index = 0;
            }
            Event::Paste(text) => handle_paste(&mut state, &text),
            Event::Mouse(mouse) => match mouse.kind {
                MouseEventKind::ScrollUp if state.ui.focus == Focus::Inspector => {
                    state.ui.inspector_scroll = state.ui.inspector_scroll.saturating_sub(3)
                }
                MouseEventKind::ScrollDown if state.ui.focus == Focus::Inspector => {
                    state.ui.inspector_scroll = state.ui.inspector_scroll.saturating_add(3)
                }
                MouseEventKind::ScrollUp => state.scroll_up(3),
                MouseEventKind::ScrollDown => state.scroll_down(3),
                _ => {}
            },
            _ => {}
        }
    }
    Ok(())
}

pub(super) fn handle_key(
    state: &mut TuiState,
    key: KeyEvent,
    prompt: &mut Option<PromptTask>,
    ui: &mut Option<UiTask>,
) -> Result<bool> {
    if key.kind == KeyEventKind::Release {
        return Ok(false);
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c' | 'q'))
    {
        return run_command(state, CommandId::Quit, prompt, ui);
    }
    if let Some(overlay) = state.ui.overlay {
        match key.code {
            KeyCode::Esc => {
                state.ui.overlay = None;
                state.ui.cancel_session_id = None;
            }
            KeyCode::Enter if overlay == Overlay::Quit => return Ok(true),
            KeyCode::Enter if overlay == Overlay::Cancel => {
                if !confirm_cancel(state, ui.is_some()) {
                    return Ok(false);
                }
                request_action(state, ui, Action::Cancel);
            }
            KeyCode::Enter if overlay == Overlay::Help => state.ui.overlay = None,
            KeyCode::Enter => {
                let index = state.ui.menu_index;
                if overlay == Overlay::Commands {
                    if let Some(i) = command_matches(state).get(index) {
                        let command = COMMANDS[*i].0;
                        state.ui.overlay = None;
                        return run_command(state, command, prompt, ui);
                    }
                } else if let Some(i) = session_matches(state).get(index) {
                    state.selected_session_index = *i;
                    state.ui.overlay = None;
                    open_selected(state, prompt, ui);
                }
            }
            KeyCode::Down if overlay == Overlay::Help => {
                state.ui.menu_index = (state.ui.menu_index + 1).min(COMMANDS.len() + 7)
            }
            KeyCode::PageDown if overlay == Overlay::Help => {
                state.ui.menu_index = (state.ui.menu_index + 5).min(COMMANDS.len() + 7)
            }
            KeyCode::PageUp if overlay == Overlay::Help => {
                state.ui.menu_index = state.ui.menu_index.saturating_sub(5)
            }
            KeyCode::Down => {
                let len = if overlay == Overlay::Commands {
                    command_matches(state).len()
                } else {
                    session_matches(state).len()
                };
                state.ui.menu_index = (state.ui.menu_index + 1).min(len.saturating_sub(1));
            }
            KeyCode::Up => state.ui.menu_index = state.ui.menu_index.saturating_sub(1),
            KeyCode::Backspace => {
                state.ui.query.pop();
                state.ui.menu_index = 0;
            }
            KeyCode::Char(ch)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                state.ui.query.push(ch);
                state.ui.menu_index = 0;
            }
            _ => {}
        }
        return Ok(false);
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        let command = match key.code {
            KeyCode::Char('n') => Some(CommandId::New),
            KeyCode::Char('o') => Some(CommandId::Open),
            KeyCode::Char('l') => Some(CommandId::Latest),
            KeyCode::Char('r') => Some(CommandId::Refresh),
            KeyCode::Char('h') => Some(CommandId::Handoff),
            KeyCode::Char('y') => Some(CommandId::Accept),
            KeyCode::Char('p') => Some(CommandId::Permissions),
            KeyCode::Char('a') => Some(CommandId::Approve),
            KeyCode::Char('d') => Some(CommandId::Deny),
            KeyCode::Char('e') => Some(CommandId::Complete),
            KeyCode::Char('x') => Some(CommandId::Cancel),
            _ => None,
        };
        if let Some(command) = command {
            return run_command(state, command, prompt, ui);
        }
        match key.code {
            KeyCode::Char('k') => open_overlay(state, Overlay::Commands),
            KeyCode::Char('j') => insert_text(state, "\n"),
            KeyCode::Char('v') => attach_clipboard_image(state),
            _ => {}
        }
        return Ok(false);
    }
    if process_key(state, key) {
        return Ok(false);
    }
    match key.code {
        KeyCode::F(1) => open_overlay(state, Overlay::Help),
        KeyCode::F(2) => {
            state.ui.focus = Focus::Sessions;
            state.select_previous_recent_session();
        }
        KeyCode::F(3) => {
            state.ui.focus = Focus::Sessions;
            state.select_next_recent_session();
        }
        KeyCode::F(4) => open_selected(state, prompt, ui),
        KeyCode::F(5) => request_action(state, ui, Action::Details),
        KeyCode::F(6) => move_focus(state, false),
        KeyCode::BackTab => move_focus(state, true),
        KeyCode::F(7) => {
            state.ui.focus = Focus::Inspector;
            state.detail_mode = true;
            state.ui.panel = state.ui.panel.next();
            state.ui.inspector_scroll = 0;
        }
        KeyCode::F(8) => {
            state.detail_mode = !state.detail_mode;
            if !state.detail_mode {
                state.ui.focus = Focus::Composer;
            }
        }
        KeyCode::Tab => {
            state.cycle_provider();
            state.ui.context = Value::Null;
        }
        KeyCode::Esc => {
            state.ui.focus = Focus::Composer;
            state.ui.notice = None;
        }
        KeyCode::PageUp if state.ui.focus == Focus::Inspector => {
            state.ui.inspector_scroll = state.ui.inspector_scroll.saturating_sub(10)
        }
        KeyCode::PageDown if state.ui.focus == Focus::Inspector => {
            state.ui.inspector_scroll = state.ui.inspector_scroll.saturating_add(10)
        }
        KeyCode::Home if state.ui.focus != Focus::Composer => state.scroll_to_top(),
        KeyCode::End if state.ui.focus != Focus::Composer => state.scroll_to_bottom(),
        KeyCode::PageUp => state.scroll_up(10),
        KeyCode::PageDown => state.scroll_down(10),
        KeyCode::Up if state.ui.focus == Focus::Sessions => {
            state.select_previous_recent_session();
        }
        KeyCode::Down if state.ui.focus == Focus::Sessions => {
            state.select_next_recent_session();
        }
        KeyCode::Up if state.ui.focus == Focus::Inspector => {
            if state.ui.panel == Panel::Permissions {
                state.select_previous_permission();
                state.ui.reviewed_permission_id = state
                    .selected_permission()
                    .map(|permission| permission.id.clone());
                state.ui.inspector_scroll = 0;
            } else {
                state.ui.inspector_scroll = state.ui.inspector_scroll.saturating_sub(1);
            }
        }
        KeyCode::Down if state.ui.focus == Focus::Inspector => {
            if state.ui.panel == Panel::Permissions {
                state.select_next_permission();
                state.ui.reviewed_permission_id = state
                    .selected_permission()
                    .map(|permission| permission.id.clone());
                state.ui.inspector_scroll = 0;
            } else {
                state.ui.inspector_scroll = state.ui.inspector_scroll.saturating_add(1);
            }
        }
        KeyCode::Enter if state.ui.focus == Focus::Sessions => open_selected(state, prompt, ui),
        KeyCode::Enter
            if state.ui.focus == Focus::Composer
                && !key
                    .modifiers
                    .intersects(KeyModifiers::SHIFT | KeyModifiers::ALT) =>
        {
            if prompt.is_some() || ui.is_some() || state.ui.execution_uncertain {
                state.ui.notice = Some("An operation is running. Your draft is kept.".into());
                return Ok(false);
            }
            if state.ui.execution_uncertain {
                state.ui.notice = Some(
                    "Execution is unconfirmed. F5 reconciles history before another submission."
                        .into(),
                );
                return Ok(false);
            }
            if state.ui.session_status == "Canceled" {
                state.ui.notice = Some(
                    "This session is canceled. Inspect workspace, then start a new conversation."
                        .into(),
                );
                return Ok(false);
            }
            if let Some(submission) = begin_prompt_submission(state) {
                state.ui.last_submission = Some(submission.clone());
                state.ui.cursor = 0;
                state.ui.session_status = "Running".into();
                state.ui.notice = None;
                *prompt = Some(start_prompt_task(submission)?);
            }
        }
        _ if state.ui.focus == Focus::Composer => {
            edit_key(state, key);
        }
        _ => {}
    }
    Ok(false)
}

pub(super) fn open_selected(
    state: &mut TuiState,
    prompt: &Option<PromptTask>,
    ui: &mut Option<UiTask>,
) {
    if prompt.is_some() || ui.is_some() {
        state.ui.notice = Some("Wait for the active operation before switching sessions".into());
        return;
    }
    save_draft(state);
    if let Some(task) = start_selected_session_load_task(state) {
        *ui = Some(task);
        state.background_status = Some("Restoring conversation".into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn process_focus_preserves_draft_and_enter_opens_details_without_submitting() {
        let mut state = TuiState {
            input: "my draft".into(),
            ..TuiState::default()
        };
        let (mut prompt, mut ui) = (None, None);
        handle_key(
            &mut state,
            KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
            &mut prompt,
            &mut ui,
        )
        .unwrap();
        assert_eq!(state.input, "my draft ");
        apply_prompt_stream_event(
            &mut state,
            &json!({"id":"1","event_type":"session.agent.tool_call","payload":{"text":"read [DONE]"}}),
        );
        run_command(&mut state, CommandId::Process, &mut prompt, &mut ui).unwrap();
        assert_eq!(state.ui.focus, Focus::Conversation);
        handle_key(
            &mut state,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut prompt,
            &mut ui,
        )
        .unwrap();
        assert!(state.ui.output.turns[0].details);
        assert!(prompt.is_none());
        handle_key(
            &mut state,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &mut prompt,
            &mut ui,
        )
        .unwrap();
        assert_eq!(state.ui.focus, Focus::Composer);
        assert_eq!(state.input, "my draft ");
    }
    #[test]
    fn escape_dismisses_overlay_without_quitting_or_losing_draft() {
        let mut state = TuiState {
            input: "draft".into(),
            ..TuiState::default()
        };
        let (mut prompt, mut ui) = (None, None);
        open_overlay(&mut state, Overlay::Help);
        assert!(!handle_key(
            &mut state,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &mut prompt,
            &mut ui
        )
        .unwrap());
        assert!(state.ui.overlay.is_none());
        assert_eq!(state.input, "draft");
        run_command(&mut state, CommandId::Quit, &mut prompt, &mut ui).unwrap();
        assert_eq!(state.ui.overlay, Some(Overlay::Quit));
        assert!(handle_key(
            &mut state,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut prompt,
            &mut ui
        )
        .unwrap());
    }
    #[test]
    fn blocks_new_session_while_running_and_preserves_draft_on_release() {
        let mut state = TuiState {
            session_id: Some("task_1".into()),
            input: "draft".into(),
            ..TuiState::default()
        };
        let (_, receiver) = mpsc::channel();
        let mut prompt = Some(PromptTask {
            receiver,
            stop_stream: Arc::new(AtomicBool::new(false)),
        });
        let mut ui = None;
        run_command(&mut state, CommandId::New, &mut prompt, &mut ui).unwrap();
        assert_eq!(state.session_id.as_deref(), Some("task_1"));
        handle_key(
            &mut state,
            KeyEvent::new_with_kind(
                KeyCode::Char('x'),
                KeyModifiers::NONE,
                KeyEventKind::Release,
            ),
            &mut prompt,
            &mut ui,
        )
        .unwrap();
        assert_eq!(state.input, "draft");
    }
    #[test]
    fn filters_commands_and_recovers_without_automatic_resubmission() {
        let mut state = TuiState::default();
        state.ui.query = "handoff".into();
        assert_eq!(command_matches(&state).len(), 2);
        state.ui.last_submission = Some(PendingPromptSubmission {
            prompt: "review me".into(),
            provider_id: "codex".into(),
            image_paths: vec![],
            workspace_id: None,
            session_id: None,
        });
        let (mut prompt, mut ui) = (None, None);
        run_command(&mut state, CommandId::RecoverDraft, &mut prompt, &mut ui).unwrap();
        assert_eq!(state.input, "review me");
        assert!(prompt.is_none());
    }
}
