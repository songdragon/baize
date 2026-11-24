use super::*;

pub(super) type Draft = (String, usize, Vec<PathBuf>);
pub(super) type DraftMap = HashMap<String, Draft>;

pub(super) fn current_draft(state: &TuiState) -> Option<Draft> {
    if !state.input.is_empty() || !state.image_paths.is_empty() {
        Some((
            state.input.clone(),
            state.ui.cursor,
            state.image_paths.clone(),
        ))
    } else {
        None
    }
}

pub(super) fn remember_recovery_draft(state: &mut TuiState) {
    if state.ui.execution_uncertain || state.activity_status.starts_with("running") {
        if let Some(last) = &state.ui.last_submission {
            state.ui.recovery_drafts.insert(
                state.session_id.clone().unwrap_or_default(),
                (last.prompt.clone(), usize::MAX, last.image_paths.clone()),
            );
        }
    }
}

pub(super) fn restore_recovery_submission(state: &mut TuiState) {
    if let Some((prompt, _, images)) = state
        .ui
        .recovery_drafts
        .get(state.session_id.as_deref().unwrap_or_default())
    {
        state.ui.last_submission = Some(PendingPromptSubmission {
            prompt: prompt.clone(),
            image_paths: images.clone(),
            provider_id: state.selected_provider().into(),
            workspace_id: state.workspace_id.clone(),
            session_id: state.session_id.clone(),
        });
    }
}

