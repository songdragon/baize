use super::*;

#[tokio::test]
async fn legacy_discarded_or_uncertain_results_are_not_terminal_evidence() {
    for (kind, explicit_marker) in [
        ("session.agent.ignored", false),
        ("session.agent.failed", true),
        ("session.agent.failed", false),
    ] {
        let (state, app, data, project, executor) = environment();
        let (_, id) = setup_workspace_and_session(&app, &project).await;
        let mut started = baize_core::BaizeEvent::new(
            "session.agent.started",
            json!({"prompt":"legacy dispatched work"}),
        );
        started.session_id = Some(TaskSessionId(id.clone()));
        started.provider_id = Some(ProviderId("codex".into()));
        let mut terminal =
            baize_core::BaizeEvent::new(kind, json!({"reason":"legacy canceled result"}));
        terminal.session_id = started.session_id.clone();
        terminal.provider_id = started.provider_id.clone();
        if kind == "session.agent.failed" {
            terminal.payload = json!({"exit_code":1,"provider_error":{"kind":"Timeout"}});
            if explicit_marker {
                terminal.payload["active_turn"] =
                    json!({"id":started.id,"provider_id":"codex","uncertain":true});
            }
        }
        {
            let store = state.store.lock().unwrap();
            let mut session = store
                .get_task_session(&TaskSessionId(id.clone()))
                .unwrap()
                .unwrap();
            session.status = TaskSessionStatus::Canceled;
            store.upsert_task_session(&session).unwrap();
            store.append_event(&started).unwrap();
            store.append_event(&terminal).unwrap();
        }
        let reopened = AppState::with_executor(
            BaizeConfig::default(),
            EventStore::open(data.path().join("baize.db")).unwrap(),
            executor,
        );
        let session = reopened
            .store
            .lock()
            .unwrap()
            .get_task_session(&TaskSessionId(id))
            .unwrap()
            .unwrap();
        assert!(
            session
                .continuity
                .active_turn
                .as_ref()
                .is_some_and(|turn| turn.uncertain),
            "{kind} must not erase uncertainty without terminal evidence"
        );
    }
}

