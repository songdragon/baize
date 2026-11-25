use super::*;
use baize_core::{
    ProjectId, ProjectKind, TaskSessionId, TaskSessionStatus, TrustLevel, VcsKind, WorkspaceId,
};
use chrono::Utc;
use std::collections::BTreeMap;
use std::path::PathBuf;

#[path = "zcode_tests.rs"]
mod zcode_review;

fn pack(session: &TaskSession, events: &[BaizeEvent], budget: u32) -> ContextPackBuildResult {
    build_context_pack(ContextPackInput {
        session,
        target_provider: &ProviderId("codex".into()),
        current_prompt: "continue safely",
        profile: &profile("codex", true, budget),
        workspace_status: None,
        project: &project(),
        events,
        route_decisions: &[],
        handoffs: &[],
        diff_hunks: &[],
    })
}

#[test]
fn missed_user_requests_are_not_limited_to_eight_messages_or_clipped() {
    let session = session_with_native_ids(BTreeMap::new());
    let events = (0..24)
        .map(|i| {
            event(
                "session.agent.started",
                "antigravity",
                serde_json::json!({"prompt":format!("correction-{i} {}", "中".repeat(80))}),
            )
        })
        .collect::<Vec<_>>();
    let result = pack(&session, &events, 24_000);
    assert!(!result.blocked);
    for i in 0..24 {
        assert!(result.prompt.contains(&format!("correction-{i} ")));
    }
    assert_eq!(result.sources.len(), 24);
    assert!(result
        .sources
        .iter()
        .all(|source| source.included && source.role == "user"));
}

#[test]
fn active_and_revoked_requirements_are_authoritative_and_never_silently_clipped() {
    let mut session = session_with_native_ids(BTreeMap::new());
    session.continuity.requirements.insert(
        "format".into(),
        baize_core::SessionRequirement {
            text: "new requirement".into(),
            active: true,
            revision: 3,
            source_ref: baize_core::EventId::new().0,
        },
    );
    session.continuity.requirements.insert(
        "old".into(),
        baize_core::SessionRequirement {
            text: "withdrawn requirement".into(),
            active: false,
            revision: 2,
            source_ref: baize_core::EventId::new().0,
        },
    );
    let events = vec![event(
        "session.agent.output",
        "antigravity",
        serde_json::json!({"text":"agent suggests delete tests"}),
    )];
    let result = pack(&session, &events, 16_000);
    let objective = result.prompt.split("</objective>").next().unwrap();
    assert!(objective.contains("format [r3 active"));
    assert!(objective.contains("old [r2 revoked"));
    assert!(!objective.contains("delete tests"));
    let blocked = pack(&session, &events, 10);
    assert!(blocked.blocked && blocked.prompt.contains("new requirement"));
}

#[test]
fn unverified_legacy_or_replaced_native_ids_never_skip_history() {
    let mut session = session_with_native_ids(BTreeMap::from([("codex".into(), "legacy".into())]));
    let events = vec![event(
        "session.agent.started",
        "codex",
        serde_json::json!({"prompt":"old valid context"}),
    )];
    let first = pack(&session, &events, 16_000);
    assert_eq!(first.strategy, ContextPackStrategy::FullContextPack);
    assert!(first.prompt.contains("old valid context"));
    assert!(first
        .warnings
        .iter()
        .any(|warning| warning.contains("unverified")));
    session.continuity.checkpoints.insert(
        "codex".into(),
        baize_core::ProviderCheckpoint {
            native_session_id: "different".into(),
            through_event_id: events[0].id.clone(),
            pending_event_ids: vec![],
            requirement_revision: 0,
            execution_identity: execution_identity(&profile("codex", true, 16_000), &project()),
        },
    );
    assert_eq!(
        pack(&session, &events, 16_000).strategy,
        ContextPackStrategy::FullContextPack
    );
}

