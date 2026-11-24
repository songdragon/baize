use super::*;

#[tokio::test]
async fn timeout_keeps_confirmed_receipt_and_blocks_replay_until_exact_review() {
    let (mut state, app, _data, project, _executor) = environment();
    let (_, id) = setup_workspace_and_session(&app, &project).await;
    let first = prompt(&app, &id, "codex", "initial successful turn").await;
    let requests = Arc::new(Mutex::new(vec![]));
    state.executor = Arc::new(RecordingAgentExecutor {
        requests: requests.clone(),
        result: AgentRunResult {
            provider_id: ProviderId("codex".into()),
            success: false,
            exit_code: None,
            native_session_id: Some("native-codex".into()),
            events: vec![],
            stderr: "timed out with partial changes".into(),
            error: Some(AgentErrorDetail {
                kind: AgentErrorKind::Timeout,
                message: "timed out".into(),
                source: AgentErrorSource::Runtime,
            }),
        },
    });
    let app = router(state);
    let failed = prompt(&app, &id, "codex", "possibly modifies files").await;
    assert_eq!(
        failed["continuity"]["checkpoints"],
        first["continuity"]["checkpoints"]
    );
    assert_eq!(failed["continuity"]["active_turn"]["uncertain"], true);
    assert_eq!(
        api(
            &app,
            Method::POST,
            &format!("/sessions/{id}/prompt"),
            json!({"prompt":"do not retry automatically"})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(requests.lock().unwrap().len(), 1);
    let tid = failed["continuity"]["active_turn"]["id"].as_str().unwrap();
    assert_eq!(
        prompt(&app, &id, "codex", &format!("/reconcile {tid}")).await["local_command"],
        true
    );
    assert_eq!(requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn changed_execution_identity_requires_explicit_reset_before_a_new_entry_is_used() {
    let (state, app, _data, project, executor) = environment();
    let (_, id) = setup_workspace_and_session(&app, &project).await;
    prompt(&app, &id, "codex", "first turn").await;
    {
        let store = state.store.lock().unwrap();
        let mut session = store
            .get_task_session(&TaskSessionId(id.clone()))
            .unwrap()
            .unwrap();
        session
            .continuity
            .checkpoints
            .get_mut("codex")
            .unwrap()
            .execution_identity = "different CLI/profile/workspace".into();
        store.upsert_task_session(&session).unwrap();
    }
    let rejected = api(
        &app,
        Method::POST,
        &format!("/sessions/{id}/prompt"),
        json!({"prompt":"must not assume same native entry"}),
    )
    .await;
    assert_eq!(rejected.0, StatusCode::CONFLICT);
    assert_eq!(rejected.1["not_submitted"], true);
    assert!(rejected.1["error"]
        .as_str()
        .unwrap()
        .contains("/reset-provider"));
    assert_eq!(executor.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn handoff_preview_and_accept_preserve_receipts_and_show_the_actual_draft_sources() {
    let (_, app, _data, project, executor) = environment();
    let (_, id) = setup_workspace_and_session(&app, &project).await;
    prompt(&app, &id, "codex", "/remember api keep public API").await;
    let first = prompt(&app, &id, "codex", "initial task").await;
    let handoff=api(&app,Method::POST,&format!("/sessions/{id}/handoff"),json!({"to_provider_id":"antigravity","prompt":"unsent user correction","user_constraints":["legacy constraint"]})).await;
    assert_eq!(handoff.0, StatusCode::OK);
    let preview = &handoff.1["context_preview"];
    assert_eq!(preview["preview_only"], true);
    assert!(preview["prompt_preview"]
        .as_str()
        .unwrap()
        .contains("unsent user correction"));
    assert!(preview["context"]["sources"]
        .as_array()
        .unwrap()
        .iter()
        .any(|source| source["role"] == "user" && source["provider_id"] == "codex"));
    let artifact = fs::read_to_string(handoff.1["artifact_path"].as_str().unwrap()).unwrap();
    assert!(
        artifact.contains("Target context preview")
            && artifact.contains("keep public API")
            && artifact.contains("unsent user correction")
    );
    assert_eq!(
        executor.requests.lock().unwrap().len(),
        1,
        "source agent must not be called to prepare a handoff"
    );
    let hid = handoff.1["handoff"]["id"].as_str().unwrap();
    let accepted = api(
        &app,
        Method::POST,
        &format!("/sessions/{id}/handoff/{hid}/accept"),
        json!({}),
    )
    .await;
    assert_eq!(accepted.0, StatusCode::OK);
    assert_eq!(accepted.1["session"]["id"], id);
    assert_eq!(
        accepted.1["session"]["continuity"]["checkpoints"],
        first["continuity"]["checkpoints"]
    );
    assert_eq!(
        api(
            &app,
            Method::POST,
            &format!("/sessions/{id}/handoff/{hid}/accept"),
            json!({})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(executor.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn legacy_handoff_requirements_import_once_and_can_be_revoked_without_resurrection() {
    let (_, app, _data, project, executor) = environment();
    let (_, id) = setup_workspace_and_session(&app, &project).await;
    let handoff = api(
        &app,
        Method::POST,
        &format!("/sessions/{id}/handoff"),
        json!({"to_provider_id":"antigravity","user_constraints":["keep legacy behavior"]}),
    )
    .await
    .1;
    let hid = handoff["handoff"]["id"].as_str().unwrap();
    let key = format!("{hid}-0");
    let before = api(
        &app,
        Method::GET,
        &format!("/sessions/{id}/requirements"),
        json!(null),
    )
    .await
    .1;
    assert_eq!(
        before["continuity"]["requirements"][&key]["source_ref"],
        format!("handoff:{hid}")
    );
    assert_eq!(
        prompt(&app, &id, "codex", &format!("/forget {key}")).await["local_command"],
        true
    );
    prompt(&app, &id, "codex", "continue").await;
    let after = api(
        &app,
        Method::GET,
        &format!("/sessions/{id}/requirements"),
        json!(null),
    )
    .await
    .1;
    assert_eq!(after["continuity"]["requirements"][&key]["active"], false);
    assert_eq!(after["continuity"]["imported_handoffs"], json!([hid]));
    let requests = executor.requests.lock().unwrap();
    let objective = requests[0].prompt.split("</objective>").next().unwrap();
    assert!(objective.contains("revoked"));
    assert!(!objective.contains("legacy handoff"));
}

#[tokio::test]
async fn crash_between_lease_and_dispatch_recovers_even_if_session_was_completed() {
    let (state, app, data, project, executor) = environment();
    let (_, id) = setup_workspace_and_session(&app, &project).await;
    let turn_id = baize_core::EventId::new();
    {
        let store = state.store.lock().unwrap();
        let mut session = store
            .get_task_session(&TaskSessionId(id.clone()))
            .unwrap()
            .unwrap();
        session.status = TaskSessionStatus::Completed;
        session.continuity.active_turn = Some(baize_core::ExecutionAttempt {
            id: turn_id.clone(),
            provider_id: ProviderId("codex".into()),
            uncertain: false,
        });
        store.upsert_task_session(&session).unwrap();
    }
    let restarted = AppState::with_executor(
        BaizeConfig::default(),
        EventStore::open(data.path().join("baize.db")).unwrap(),
        executor.clone(),
    );
    let app = router(restarted);
    let value = api(&app, Method::GET, &format!("/sessions/{id}"), json!(null))
        .await
        .1;
    assert_eq!(
        value["session"]["continuity"]["active_turn"]["uncertain"],
        true
    );
    assert_eq!(
        prompt(&app, &id, "codex", &format!("/reconcile {}", turn_id.0)).await["local_command"],
        true
    );
    assert!(executor.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn legacy_unmatched_dispatch_does_not_become_safe_to_replay_on_upgrade() {
    let (state, app, data, project, executor) = environment();
    let (_, id) = setup_workspace_and_session(&app, &project).await;
    let mut started = baize_core::BaizeEvent::new(
        "session.agent.started",
        json!({"prompt":"possibly modified files"}),
    );
    started.session_id = Some(TaskSessionId(id.clone()));
    started.provider_id = Some(ProviderId("codex".into()));
    state.store.lock().unwrap().append_event(&started).unwrap();
    let restarted = AppState::with_executor(
        BaizeConfig::default(),
        EventStore::open(data.path().join("baize.db")).unwrap(),
        executor.clone(),
    );
    let app = router(restarted);
    let value = api(&app, Method::GET, &format!("/sessions/{id}"), json!(null))
        .await
        .1;
    assert_eq!(
        value["session"]["continuity"]["active_turn"]["id"],
        started.id.0
    );
    assert_eq!(
        value["session"]["continuity"]["active_turn"]["uncertain"],
        true
    );
    assert_eq!(
        api(
            &app,
            Method::POST,
            &format!("/sessions/{id}/prompt"),
            json!({"prompt":"do not duplicate"})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert!(executor.requests.lock().unwrap().is_empty());
}
