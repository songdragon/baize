use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

struct ContinuityExecutor {
    requests: Mutex<Vec<AgentPromptRequest>>,
    fail_next: AtomicBool,
}

impl AgentExecutor for ContinuityExecutor {
    fn run_prompt(
        &self,
        request: AgentPromptRequest,
        _: Option<&mut baize_adapters::AgentEventSink<'_>>,
    ) -> Result<AgentRunResult> {
        let provider = request.provider_id.clone();
        self.requests.lock().unwrap().push(request);
        let success = !self.fail_next.swap(false, Ordering::SeqCst);
        Ok(AgentRunResult {
            provider_id: provider.clone(),
            success,
            exit_code: Some(if success { 0 } else { 1 }),
            native_session_id: Some(format!("native-{}", provider.0)),
            events: vec![AgentExecutionEvent {
                kind: AgentExecutionEventKind::Output,
                text: Some(format!(
                    "{} work already done; inspect files before repeating",
                    provider.0
                )),
                raw: None,
                stream: None,
            }],
            stderr: if success {
                "".into()
            } else {
                "native reference unavailable".into()
            },
            error: None,
        })
    }
}

fn environment() -> (
    AppState,
    Router,
    tempfile::TempDir,
    tempfile::TempDir,
    Arc<ContinuityExecutor>,
) {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let executor = Arc::new(ContinuityExecutor {
        requests: Mutex::new(vec![]),
        fail_next: AtomicBool::new(false),
    });
    let state = AppState::with_executor(
        BaizeConfig::default(),
        EventStore::open(data.path().join("baize.db")).unwrap(),
        executor.clone(),
    );
    (state.clone(), router(state), data, project, executor)
}

async fn api(
    app: &Router,
    method: Method,
    path: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    json_response_with_status(
        app.clone(),
        Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
    )
    .await
}

async fn prompt(app: &Router, id: &str, provider: &str, text: &str) -> serde_json::Value {
    api(
        app,
        Method::POST,
        &format!("/sessions/{id}/prompt"),
        json!({"provider_id":provider,"prompt":text}),
    )
    .await
    .1
}

