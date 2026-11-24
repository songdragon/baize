use super::*;

pub(super) fn request_cancel_confirmation(state: &mut TuiState) {
    if state.session_id.is_none() {
        state.ui.notice = Some("Start or open a session first".into());
    } else if state.ui.session_status == "Canceled" {
        state.ui.notice = Some("This session is already canceled".into());
    } else {
        state.ui.cancel_session_id = state.session_id.clone();
        open_overlay(state, Overlay::Cancel);
    }
}

// Separate the user's confirmation from dispatching a mutating HTTP request.
pub(super) fn confirm_cancel(state: &mut TuiState, background_busy: bool) -> bool {
    if state.ui.cancel_session_id.is_none()
        || state.ui.cancel_session_id != state.session_id
        || state.ui.session_status == "Canceled"
    {
        state.ui.overlay = None;
        state.ui.cancel_session_id = None;
        state.ui.notice = Some("The session changed; review cancellation again".into());
        false
    } else if background_busy {
        state.ui.notice =
            Some("Wait for the current background operation before confirming".into());
        false
    } else {
        state.ui.overlay = None;
        state.ui.cancel_session_id = None;
        true
    }
}

pub(super) fn permission_review_ready(state: &mut TuiState) -> bool {
    if state.ui.terminal_size.0 < 36 || state.ui.terminal_size.1 < 18 {
        state.ui.focus = Focus::Inspector;
        state.ui.panel = Panel::Permissions;
        state.detail_mode = true;
        state.ui.reviewed_permission_id = None;
        state.ui.notice = Some(
            "Resize to at least 36 × 18 to review permissions; PgUp/PgDn scroll details.".into(),
        );
        return false;
    }
    let selected = state
        .selected_permission()
        .map(|permission| permission.id.clone());
    if selected.is_some()
        && state.ui.focus == Focus::Inspector
        && state.ui.panel == Panel::Permissions
        && state.ui.reviewed_permission_id == selected
    {
        return true;
    }
    state.ui.focus = Focus::Inspector;
    state.ui.panel = Panel::Permissions;
    state.detail_mode = true;
    state.ui.inspector_scroll = 0;
    state.ui.reviewed_permission_id = selected;
    state.ui.notice = Some(
        if state.ui.reviewed_permission_id.is_some() {
            "Review the selected session, command and reason; Ctrl-A approves, Ctrl-D denies."
        } else {
            "No pending permission selected"
        }
        .into(),
    );
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_decisions_require_a_readable_terminal_and_review_again_after_resize() {
        let mut state = TuiState {
            pending_permissions: vec![PermissionView {
                id: "perm_1".into(),
                session_id: Some("task_1".into()),
                command: "cargo test".into(),
                reason: "review first".into(),
                status: "Pending".into(),
                risk_level: None,
            }],
            ..TuiState::default()
        };
        let (mut prompt, mut task) = (None, None);
        for size in [(35, 9), (36, 10), (80, 17)] {
            state.ui.terminal_size = size;
            for command in [CommandId::Approve, CommandId::Deny, CommandId::Approve] {
                run_command(&mut state, command, &mut prompt, &mut task).unwrap();
                assert!(task.is_none());
                assert!(state.ui.reviewed_permission_id.is_none());
                assert!(state.ui.notice.as_deref().unwrap().contains("Resize"));
            }
        }
        state.ui.terminal_size = (80, 18);
        assert!(!permission_review_ready(&mut state));
        handle_key(
            &mut state,
            KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
            &mut prompt,
            &mut task,
        )
        .unwrap();
        assert!(permission_review_ready(&mut state));
        assert_eq!(state.ui.inspector_scroll, 10);
        state.ui.terminal_size = (35, 9);
        assert!(!permission_review_ready(&mut state));
        state.ui.terminal_size = (80, 18);
        assert!(!permission_review_ready(&mut state));
        assert_eq!(state.ui.inspector_scroll, 0);
        assert!(prompt.is_none() && task.is_none());
    }

    #[test]
    fn an_uncertain_result_with_no_worker_explains_how_to_reconcile_without_resubmitting() {
        let mut state = TuiState {
            session_id: Some("task_uncertain".into()),
            input: "keep my draft".into(),
            image_paths: vec!["image.png".into()],
            ..TuiState::default()
        };
        state.ui.execution_uncertain = true;
        let (mut prompt, mut task) = (None, None);
        handle_key(
            &mut state,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut prompt,
            &mut task,
        )
        .unwrap();
        assert!(state.ui.notice.as_deref().unwrap().contains("F5"));
        assert_eq!(state.input, "keep my draft");
        assert_eq!(state.image_paths, [PathBuf::from("image.png")]);
        assert!(prompt.is_none() && task.is_none());
    }

    #[test]
    fn cancellation_requires_confirmation_and_escape_preserves_execution_and_draft() {
        let mut state = TuiState {
            session_id: Some("task_1".into()),
            input: "draft 中文".into(),
            ..TuiState::default()
        };
        let (_sender, receiver) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let mut prompt = Some(PromptTask {
            receiver,
            stop_stream: stop.clone(),
        });
        let mut ui = None;
        for _ in 0..3 {
            run_command(&mut state, CommandId::Cancel, &mut prompt, &mut ui).unwrap();
            assert_eq!(state.ui.overlay, Some(Overlay::Cancel));
            assert!(ui.is_none());
        }
        handle_key(
            &mut state,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &mut prompt,
            &mut ui,
        )
        .unwrap();
        assert!(state.ui.cancel_session_id.is_none());
        assert_eq!(state.input, "draft 中文");
        assert!(prompt.is_some() && !stop.load(Ordering::SeqCst));
        request_cancel_confirmation(&mut state);
        assert!(confirm_cancel(&mut state, false));
        assert!(!confirm_cancel(&mut state, false));
    }

    #[test]
    fn cancellation_rejects_missing_changed_or_canceled_targets_and_waits_for_worker() {
        let mut state = TuiState::default();
        request_cancel_confirmation(&mut state);
        assert!(state.ui.overlay.is_none());
        state.session_id = Some("old".into());
        request_cancel_confirmation(&mut state);
        state.session_id = Some("new".into());
        assert!(!confirm_cancel(&mut state, false));
        request_cancel_confirmation(&mut state);
        assert!(!confirm_cancel(&mut state, true));
        assert_eq!(state.ui.overlay, Some(Overlay::Cancel));
        assert!(confirm_cancel(&mut state, false));
        state.ui.session_status = "Canceled".into();
        request_cancel_confirmation(&mut state);
        assert!(state.ui.overlay.is_none());
    }

    #[test]
    fn both_permission_decisions_require_visible_review_and_refresh_preserves_identity() {
        fn permission(id: &str) -> PermissionView {
            PermissionView {
                id: id.into(),
                session_id: Some("other_session".into()),
                command: "cargo test".into(),
                reason: "review".into(),
                status: "Pending".into(),
                risk_level: None,
            }
        }
        for command in [CommandId::Approve, CommandId::Deny] {
            let mut state = TuiState::default();
            state.set_pending_permissions(vec![permission("one"), permission("two")]);
            let (mut prompt, mut ui) = (None, None);
            run_command(&mut state, command, &mut prompt, &mut ui).unwrap();
            assert!(ui.is_none());
            assert_eq!(state.ui.focus, Focus::Inspector);
            assert!(permission_review_ready(&mut state));
            state.set_pending_permissions(vec![permission("two"), permission("one")]);
            assert_eq!(state.selected_permission().unwrap().id, "one");
            assert!(permission_review_ready(&mut state));
            state.set_pending_permissions(vec![permission("two")]);
            assert!(!permission_review_ready(&mut state));
            assert_eq!(state.ui.reviewed_permission_id.as_deref(), Some("two"));
            state.set_pending_permissions(vec![]);
            assert!(!permission_review_ready(&mut state));
        }
    }

    #[test]
    fn cancel_dialog_renders_target_and_scope_in_normal_and_narrow_terminals() {
        use ratatui::backend::TestBackend;
        let mut state = TuiState {
            session_id: Some("task_target".into()),
            ..TuiState::default()
        };
        request_cancel_confirmation(&mut state);
        for (width, height) in [(36, 10), (80, 24), (112, 36)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| render(frame, &state)).unwrap();
            let text = format!("{:?}", terminal.backend().buffer());
            assert!(text.contains("task_target"));
            if width >= 80 {
                assert!(
                    text.contains("Provider may keep running") && text.contains("Enter cancel")
                );
            }
        }
    }
}
