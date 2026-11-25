use super::*;

#[tokio::test]
async fn an_unconfirmed_turn_stop_requires_exact_review_before_handoff_in_the_same_conversation() {
    let (mut state, app, _data, project, _) = environment();
    let (_, id) = setup_workspace_and_session(&app, &project).await;
    let calls = Arc::new(AtomicUsize::new(0));
    state.executor = Arc::new(NativeExecutor {
        executed: Arc::new(AtomicBool::new(false)),
        calls: calls.clone(),
        confirmed_stop: false,
    });
    let app = router(state);
    let request_app = app.clone();
    let request_id = id.clone();
    let work = tokio::spawn(async move {
        api(
            &request_app,
            Method::POST,
            &format!("/sessions/{request_id}/prompt"),
            json!({"prompt":"unconfirmed stop needs review"}),
        )
        .await
    });
    waiting(&app, &id).await;
    api(
        &app,
        Method::POST,
        &format!("/sessions/{id}/execution/cancel"),
        json!({}),
    )
    .await;
    let outcome = work.await.unwrap();
    assert_eq!(outcome.0, StatusCode::CONFLICT);
    assert_eq!(outcome.1["session_status"], "Running");
    assert_eq!(
        outcome.1["continuity"]["last_stop"]["status"],
        "unconfirmed"
    );
    let handoff = api(
        &app,
        Method::POST,
        &format!("/sessions/{id}/handoff"),
        json!({"to_provider_id":"antigravity"}),
    )
    .await
    .1;
    let accept = format!(
        "/sessions/{id}/handoff/{}/accept",
        handoff["handoff"]["id"].as_str().unwrap()
    );
    assert_eq!(
        api(&app, Method::POST, &accept, json!({})).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        prompt(&app, &id, "codex", "/reconcile evt_wrong").await["not_submitted"],
        true
    );
    let attempt = outcome.1["continuity"]["active_turn"]["id"]
        .as_str()
        .unwrap();
    assert_eq!(
        prompt(&app, &id, "codex", &format!("/reconcile {attempt}")).await["local_command"],
        true
    );
    assert_eq!(
        api(&app, Method::POST, &accept, json!({})).await.0,
        StatusCode::OK
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_persisted_stop_request_blocks_approval_before_the_worker_receives_its_signal() {
    let (mut state, app, _data, project, _) = environment();
    let (_, id) = setup_workspace_and_session(&app, &project).await;
    let executed = Arc::new(AtomicBool::new(false));
    state.executor = Arc::new(NativeExecutor {
        executed: executed.clone(),
        calls: Arc::new(AtomicUsize::new(0)),
        confirmed_stop: true,
    });
    let app = router(state.clone());
    let request_app = app.clone();
    let request_id = id.clone();
    let work = tokio::spawn(async move {
        api(
            &request_app,
            Method::POST,
            &format!("/sessions/{request_id}/prompt"),
            json!({"prompt":"stop approval race"}),
        )
        .await
    });
    let permission = waiting(&app, &id).await;
    {
        let store = state.store.lock().unwrap();
        let mut session = store
            .get_task_session(&TaskSessionId(id.clone()))
            .unwrap()
            .unwrap();
        let attempt = session.continuity.active_turn.as_ref().unwrap();
        assert!(!state
            .executions
            .get(&attempt.id)
            .unwrap()
            .unwrap()
            .is_canceled());
        session.continuity.last_stop = Some(baize_core::ExecutionStop {
            attempt_id: attempt.id.clone(),
            provider_id: attempt.provider_id.clone(),
            status: baize_core::ExecutionStopStatus::Stopping,
            detail: "stop committed before control delivery".into(),
        });
        store.upsert_task_session(&session).unwrap();
    }
    let decision = api(
        &app,
        Method::POST,
        &format!(
            "/permissions/{}/approve",
            permission["id"].as_str().unwrap()
        ),
        json!({}),
    )
    .await;
    api(
        &app,
        Method::POST,
        &format!("/sessions/{id}/execution/cancel"),
        json!({}),
    )
    .await;
    let _ = work.await.unwrap();
    assert_eq!(decision.0, StatusCode::CONFLICT);
    assert!(!executed.load(Ordering::SeqCst));
}

#[tokio::test]
async fn restart_expires_native_callbacks_after_the_first_page_and_does_not_claim_a_stop() {
    let (state, app, data, project, executor) = environment();
    let (_, id) = setup_workspace_and_session(&app, &project).await;
    let attempt = baize_core::EventId::new();
    let mut permission = baize_core::PermissionRequest {
        id: baize_core::PermissionId::new(),
        workspace_id: None,
        session_id: Some(TaskSessionId(id.clone())),
        command: "historical record".into(),
        reason: "manual record".into(),
        risk: Default::default(),
        status: baize_core::PermissionStatus::Pending,
        created_at: chrono::Utc::now(),
        resolved_at: None,
        native: None,
    };
    let native_id;
    {
        let store = state.store.lock().unwrap();
        for _ in 0..120 {
            permission.id = baize_core::PermissionId::new();
            store.upsert_permission(&permission).unwrap();
        }
        permission.id = baize_core::PermissionId::new();
        native_id = permission.id.clone();
        permission.native = Some(baize_core::NativePermission {
            attempt_id: attempt.clone(),
            provider_id: ProviderId("codex".into()),
            request_id: "101".into(),
            method: "item/commandExecution/requestApproval".into(),
            params: json!({"command":"native operation"}),
        });
        store.upsert_permission(&permission).unwrap();
        let mut session = store
            .get_task_session(&TaskSessionId(id.clone()))
            .unwrap()
            .unwrap();
        session.continuity.active_turn = Some(baize_core::ExecutionAttempt {
            id: attempt.clone(),
            provider_id: ProviderId("codex".into()),
            uncertain: false,
        });
        session.continuity.last_stop = Some(baize_core::ExecutionStop {
            attempt_id: attempt,
            provider_id: ProviderId("codex".into()),
            status: baize_core::ExecutionStopStatus::Stopping,
            detail: "waiting for stop".into(),
        });
        store.upsert_task_session(&session).unwrap();
    }
    let reopened = AppState::with_executor(
        BaizeConfig::default(),
        EventStore::open(data.path().join("baize.db")).unwrap(),
        executor,
    );
    let app = router(reopened.clone());
    let stored = api(
        &app,
        Method::GET,
        &format!("/permissions/{}", native_id.0),
        json!(null),
    )
    .await
    .1;
    assert_eq!(stored["permission"]["status"], "Expired");
    assert_eq!(
        api(
            &app,
            Method::POST,
            &format!("/permissions/{}/approve", native_id.0),
            json!({})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let session = api(&app, Method::GET, &format!("/sessions/{id}"), json!(null))
        .await
        .1;
    assert_eq!(
        session["session"]["continuity"]["last_stop"]["status"],
        "unconfirmed"
    );
    assert_eq!(
        session["session"]["continuity"]["active_turn"]["uncertain"],
        true
    );
    let manual = reopened
        .store
        .lock()
        .unwrap()
        .list_permissions(Some(120), None)
        .unwrap();
    assert_eq!(manual.len(), 120);
    assert!(manual
        .iter()
        .all(|p| matches!(p.status, baize_core::PermissionStatus::Pending)));
}