#[test]
fn omitted_stream_item_is_reconstructed_before_receipt_filtering() {
    let mut session = session_with_native_ids(BTreeMap::from([("codex".into(), "native".into())]));
    let events = vec![
        event(
            "session.agent.output",
            "antigravity",
            serde_json::json!({"text":"complete ","stream":{"item_id":"item","mode":"delta"}}),
        ),
        event(
            "session.agent.output",
            "antigravity",
            serde_json::json!({"text":"answer","stream":{"item_id":"item","mode":"delta"}}),
        ),
        event(
            "session.agent.completed",
            "codex",
            serde_json::json!({"success":true}),
        ),
    ];
    session.continuity.checkpoints.insert(
        "codex".into(),
        baize_core::ProviderCheckpoint {
            native_session_id: "native".into(),
            through_event_id: events[2].id.clone(),
            pending_event_ids: vec![events[0].id.clone()],
            requirement_revision: 0,
            execution_identity: execution_identity(&profile("codex", true, 16_000), &project()),
        },
    );
    assert!(pack(&session, &events, 16_000)
        .prompt
        .contains("complete answer"));
}

#[test]
fn assistant_excerpts_remain_pending_and_do_not_displace_required_user_messages() {
    let session = session_with_native_ids(BTreeMap::new());
    let events = vec![
        event(
            "session.agent.output",
            "antigravity",
            serde_json::json!({"text":"x".repeat(10_000)}),
        ),
        event(
            "session.agent.started",
            "antigravity",
            serde_json::json!({"prompt":"retain this correction"}),
        ),
    ];
    let result = pack(&session, &events, 900);
    assert!(result.prompt.contains("retain this correction"));
    assert!(result.omitted_event_ids.contains(&events[0].id));
    assert!(result
        .sources
        .iter()
        .any(|source| source.role == "assistant" && source.truncated));
    assert!(result
        .warnings
        .iter()
        .any(|warning| warning.contains("excerpt")));
}

#[test]
fn context_joins_stream_fragments_replaces_snapshots_and_excludes_diagnostics() {
    let events = vec![
        event(
            "session.agent.started",
            "codex",
            serde_json::json!({"prompt":"keep API"}),
        ),
        event(
            "session.agent.output",
            "codex",
            serde_json::json!({"text":"hello ","stream":{"item_id":"m","mode":"delta"}}),
        ),
        event(
            "session.agent.diagnostic",
            "codex",
            serde_json::json!({"text":"private diagnostic"}),
        ),
        event(
            "session.agent.output",
            "codex",
            serde_json::json!({"text":"world","stream":{"item_id":"m","mode":"delta"}}),
        ),
        event(
            "session.agent.output",
            "codex",
            serde_json::json!({"text":"hello world","stream":{"item_id":"m","mode":"snapshot"}}),
        ),
        event(
            "session.agent.started",
            "opencode",
            serde_json::json!({"prompt":"continue"}),
        ),
        event(
            "session.agent.output",
            "opencode",
            serde_json::json!({"text":"second turn","stream":{"item_id":"m","mode":"snapshot"}}),
        ),
    ];
    let text = pack(&session_with_native_ids(BTreeMap::new()), &events, 24_000).prompt;
    assert_eq!(text.matches("hello world").count(), 1);
    assert!(text.contains("user: keep API") && text.contains("assistant: second turn"));
    assert!(!text.contains("private diagnostic"));
    assert!(crate::context_history::messages(&[]).is_empty());
}

#[test]
fn no_native_resume_includes_prior_history() {
    let session = session_with_native_ids(BTreeMap::new());
    let profile = profile("antigravity", true, 16_000);
    let events = vec![
        event(
            "session.agent.started",
            "codex",
            serde_json::json!({ "prompt": "hello" }),
        ),
        event(
            "session.agent.output",
            "codex",
            serde_json::json!({ "text": "ready to help" }),
        ),
    ];
    let project = project();

    let result = build_context_pack(ContextPackInput {
        session: &session,
        target_provider: &ProviderId("antigravity".to_string()),
        current_prompt: "continue",
        profile: &profile,
        workspace_status: None,
        project: &project,
        events: &events,
        route_decisions: &[],
        handoffs: &[],
        diff_hunks: &[],
    });

    assert_eq!(result.strategy, ContextPackStrategy::FullContextPack);
    assert!(result.prompt.contains("<recent_conversation>"));
    assert!(result.prompt.contains("user: hello"));
    assert!(result.prompt.contains("assistant: ready to help"));
}

