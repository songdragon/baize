use super::*;

#[tokio::test]
async fn a_changed_native_id_cannot_certify_history_that_only_the_old_id_received() {
    let (mut state, app, _data, project, _) = environment();
    let (_, id) = setup_workspace_and_session(&app, &project).await;
    let first = prompt(
        &app,
        &id,
        "codex",
        "early user requirement survives a native rollover",
    )
    .await;
    let requests = Arc::new(Mutex::new(vec![]));
    state.executor = Arc::new(RecordingAgentExecutor {
        requests: requests.clone(),
        result: AgentRunResult {
            provider_id: ProviderId("codex".into()),
            success: true,
            exit_code: Some(0),
            native_session_id: Some("replacement-native".into()),
            events: vec![],
            stderr: String::new(),
            error: None,
        },
    });
    let app = router(state);
    let second = prompt(&app, &id, "codex", "delta sent to the old native id").await;
    assert_eq!(second["turn_status"], "completed");
    assert_eq!(
        requests.lock().unwrap()[0].session_id.as_deref(),
        Some("native-codex")
    );
    assert!(!requests.lock().unwrap()[0]
        .prompt
        .contains("early user requirement"));
    assert_eq!(
        second["continuity"]["checkpoints"],
        first["continuity"]["checkpoints"]
    );
    let preview = api(
        &app,
        Method::POST,
        &format!("/sessions/{id}/context-preview"),
        json!({"provider_id":"codex","prompt":"restore missing coverage"}),
    )
    .await
    .1;
    assert_eq!(preview["context"]["strategy"], "FullContextPack");
    assert!(preview["prompt_preview"]
        .as_str()
        .unwrap()
        .contains("early user requirement"));
    assert!(preview["context"]["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning.as_str().unwrap().contains("unverified")));
    let third = prompt(&app, &id, "codex", "restore missing coverage").await;
    assert_eq!(
        requests.lock().unwrap()[1].session_id.as_deref(),
        Some("replacement-native")
    );
    assert!(requests.lock().unwrap()[1]
        .prompt
        .contains("early user requirement"));
    assert_eq!(
        third["continuity"]["checkpoints"]["codex"]["native_session_id"],
        "replacement-native"
    );
}

#[tokio::test]
async fn draft_handoff_constraints_do_not_apply_and_accepted_duplicates_do_not_revive() {
    let (_, app, _data, project, executor) = environment();
    let (_, id) = setup_workspace_and_session(&app, &project).await;
    let mut handoffs = vec![];
    for text in [
        "shared legacy requirement",
        " shared legacy requirement ",
        "shared legacy requirement",
    ] {
        let response = api(
            &app,
            Method::POST,
            &format!("/sessions/{id}/handoff"),
            json!({"to_provider_id":"antigravity","user_constraints":[text]}),
        )
        .await;
        assert_eq!(response.0, StatusCode::OK);
        handoffs.push(response.1["handoff"]["id"].as_str().unwrap().to_string());
    }
    let queried = api(
        &app,
        Method::GET,
        &format!("/sessions/{id}/requirements"),
        json!(null),
    )
    .await
    .1;
    assert!(queried["continuity"]["requirements"]
        .as_object()
        .unwrap()
        .is_empty());
    let preview = api(
        &app,
        Method::POST,
        &format!("/sessions/{id}/context-preview"),
        json!({"prompt":"drafts are not active instructions"}),
    )
    .await
    .1;
    let objective = preview["prompt_preview"]
        .as_str()
        .unwrap()
        .split("</objective>")
        .next()
        .unwrap();
    assert!(!objective.contains("shared legacy requirement"));
    for hid in &handoffs[..2] {
        assert_eq!(
            api(
                &app,
                Method::POST,
                &format!("/sessions/{id}/handoff/{hid}/accept"),
                json!({})
            )
            .await
            .0,
            StatusCode::OK
        );
    }
    let queried = api(
        &app,
        Method::GET,
        &format!("/sessions/{id}/requirements"),
        json!(null),
    )
    .await
    .1;
    let requirements = queried["continuity"]["requirements"].as_object().unwrap();
    assert_eq!(requirements.len(), 1);
    let key = requirements.keys().next().unwrap();
    assert_eq!(
        prompt(&app, &id, "codex", &format!("/forget {key}")).await["local_command"],
        true
    );
    assert_eq!(
        api(
            &app,
            Method::POST,
            &format!("/sessions/{id}/handoff/{}/accept", handoffs[2]),
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    let queried = api(
        &app,
        Method::GET,
        &format!("/sessions/{id}/requirements"),
        json!(null),
    )
    .await
    .1;
    let requirements = queried["continuity"]["requirements"].as_object().unwrap();
    assert_eq!(requirements.len(), 1);
    assert_eq!(requirements[key]["active"], false);
    assert!(executor.requests.lock().unwrap().is_empty());
}