#[tokio::test]
async fn pre_dispatch_context_failure_is_explicitly_not_submitted() {
    let (_, app, _data, _project, executor) = environment();
    let created = api(&app,Method::POST,"/sessions",json!({"workspace_id":"ws_missing","objective":"unavailable workspace","provider_id":"codex"})).await.1;
    let id = created["session"]["id"].as_str().unwrap();
    let failed = api(
        &app,
        Method::POST,
        &format!("/sessions/{id}/prompt"),
        json!({"prompt":"keep this unsent request"}),
    )
    .await;
    assert_eq!(failed.0, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(failed.1["not_submitted"], true);
    assert!(executor.requests.lock().unwrap().is_empty());
    let session = api(&app, Method::GET, &format!("/sessions/{id}"), json!(null))
        .await
        .1;
    assert!(session["session"]["continuity"]["active_turn"].is_null());
}

#[tokio::test]
async fn requirements_query_is_read_only_even_with_an_active_turn() {
    for running in [false, true] {
        let (state, app, _data, project, executor) = environment();
        let (_, id) = setup_workspace_and_session(&app, &project).await;
        prompt(&app, &id, "codex", "/remember api preserve API").await;
        let mut current = state
            .store
            .lock()
            .unwrap()
            .get_task_session(&TaskSessionId(id.clone()))
            .unwrap()
            .unwrap();
        let _turn = if running {
            Some(
                crate::continuity::TurnGuard::begin(
                    &state,
                    &mut current,
                    &ProviderId("codex".into()),
                )
                .unwrap(),
            )
        } else {
            None
        };
        let before = api(&app, Method::GET, &format!("/sessions/{id}"), json!(null))
            .await
            .1;
        let events_before = api(
            &app,
            Method::GET,
            &format!("/sessions/{id}/events"),
            json!(null),
        )
        .await
        .1;
        let queried = api(
            &app,
            Method::POST,
            &format!("/sessions/{id}/prompt"),
            json!({"prompt":"/requirements"}),
        )
        .await;
        assert_eq!(queried.0, StatusCode::OK);
        assert!(queried.1["message"]
            .as_str()
            .unwrap()
            .contains("preserve API"));
        assert_eq!(
            api(&app, Method::GET, &format!("/sessions/{id}"), json!(null))
                .await
                .1,
            before
        );
        assert_eq!(
            api(
                &app,
                Method::GET,
                &format!("/sessions/{id}/events"),
                json!(null)
            )
            .await
            .1,
            events_before
        );
        assert_eq!(queried.1["read_only"], true);
        assert!(executor.requests.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn an_explicit_target_can_start_a_legacy_session_without_an_active_provider() {
    let (state, app, _data, project, executor) = environment();
    let (_, id) = setup_workspace_and_session(&app, &project).await;
    {
        let store = state.store.lock().unwrap();
        let mut session = store
            .get_task_session(&TaskSessionId(id.clone()))
            .unwrap()
            .unwrap();
        session.active_provider_id = None;
        store.upsert_task_session(&session).unwrap();
    }
    let result = api(
        &app,
        Method::POST,
        &format!("/sessions/{id}/prompt"),
        json!({"provider_id":"antigravity","prompt":"explicit first executor"}),
    )
    .await;
    assert_eq!(result.0, StatusCode::OK);
    assert_eq!(
        executor.requests.lock().unwrap()[0].provider_id.0,
        "antigravity"
    );
}

#[tokio::test]
async fn entry_mismatch_is_blocked_in_preview_as_well_as_submission() {
    let (state, app, _data, project, _executor) = environment();
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
            .execution_identity = "different entry".into();
        store.upsert_task_session(&session).unwrap();
    }
    let preview = api(
        &app,
        Method::POST,
        &format!("/sessions/{id}/context-preview"),
        json!({"provider_id":"codex","prompt":"next"}),
    )
    .await
    .1;
    assert_eq!(preview["context"]["blocked"], true);
    assert!(preview["context"]["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning.as_str().unwrap().contains("/reset-provider codex")));
}

struct CancelBeforeReturn {
    canceller: Router,
    session_id: String,
    result: Option<AgentRunResult>,
}

impl AgentExecutor for CancelBeforeReturn {
    fn run_prompt(
        &self,
        _: AgentPromptRequest,
        _: Option<&mut baize_adapters::AgentEventSink<'_>>,
    ) -> Result<AgentRunResult> {
        let (status, _) = tokio::runtime::Handle::current().block_on(api(
            &self.canceller,
            Method::POST,
            &format!("/sessions/{}/cancel", self.session_id),
            json!({}),
        ));
        assert_eq!(status, StatusCode::OK);
        self.result
            .clone()
            .ok_or_else(|| anyhow::anyhow!("worker result lost after cancellation"))
    }
}

#[tokio::test]
async fn cancellation_does_not_erase_an_unconfirmed_timeout_result() {
    let (mut state, app, data, project, _executor) = environment();
    let (_, id) = setup_workspace_and_session(&app, &project).await;
    let first = prompt(&app, &id, "codex", "confirmed work").await;
    state.executor = Arc::new(CancelBeforeReturn {
        canceller: app.clone(),
        session_id: id.clone(),
        result: Some(AgentRunResult {
            provider_id: ProviderId("codex".into()),
            success: false,
            exit_code: None,
            native_session_id: None,
            events: vec![],
            stderr: "timed out".into(),
            error: Some(AgentErrorDetail {
                kind: AgentErrorKind::Timeout,
                message: "execution unconfirmed".into(),
                source: AgentErrorSource::Runtime,
            }),
        }),
    });
    let app = router(state.clone());
    let canceled = prompt(&app, &id, "codex", "possibly still executing").await;
    assert_eq!(canceled["session_status"], "Canceled");
    let latest = api(&app, Method::GET, &format!("/sessions/{id}"), json!(null))
        .await
        .1;
    assert_eq!(
        latest["session"]["continuity"]["active_turn"]["uncertain"],
        true
    );
    assert_eq!(
        latest["session"]["continuity"]["checkpoints"],
        first["continuity"]["checkpoints"]
    );
    let recovered = AppState::with_executor(
        BaizeConfig::default(),
        EventStore::open(data.path().join("baize.db")).unwrap(),
        state.executor.clone(),
    );
    assert!(
        recovered
            .store
            .lock()
            .unwrap()
            .get_task_session(&TaskSessionId(id.clone()))
            .unwrap()
            .unwrap()
            .continuity
            .active_turn
            .unwrap()
            .uncertain
    );
    let app = router(recovered);
    let turn_id = latest["session"]["continuity"]["active_turn"]["id"]
        .as_str()
        .unwrap();
    assert_eq!(
        prompt(&app, &id, "codex", &format!("/reconcile {turn_id}")).await["local_command"],
        true
    );
    let reconciled = api(&app, Method::GET, &format!("/sessions/{id}"), json!(null))
        .await
        .1;
    assert_eq!(reconciled["session"]["status"], "Canceled");
    assert!(reconciled["session"]["continuity"]["active_turn"].is_null());
    let reopened = AppState::with_executor(
        BaizeConfig::default(),
        EventStore::open(data.path().join("baize.db")).unwrap(),
        state.executor.clone(),
    );
    let row = reopened
        .store
        .lock()
        .unwrap()
        .get_task_session(&TaskSessionId(id))
        .unwrap()
        .unwrap();
    assert!(row.continuity.active_turn.is_none());
}

#[tokio::test]
async fn a_known_result_losing_the_cancel_race_is_terminal_after_restart() {
    let (state, app, data, project, executor) = environment();
    let (_, id) = setup_workspace_and_session(&app, &project).await;
    let mut session = state
        .store
        .lock()
        .unwrap()
        .get_task_session(&TaskSessionId(id.clone()))
        .unwrap()
        .unwrap();
    let mut guard =
        crate::continuity::TurnGuard::begin(&state, &mut session, &ProviderId("codex".into()))
            .unwrap();
    let mut started =
        baize_core::BaizeEvent::new("session.agent.started", json!({"prompt":"known dispatch"}));
    started.id = guard.id.clone();
    started.session_id = Some(session.id.clone());
    started.provider_id = Some(ProviderId("codex".into()));
    state
        .store
        .lock()
        .unwrap()
        .save_session_event(&session, &started)
        .unwrap();
    guard.dispatched = true;
    api(
        &app,
        Method::POST,
        &format!("/sessions/{id}/cancel"),
        json!({}),
    )
    .await;
    let mut result = baize_core::BaizeEvent::new(
        "session.agent.completed",
        json!({"success":true,"exit_code":0}),
    );
    result.session_id = Some(session.id.clone());
    result.provider_id = Some(ProviderId("codex".into()));
    session.continuity.active_turn = None;
    assert!(!crate::continuity::save_turn(&state, &session, &result, &guard.id).unwrap());
    drop(guard);
    let reopened = AppState::with_executor(
        BaizeConfig::default(),
        EventStore::open(data.path().join("baize.db")).unwrap(),
        executor,
    );
    let row = reopened
        .store
        .lock()
        .unwrap()
        .get_task_session(&TaskSessionId(id))
        .unwrap()
        .unwrap();
    assert!(
        row.continuity.active_turn.is_none(),
        "A discarded but confirmed finished result must not resurrect as an unknown dispatch"
    );
}

#[tokio::test]
async fn cancellation_keeps_an_unknown_worker_error_protected() {
    let (mut state, app, _data, project, _executor) = environment();
    let (_, id) = setup_workspace_and_session(&app, &project).await;
    state.executor = Arc::new(CancelBeforeReturn {
        canceller: app,
        session_id: id.clone(),
        result: None,
    });
    let app = router(state);
    let response = prompt(&app, &id, "codex", "worker may still have effects").await;
    assert_eq!(response["session_status"], "Canceled");
    assert_eq!(response["turn_status"], "unconfirmed");
    assert_eq!(response["continuity"]["active_turn"]["uncertain"], true);
    assert_eq!(response["continuity"]["checkpoints"], json!({}));
}