#[tokio::test]
async fn requirements_create_replace_revoke_and_api_errors_never_call_an_agent() {
    let (_, app, _data, project, executor) = environment();
    let (_, id) = setup_workspace_and_session(&app, &project).await;
    for text in [
        "/remember language English only",
        "/remember language 中文回答",
        "/forget language",
        "/remember safety do not delete files",
    ] {
        assert_eq!(
            prompt(&app, &id, "antigravity", text).await["local_command"],
            true
        );
    }
    let (status, value) = api(
        &app,
        Method::GET,
        &format!("/sessions/{id}/requirements"),
        json!(null),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        value["continuity"]["requirements"]["language"]["active"],
        false
    );
    assert_eq!(
        value["continuity"]["requirements"]["language"]["revision"],
        3
    );
    assert_eq!(
        value["continuity"]["requirements"]["safety"]["text"],
        "do not delete files"
    );
    assert!(executor.requests.lock().unwrap().is_empty());
    let events = api(
        &app,
        Method::GET,
        &format!("/sessions/{id}/events"),
        json!(null),
    )
    .await
    .1;
    let source = value["continuity"]["requirements"]["safety"]["source_ref"]
        .as_str()
        .unwrap();
    assert!(events["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|event| event["id"] == source
            && event["payload"]["prompt"] == "/remember safety do not delete files"));
    let bad = api(
        &app,
        Method::POST,
        &format!("/sessions/{id}/requirements"),
        json!({"action":"remember","key":"bad key","text":"invalid"}),
    )
    .await;
    assert_eq!(bad.0, StatusCode::BAD_REQUEST);
    assert_eq!(
        api(
            &app,
            Method::GET,
            "/sessions/missing/requirements",
            json!(null)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        api(
            &app,
            Method::POST,
            &format!("/sessions/{id}/requirements"),
            json!({"action":"forget","key":"missing"})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn a_b_a_survives_long_dialogue_and_restart_with_confirmed_receipts() {
    let (state, app, data, project, executor) = environment();
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.name", "Baize test"],
        vec!["config", "user.email", "baize@example.invalid"],
    ] {
        assert!(Command::new("git")
            .args(args)
            .current_dir(project.path())
            .status()
            .unwrap()
            .success());
    }
    fs::write(project.path().join("base.txt"), "base").unwrap();
    assert!(Command::new("git")
        .args(["add", "."])
        .current_dir(project.path())
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["commit", "-qm", "base"])
        .current_dir(project.path())
        .status()
        .unwrap()
        .success());
    let (_, id) = setup_workspace_and_session(&app, &project).await;
    prompt(&app, &id, "codex", "/remember safety preserve public API").await;
    let first = prompt(&app, &id, "codex", "first task").await;
    assert_eq!(first["turn_status"], "completed");
    let a_receipt = first["continuity"]["checkpoints"]["codex"].clone();
    prompt(
        &app,
        &id,
        "antigravity",
        "/remember language use Chinese responses",
    )
    .await;
    for i in 0..14 {
        assert_eq!(
            prompt(
                &app,
                &id,
                "antigravity",
                &format!("B correction-{i}; keep API stable")
            )
            .await["turn_status"],
            "completed"
        );
    }
    fs::write(project.path().join("external.txt"), "external modification").unwrap();
    let stored = state
        .store
        .lock()
        .unwrap()
        .get_task_session(&TaskSessionId(id.clone()))
        .unwrap()
        .unwrap();
    assert_eq!(
        stored.continuity.checkpoints["codex"].native_session_id,
        "native-codex"
    );
    let restarted = AppState::with_executor(
        BaizeConfig::default(),
        EventStore::open(data.path().join("baize.db")).unwrap(),
        executor.clone(),
    );
    let app = router(restarted);
    assert_eq!(
        api(&app, Method::GET, &format!("/sessions/{id}"), json!(null))
            .await
            .1["session"]["status"],
        "Running"
    );
    let before = api(
        &app,
        Method::GET,
        &format!("/sessions/{id}/requirements"),
        json!(null),
    )
    .await
    .1;
    assert_eq!(before["continuity"]["checkpoints"]["codex"], a_receipt);
    let preview = api(
        &app,
        Method::POST,
        &format!("/sessions/{id}/context-preview"),
        json!({"provider_id":"codex","prompt":"check latest changes"}),
    )
    .await;
    assert_eq!(preview.0, StatusCode::OK);
    assert_eq!(preview.1["preview_only"], true);
    assert_eq!(preview.1["context"]["strategy"], "CrossProviderDelta");
    let text = preview.1["prompt_preview"].as_str().unwrap();
    assert!(text.contains("correction-0") && text.contains("correction-13"));
    assert!(
        text.contains("use Chinese responses")
            && text.contains("preserve public API")
            && text.contains("external.txt")
            && text.contains("HEAD:")
    );
    assert!(!text.contains("user: first task"));
    let after = api(
        &app,
        Method::GET,
        &format!("/sessions/{id}/requirements"),
        json!(null),
    )
    .await
    .1;
    assert_eq!(before, after, "preview must not acknowledge delivery");
    let back = prompt(&app, &id, "codex", "check latest changes").await;
    assert_eq!(back["turn_status"], "completed");
    assert_ne!(
        back["continuity"]["checkpoints"]["codex"]["through_event_id"],
        a_receipt["through_event_id"]
    );
    let requests = executor.requests.lock().unwrap();
    assert_eq!(requests.len(), 16);
    assert_eq!(
        requests.last().unwrap().session_id.as_deref(),
        Some("native-codex")
    );
    assert!(requests.last().unwrap().prompt.contains("correction-0"));
}

#[tokio::test]
async fn failed_target_does_not_advance_or_retry_and_native_reset_is_explicit() {
    let (_, app, _data, project, executor) = environment();
    let (_, id) = setup_workspace_and_session(&app, &project).await;
    let first = prompt(&app, &id, "codex", "first").await;
    executor.fail_next.store(true, Ordering::SeqCst);
    let failed = prompt(&app, &id, "codex", "failed turn").await;
    assert_eq!(failed["turn_status"], "failed");
    assert_eq!(
        failed["continuity"]["checkpoints"],
        first["continuity"]["checkpoints"]
    );
    assert!(failed["recovery_hint"]
        .as_str()
        .unwrap()
        .contains("No automatic retry"));
    assert_eq!(executor.requests.lock().unwrap().len(), 2);
    assert_eq!(
        prompt(&app, &id, "codex", "/reset-provider codex").await["local_command"],
        true
    );
    assert_eq!(executor.requests.lock().unwrap().len(), 2);
    let next = prompt(&app, &id, "codex", "continue after inspecting changes").await;
    assert_eq!(next["context"]["strategy"], "FullContextPack");
    let requests = executor.requests.lock().unwrap();
    assert!(requests.last().unwrap().session_id.is_none());
    assert!(requests.last().unwrap().prompt.contains("failed turn"));
}

#[tokio::test]
async fn required_context_budget_failure_never_dispatches_or_advances_a_receipt() {
    let (mut state, _, _data, project, executor) = environment();
    state
        .config
        .context
        .provider_token_budgets
        .insert("codex".into(), 30);
    let app = router(state);
    let (_, id) = setup_workspace_and_session(&app, &project).await;
    prompt(&app, &id, "codex", "/remember safety keep every file").await;
    let result = api(
        &app,
        Method::POST,
        &format!("/sessions/{id}/prompt"),
        json!({"prompt":"continue"}),
    )
    .await;
    assert_eq!(result.0, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(result.1["not_submitted"], true);
    assert_eq!(result.1["context"]["blocked"], true);
    assert!(executor.requests.lock().unwrap().is_empty());
    let value = api(
        &app,
        Method::GET,
        &format!("/sessions/{id}/requirements"),
        json!(null),
    )
    .await
    .1;
    assert!(value["continuity"]["active_turn"].is_null());
    assert_eq!(value["continuity"]["checkpoints"], json!({}));
}

#[tokio::test]
async fn restarting_an_interrupted_attempt_requires_exact_explicit_reconciliation() {
    let (state, app, data, project, executor) = environment();
    let (_, id) = setup_workspace_and_session(&app, &project).await;
    let mut session = state
        .store
        .lock()
        .unwrap()
        .get_task_session(&TaskSessionId(id.clone()))
        .unwrap()
        .unwrap();
    let mut turn =
        crate::continuity::TurnGuard::begin(&state, &mut session, &ProviderId("codex".into()))
            .unwrap();
    turn.dispatched = true;
    let turn_id = turn.id.0.clone();
    drop(turn);
    let restarted = AppState::with_executor(
        BaizeConfig::default(),
        EventStore::open(data.path().join("baize.db")).unwrap(),
        executor.clone(),
    );
    let app = router(restarted);
    let rejected = api(
        &app,
        Method::POST,
        &format!("/sessions/{id}/prompt"),
        json!({"prompt":"do not duplicate"}),
    )
    .await;
    assert_eq!(rejected.0, StatusCode::CONFLICT);
    assert_eq!(
        prompt(&app, &id, "codex", "/reconcile wrong-id").await["not_submitted"],
        true
    );
    let reconciled = prompt(&app, &id, "codex", &format!("/reconcile {turn_id}")).await;
    assert_eq!(reconciled["local_command"], true);
    assert!(executor.requests.lock().unwrap().is_empty());
    assert_eq!(
        prompt(&app, &id, "antigravity", "continue after inspection").await["turn_status"],
        "completed"
    );
}

#[tokio::test]
async fn concurrent_clients_cannot_dispatch_a_second_turn_or_mutate_running_requirements() {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let emitted = Arc::new(AtomicBool::new(false));
    let completed = Arc::new(AtomicBool::new(false));
    let state = AppState::with_executor(
        BaizeConfig::default(),
        EventStore::open(data.path().join("baize.db")).unwrap(),
        Arc::new(SlowStreamingAgentExecutor {
            emitted: emitted.clone(),
            completed,
        }),
    );
    let app = router(state);
    let (_, id) = setup_workspace_and_session(&app, &project).await;
    let first_app = app.clone();
    let first_id = id.clone();
    let first =
        tokio::spawn(async move { prompt(&first_app, &first_id, "codex", "slow first").await });
    while !emitted.load(Ordering::SeqCst) {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        api(
            &app,
            Method::POST,
            &format!("/sessions/{id}/prompt"),
            json!({"prompt":"duplicate"})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        prompt(
            &app,
            &id,
            "codex",
            "/remember unsafe mutate during execution"
        )
        .await["not_submitted"],
        true
    );
    assert_eq!(first.await.unwrap()["turn_status"], "completed");
}

#[path = "handoff_continuity_tests.rs"]
mod handoff;

#[path = "continuity_review_tests.rs"]
mod review;
