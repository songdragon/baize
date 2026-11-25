use super::*;

#[test]
fn credential_field_names_in_user_history_are_delivered_without_false_redaction() {
    let events = vec![event(
        "session.agent.started",
        "codex",
        serde_json::json!({"prompt":"修复 refresh_token 过期后的重试，并验证 access_token 和 api_key 字段"}),
    )];
    let result = pack(&session_with_native_ids(BTreeMap::new()), &events, 16_000);
    assert!(result.prompt.contains("修复 refresh_token 过期后的重试"));
    assert!(result.sources[0].included && !result.sources[0].truncated);
    assert!(result.omitted_event_ids.is_empty());
}

#[test]
fn actual_secret_redaction_is_visible_and_never_certified_as_complete_delivery() {
    for role in ["session.agent.started", "session.agent.output"] {
        let payload = if role.ends_with("started") {
            serde_json::json!({"prompt":"API_KEY=private-secret\nsafe remainder"})
        } else {
            serde_json::json!({"text":"API_KEY=private-secret\nsafe remainder"})
        };
        let events = vec![event(role, "codex", payload)];
        let result = pack(&session_with_native_ids(BTreeMap::new()), &events, 16_000);
        assert!(!result.prompt.contains("private-secret"));
        assert!(result.prompt.contains("safe remainder"));
        assert!(result.sources[0].included && result.sources[0].truncated);
        assert_eq!(result.omitted_event_ids, vec![events[0].id.clone()]);
        assert!(result
            .warnings
            .iter()
            .any(|warning| warning.contains("redact")));
    }
    assert!(!prompt_preview("{\"api_key\": \"private-secret\"}").contains("private-secret"));
}

#[test]
fn large_workspace_file_lists_are_explicit_excerpts_and_do_not_block_every_turn() {
    let session = session_with_native_ids(BTreeMap::new());
    let profile = profile("codex", true, 4_000);
    let project = project();
    let status = WorkspaceStatus {
        root: project.root.clone(),
        git_root: None,
        branch: Some("work".into()),
        head: Some("abc".into()),
        dirty: true,
        changed_files: (0..2_000)
            .map(|i| format!("src/{i:04}_{}.rs", "a".repeat(40)))
            .collect(),
    };
    let result = build_context_pack(ContextPackInput {
        session: &session,
        target_provider: &ProviderId("codex".into()),
        current_prompt: "inspect current workspace",
        profile: &profile,
        workspace_status: Some(&status),
        project: &project,
        events: &[],
        route_decisions: &[],
        handoffs: &[],
        diff_hunks: &[],
    });
    assert!(!result.blocked);
    assert!(result.prompt.contains("+1968 more"));
    assert!(result
        .truncated_sections
        .iter()
        .any(|name| name == "workspace.changed_files"));
    assert!(result
        .warnings
        .iter()
        .any(|warning| warning.contains("changed-file")));
}

#[test]
fn revoked_requirement_bodies_stop_consuming_mandatory_context_but_active_ones_still_block() {
    let mut session = session_with_native_ids(BTreeMap::new());
    session.continuity.requirements.insert(
        "old".into(),
        baize_core::SessionRequirement {
            text: "withdrawn body ".repeat(4_000),
            active: false,
            revision: 2,
            source_ref: "evt_original".into(),
        },
    );
    let result = pack(&session, &[], 2_000);
    assert!(!result.blocked);
    assert!(result
        .prompt
        .contains("old [r2 revoked source=evt_original]"));
    assert!(!result.prompt.contains("withdrawn body"));
    session
        .continuity
        .requirements
        .get_mut("old")
        .unwrap()
        .active = true;
    let result = pack(&session, &[], 2_000);
    assert!(result.blocked);
    assert!(result.prompt.contains("withdrawn body"));
}
