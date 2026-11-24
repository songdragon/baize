use super::*;

pub(super) fn begin_prompt_submission(state: &mut TuiState) -> Option<PendingPromptSubmission> {
    let prompt = state.input.trim().to_string();
    if prompt.is_empty() {
        return None;
    }
    if handle_input_command(state, &prompt) {
        state.input.clear();
        return None;
    }

    let provider_id = state.selected_provider().to_string();
    if let Some(health) = selected_provider_unavailable(state) {
        let detail = provider_unavailable_detail(state, &provider_id, health);
        state.activity_status = "blocked".to_string();
        state.push_message(format!(
            "provider {provider_id} is not ready{detail}; press Tab to switch or Ctrl-R to refresh"
        ));
        return None;
    }

    // The composer now owns this draft; an earlier failed session load may
    // have left a cached copy under the old (including empty) session key.
    state
        .ui
        .drafts
        .remove(state.session_id.as_deref().unwrap_or_default());
    state
        .ui
        .recovery_drafts
        .remove(state.session_id.as_deref().unwrap_or_default());
    state.activity_status = format!("running {provider_id}");
    push_transcript_gap(state);
    if state.detail_mode {
        state.push_message("╭─ task ─────────────────────────────".to_string());
    }
    state.push_message(format!("> {prompt}"));
    if state.detail_mode {
        state.push_message(format!("  target: {provider_id}"));
        if !state.image_paths.is_empty() {
            state.push_message(format!("  images: {} attached", state.image_paths.len()));
        }
        state.push_message(format!(
            "  policy: {} ({})",
            state.command_policy_label(),
            state.execution_mode()
        ));
    } else {
        if !state.image_paths.is_empty() {
            state.push_message(format!("  attached {} image(s)", state.image_paths.len()));
        }
    }
    begin_output_turn(state, &provider_id, None);
    state.scroll_to_bottom();
    state.input.clear();
    let image_paths = std::mem::take(&mut state.image_paths);
    state.ui.recovery_drafts.insert(
        state.session_id.clone().unwrap_or_default(),
        (prompt.clone(), usize::MAX, image_paths.clone()),
    );

    Some(PendingPromptSubmission {
        prompt,
        provider_id,
        image_paths,
        workspace_id: state.workspace_id.clone(),
        session_id: state.session_id.clone(),
    })
}

pub(super) fn restore_unsubmitted_prompt(state: &mut TuiState) {
    if state.input.is_empty() && state.image_paths.is_empty() {
        if let Some(last) = &state.ui.last_submission {
            state.input = last.prompt.clone();
            state.image_paths = last.image_paths.clone();
            state.ui.cursor = usize::MAX;
            state
                .ui
                .recovery_drafts
                .remove(state.session_id.as_deref().unwrap_or_default());
        }
    }
}

pub(super) fn apply_prompt_task_failure(state: &mut TuiState, error: &str) {
    state.ui.execution_uncertain = state.session_id.is_some();
    finish_output_turn(
        state,
        if state.ui.execution_uncertain {
            "unconfirmed"
        } else {
            "not_submitted"
        },
        None,
    );
    state.activity_status = "failed".into();
    state.streaming_assistant_open = false;
    if state.ui.execution_uncertain {
        state.ui.notice = Some("Execution result uncertain. Inspect changes and history before sending again; recover the prompt via Ctrl-K.".into());
    } else {
        // Started is sent before any prompt POST. Without its session ID,
        // setup failed before submission and the original input is safe to restore.
        restore_unsubmitted_prompt(state);
        state.ui.notice = Some("Prompt setup failed before submission. Your draft is kept; review it before sending again.".into());
    }
    remember_recovery_draft(state);
    state.push_message(format!("error: {error}"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_failure_restores_only_an_empty_editor_and_never_retries_a_prompt() {
        for (input, images) in [
            ("", vec![]),
            ("new draft", vec![]),
            ("", vec![PathBuf::from("new.png")]),
        ] {
            let mut state = TuiState {
                input: input.into(),
                image_paths: images.clone(),
                ..TuiState::default()
            };
            state.ui.last_submission = Some(PendingPromptSubmission {
                prompt: "original".into(),
                image_paths: vec!["original.png".into()],
                provider_id: "codex".into(),
                workspace_id: None,
                session_id: None,
            });
            apply_prompt_task_failure(&mut state, "setup failed");
            assert!(!state.ui.execution_uncertain);
            if input.is_empty() && images.is_empty() {
                assert_eq!(state.input, "original");
                assert_eq!(state.image_paths, [PathBuf::from("original.png")]);
            } else {
                assert_eq!(state.input, input);
                assert_eq!(state.image_paths, images);
            }
        }
        let mut state = TuiState::default();
        apply_prompt_task_failure(&mut state, "worker disconnected");
        assert!(state.input.is_empty() && state.image_paths.is_empty());
    }

    #[test]
    fn failure_after_started_keeps_execution_uncertain_and_does_not_restore_for_resend() {
        let mut state = TuiState {
            session_id: Some("task_1".into()),
            ..TuiState::default()
        };
        state.ui.last_submission = Some(PendingPromptSubmission {
            prompt: "possibly submitted".into(),
            image_paths: vec![],
            provider_id: "codex".into(),
            workspace_id: None,
            session_id: state.session_id.clone(),
        });
        apply_prompt_task_failure(&mut state, "lost response");
        assert!(state.ui.execution_uncertain && state.input.is_empty());
        assert!(state.ui.notice.as_deref().unwrap().contains("uncertain"));
        assert!(current_draft(&state).is_none());
        assert_eq!(state.ui.recovery_drafts["task_1"].0, "possibly submitted");
    }
}