#[test]
fn same_provider_native_resume_avoids_duplicate_history() {
    let mut ids = BTreeMap::new();
    ids.insert("codex".to_string(), "native_1".to_string());
    let mut session = session_with_native_ids(ids);
    let profile = profile("codex", true, 16_000);
    let events = vec![event(
        "session.agent.output",
        "codex",
        serde_json::json!({ "text": "old answer" }),
    )];
    let project = project();

    session.continuity.checkpoints.insert(
        "codex".into(),
        baize_core::ProviderCheckpoint {
            native_session_id: "native_1".into(),
            through_event_id: events[0].id.clone(),
            pending_event_ids: vec![],
            requirement_revision: 0,
            execution_identity: execution_identity(&profile, &project),
        },
    );

    let result = build_context_pack(ContextPackInput {
        session: &session,
        target_provider: &ProviderId("codex".to_string()),
        current_prompt: "next",
        profile: &profile,
        workspace_status: None,
        project: &project,
        events: &events,
        route_decisions: &[],
        handoffs: &[],
        diff_hunks: &[],
    });

    assert_eq!(result.strategy, ContextPackStrategy::CrossProviderDelta);
    assert!(result.prompt.contains("<current_user_request>\nnext"));
    assert!(!result.prompt.contains("old answer"));
}

#[test]
fn provider_switch_includes_cross_provider_delta() {
    let mut ids = BTreeMap::new();
    ids.insert("codex".to_string(), "native_1".to_string());
    let mut session = session_with_native_ids(ids);
    let profile = profile("codex", true, 16_000);
    let events = vec![
        event(
            "session.agent.output",
            "codex",
            serde_json::json!({ "text": "codex old" }),
        ),
        event(
            "session.agent.output",
            "opencode",
            serde_json::json!({ "text": "opencode changed foo.rs" }),
        ),
    ];
    let project = project();

    session.continuity.checkpoints.insert(
        "codex".into(),
        baize_core::ProviderCheckpoint {
            native_session_id: "native_1".into(),
            through_event_id: events[0].id.clone(),
            pending_event_ids: vec![],
            requirement_revision: 0,
            execution_identity: execution_identity(&profile, &project),
        },
    );

    let result = build_context_pack(ContextPackInput {
        session: &session,
        target_provider: &ProviderId("codex".to_string()),
        current_prompt: "what changed?",
        profile: &profile,
        workspace_status: None,
        project: &project,
        events: &events,
        route_decisions: &[],
        handoffs: &[],
        diff_hunks: &[],
    });

    assert_eq!(result.strategy, ContextPackStrategy::CrossProviderDelta);
    assert!(result.prompt.contains("opencode changed foo.rs"));
    assert!(!result.prompt.contains("codex old"));
}

#[test]
fn over_budget_truncates_low_priority_sections() {
    let session = session_with_native_ids(BTreeMap::new());
    let profile = profile("opencode", false, 120);
    let project = project();
    let hunks = vec![DiffHunk {
        file_path: "src/lib.rs".to_string(),
        old_start: 1,
        old_lines: 1,
        new_start: 1,
        new_lines: 20,
        header: "@@ -1,1 +1,20 @@".to_string(),
        lines: vec!["+".repeat(200); 20],
    }];

    let result = build_context_pack(ContextPackInput {
        session: &session,
        target_provider: &ProviderId("opencode".to_string()),
        current_prompt: "keep me",
        profile: &profile,
        workspace_status: None,
        project: &project,
        events: &[],
        route_decisions: &[],
        handoffs: &[],
        diff_hunks: &hunks,
    });

    assert!(result.prompt.contains("<current_user_request>\nkeep me"));
    assert!(result
        .truncated_sections
        .iter()
        .any(|section| section == "diff_hunks"));
}

#[test]
fn current_prompt_is_never_clipped_even_when_over_budget() {
    let session = session_with_native_ids(BTreeMap::new());
    let profile = profile("opencode", false, 80);
    let project = project();
    let current_prompt = "x".repeat(1_000);

    let result = build_context_pack(ContextPackInput {
        session: &session,
        target_provider: &ProviderId("opencode".to_string()),
        current_prompt: &current_prompt,
        profile: &profile,
        workspace_status: None,
        project: &project,
        events: &[],
        route_decisions: &[],
        handoffs: &[],
        diff_hunks: &[],
    });

    assert!(result.prompt.contains(&current_prompt));
}