pub(super) fn draft_snapshot(state: &TuiState) -> Result<Value> {
    let mut drafts = state.ui.drafts.clone();
    let key = state.session_id.clone().unwrap_or_default();
    if let Some(draft) = current_draft(state) {
        drafts.insert(key, draft);
    } else {
        drafts.remove(&key);
    }
    drafts.retain(|_, (text, _, images)| !text.is_empty() || !images.is_empty());
    let mut recovery = state.ui.recovery_drafts.clone();
    recovery.retain(|_, (text, _, images)| !text.is_empty() || !images.is_empty());
    Ok(json!({
        "drafts":serde_json::to_value(drafts).context("serialize client drafts")?,
        "submissions":serde_json::to_value(recovery).context("serialize recoverable submissions")?
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn invalid_attachment_paths_fail_serialization_without_panicking() {
        use std::os::unix::ffi::OsStringExt;
        let state = TuiState {
            image_paths: vec![PathBuf::from(std::ffi::OsString::from_vec(vec![0xff]))],
            ..TuiState::default()
        };
        assert!(draft_snapshot(&state).is_err());
    }

    #[test]
    fn explicit_recovery_does_not_replace_an_attachment_only_draft() {
        let mut state = TuiState {
            image_paths: vec![PathBuf::from("new.png")],
            ..TuiState::default()
        };
        state.ui.last_submission = Some(PendingPromptSubmission {
            prompt: "old input".into(),
            image_paths: vec!["old.png".into()],
            provider_id: "codex".into(),
            workspace_id: None,
            session_id: None,
        });
        let (mut prompt, mut task) = (None, None);
        run_command(&mut state, CommandId::RecoverDraft, &mut prompt, &mut task).unwrap();
        assert!(state.input.is_empty());
        assert_eq!(state.image_paths, [PathBuf::from("new.png")]);
        assert!(state.ui.notice.as_deref().unwrap().contains("Clear"));
        assert!(prompt.is_none() && task.is_none());
    }

    #[test]
    fn a_new_editor_draft_and_a_failed_first_submission_both_survive_restart() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("drafts.json");
        let mut state = TuiState::default();
        let mut store = DraftStore::open(path.clone(), "one".into(), &mut state).unwrap();
        state.input = "original request".into();
        state.ui.last_submission = begin_prompt_submission(&mut state);
        state.input = "new unsent draft".into();
        apply_prompt_task_failure(&mut state, "setup failed");
        store.checkpoint(&mut state);
        let mut restored = TuiState::default();
        DraftStore::open(path, "one".into(), &mut restored).unwrap();
        assert_eq!(restored.input, "new unsent draft");
        assert_eq!(
            restored.ui.last_submission.as_ref().unwrap().prompt,
            "original request"
        );
    }

    #[test]
    fn session_drafts_and_uncertain_submissions_are_retained_without_automatic_retry() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("drafts.json");
        let mut state = TuiState::default();
        let mut store = DraftStore::open(path.clone(), "one".into(), &mut state).unwrap();
        state.session_id = Some("task_1".into());
        state.input = "draft".into();
        save_draft(&mut state);
        state.session_id = Some("task_2".into());
        state.input.clear();
        state.ui.execution_uncertain = true;
        state.ui.last_submission = Some(PendingPromptSubmission {
            prompt: "possibly submitted".into(),
            provider_id: "codex".into(),
            image_paths: vec![],
            workspace_id: None,
            session_id: state.session_id.clone(),
        });
        store.checkpoint(&mut state);
        let mut restored = TuiState::default();
        let mut reloaded = DraftStore::open(path.clone(), "one".into(), &mut restored).unwrap();
        assert!(restored.input.is_empty());
        restored.session_id = Some("task_2".into());
        restore_draft(&mut restored);
        assert!(restored.input.is_empty());
        assert_eq!(
            restored.ui.last_submission.as_ref().unwrap().prompt,
            "possibly submitted"
        );
        replay_session(&mut restored, &json!({"events":[{"id":"done","event_type":"session.agent.completed","payload":{"status":"completed"}}]})).unwrap();
        reloaded.checkpoint(&mut restored);
        let mut final_state = TuiState::default();
        DraftStore::open(path, "one".into(), &mut final_state).unwrap();
        assert!(!final_state.ui.drafts.contains_key("task_2"));
        assert!(!final_state.ui.recovery_drafts.contains_key("task_2"));
        assert_eq!(final_state.ui.drafts["task_1"].0, "draft");
    }

    #[test]
    fn first_submission_failure_before_started_keeps_a_restartable_draft() {
        for disconnected in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("drafts.json");
            let mut state = TuiState::default();
            let mut store = DraftStore::open(path.clone(), "one".into(), &mut state).unwrap();
            state.input = "first prompt 中文".into();
            state.image_paths = vec![PathBuf::from("image.png")];
            store.checkpoint(&mut state);
            state.ui.last_submission = begin_prompt_submission(&mut state);
            let (sender, receiver) = mpsc::channel();
            if !disconnected {
                sender
                    .send(PromptTaskMessage::Failed("create session failed".into()))
                    .unwrap();
            }
            drop(sender);
            let mut task = Some(PromptTask {
                receiver,
                stop_stream: Arc::new(AtomicBool::new(false)),
            });
            poll_prompt_task(&mut state, &mut task);
            assert!(task.is_none() && state.session_id.is_none());
            store.checkpoint(&mut state);
            let mut recovered = TuiState::default();
            DraftStore::open(path, "one".into(), &mut recovered).unwrap();
            assert_eq!(recovered.input, "first prompt 中文");
            assert_eq!(recovered.image_paths, [PathBuf::from("image.png")]);
        }
    }

    #[test]
    fn a_cached_new_conversation_draft_is_consumed_when_its_submission_gets_a_session() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("drafts.json");
        let mut state = TuiState::default();
        let mut store = DraftStore::open(path.clone(), "one".into(), &mut state).unwrap();
        state.input = "submit once".into();
        // Opening another session saves the current draft, even if that load fails.
        save_draft(&mut state);
        store.checkpoint(&mut state);
        state.ui.last_submission = begin_prompt_submission(&mut state);
        apply_prompt_started(
            &mut state,
            PromptTaskStarted {
                workspace_id: Some("ws_1".into()),
                workspace_display: None,
                session_id: "task_new".into(),
                active_provider: Some("codex".into()),
                route_reason: None,
                created_session: true,
            },
        );
        store.checkpoint(&mut state);
        apply_prompt_completion(
            &mut state,
            PromptTaskCompletion {
                workspace_id: Some("ws_1".into()),
                workspace_display: None,
                session_id: "task_new".into(),
                active_provider: Some("codex".into()),
                route_reason: None,
                prompt_response: json!({"status":"completed","session_status":"Running"}),
                events_response: json!({"events":[]}),
                routes_response: json!({"routes":[]}),
                diff_response: json!({"diff":{}}),
            },
        );
        store.checkpoint(&mut state);
        let mut recovered = TuiState::default();
        DraftStore::open(path, "one".into(), &mut recovered).unwrap();
        assert!(
            recovered.input.is_empty(),
            "A completed prompt must not return as an unsent draft"
        );
        assert!(recovered.ui.drafts.is_empty());
        assert!(recovered.ui.recovery_drafts.is_empty());
    }

    #[test]
    fn running_submission_without_a_session_id_requires_explicit_draft_recovery() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("drafts.json");
        let mut state = TuiState::default();
        let mut store = DraftStore::open(path.clone(), "one".into(), &mut state).unwrap();
        state.input = "possibly sent".into();
        state.ui.last_submission = begin_prompt_submission(&mut state);
        store.checkpoint(&mut state);
        let mut recovered = TuiState::default();
        DraftStore::open(path, "one".into(), &mut recovered).unwrap();
        assert!(recovered.input.is_empty());
        assert_eq!(
            recovered.ui.last_submission.as_ref().unwrap().prompt,
            "possibly sent"
        );
        let (mut prompt, mut task) = (None, None);
        run_command(
            &mut recovered,
            CommandId::RecoverDraft,
            &mut prompt,
            &mut task,
        )
        .unwrap();
        assert_eq!(recovered.input, "possibly sent");
        assert!(prompt.is_none() && task.is_none());
    }

    #[test]
    fn loading_completed_history_keeps_a_recovered_submission_out_of_the_composer() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("drafts.json");
        let mut state = TuiState::default();
        let mut store = DraftStore::open(path.clone(), "one".into(), &mut state).unwrap();
        state.session_id = Some("task_1".into());
        state.input = "already executed".into();
        state.ui.last_submission = begin_prompt_submission(&mut state);
        store.checkpoint(&mut state);
        let mut recovered = TuiState::default();
        DraftStore::open(path, "one".into(), &mut recovered).unwrap();
        apply_selected_session_load(
            &mut recovered,
            SelectedSessionLoad {
                workspace_display: "one".into(),
                session: json!({"id":"task_1","workspace_id":"ws_1","status":"Running","active_provider_id":"codex"}),
                events_response: json!({"events":[{"id":"start","event_type":"session.agent.started","payload":{"prompt":"already executed"}},{"id":"end","event_type":"session.agent.completed","payload":{"status":"completed"}}]}),
                routes_response: json!({"routes":[]}),
                diff_response: json!({"diff":{}}),
            },
        );
        assert!(!recovered.ui.execution_uncertain);
        assert!(recovered.input.is_empty());
        assert_eq!(
            recovered.ui.last_submission.as_ref().unwrap().prompt,
            "already executed"
        );
    }
}