#[test]
fn custom_token_estimator_is_used_for_budget_reporting() {
    struct OneTokenEstimator;

    impl TokenEstimator for OneTokenEstimator {
        fn estimate(&self, _text: &str) -> u32 {
            1
        }
    }

    let session = session_with_native_ids(BTreeMap::new());
    let profile = profile("opencode", false, 80);
    let project = project();

    let result = build_context_pack_with_estimator(
        ContextPackInput {
            session: &session,
            target_provider: &ProviderId("opencode".to_string()),
            current_prompt: "continue",
            profile: &profile,
            workspace_status: None,
            project: &project,
            events: &[],
            route_decisions: &[],
            handoffs: &[],
            diff_hunks: &[],
        },
        &OneTokenEstimator,
    );

    assert_eq!(result.estimated_tokens, 1);
}

#[test]
fn sensitive_context_lines_are_redacted() {
    let session = session_with_native_ids(BTreeMap::new());
    let profile = profile("opencode", false, 16_000);
    let project = project();
    let events = vec![event(
        "session.agent.output",
        "opencode",
        serde_json::json!({ "text": "TOKEN=secret-value\nsafe line" }),
    )];

    let result = build_context_pack(ContextPackInput {
        session: &session,
        target_provider: &ProviderId("opencode".to_string()),
        current_prompt: "continue",
        profile: &profile,
        workspace_status: None,
        project: &project,
        events: &events,
        route_decisions: &[],
        handoffs: &[],
        diff_hunks: &[],
    });

    assert!(result.prompt.contains("[redacted sensitive line]"));
    assert!(!result.prompt.contains("secret-value"));
}

#[test]
fn prompt_preview_redacts_sensitive_lines() {
    let preview = prompt_preview("TOKEN=secret-value\ncontinue");

    assert!(preview.contains("[redacted sensitive line]"));
    assert!(!preview.contains("secret-value"));
}

fn session_with_native_ids(native_ids: BTreeMap<String, String>) -> TaskSession {
    let now = Utc::now();
    TaskSession {
        id: TaskSessionId("task_1".to_string()),
        workspace_id: WorkspaceId("ws_1".to_string()),
        objective: "ship context broker".to_string(),
        active_provider_id: Some(ProviderId("codex".to_string())),
        provider_native_session_ids: native_ids,
        continuity: Default::default(),
        status: TaskSessionStatus::Running,
        created_at: now,
        updated_at: now,
    }
}

fn project() -> Project {
    let now = Utc::now();
    Project {
        id: ProjectId("prj_1".to_string()),
        workspace_id: WorkspaceId("ws_1".to_string()),
        root: PathBuf::from("/tmp/baize"),
        kind: ProjectKind::Directory,
        vcs: VcsKind::None,
        active_branch: None,
        trust_level: TrustLevel::Trusted,
        created_at: now,
        updated_at: now,
    }
}

fn profile(provider_id: &str, supports_native_resume: bool, budget: u32) -> ProviderContextProfile {
    ProviderContextProfile {
        provider_id: ProviderId(provider_id.to_string()),
        command: provider_id.to_string(),
        transport: "cli".to_string(),
        prompt_entrypoint: "run".to_string(),
        supports_native_resume,
        native_resume_arg: None,
        native_session_id_capture: None,
        supports_structured_output: false,
        structured_output_format: None,
        supports_acp: false,
        supports_mcp: false,
        supports_workspace_arg: false,
        workspace_arg: None,
        supports_file_attachment: false,
        file_arg: None,
        supports_conversation_export: false,
        supports_conversation_import: false,
        context_window_tokens: None,
        default_context_budget_tokens: budget,
        reserved_output_tokens: 0,
        token_budget_source: TokenBudgetSource::ConfigDefault,
        token_budget_confidence: TokenBudgetConfidence::Low,
        preferred_context_format: PreferredContextFormat::XmlTaggedMarkdown,
        same_provider_resume_strategy: "native".to_string(),
        cross_provider_handoff_strategy: "delta".to_string(),
        risk_level: "medium".to_string(),
        workspace_exposure_notes: "test".to_string(),
        last_verified_at: "2026-07-09".to_string(),
        source_refs: Vec::new(),
    }
}

fn event(event_type: &str, provider_id: &str, payload: serde_json::Value) -> BaizeEvent {
    let mut event = BaizeEvent::new(event_type, payload);
    event.provider_id = Some(ProviderId(provider_id.to_string()));
    event
}
