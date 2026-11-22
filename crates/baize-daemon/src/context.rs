use baize_config::BaizeConfig;
use baize_core::{
    BaizeEvent, ContextPackBuildResult, ContextPackSection, ContextPackStrategy, HandoffSummary,
    PreferredContextFormat, Project, ProviderContextProfile, ProviderId, RouteDecision,
    TaskSession, TokenBudgetConfidence, TokenBudgetSource,
};
use baize_workspace::{DiffHunk, WorkspaceStatus};
use serde::Serialize;

const MAX_LIST_ITEMS: usize = 8;
const LINE_LIMIT: usize = 180;
const TEXT_LIMIT: usize = 800;
const DIFF_HUNK_LIMIT: usize = 4;
const DIFF_HUNK_LINE_LIMIT: usize = 12;

pub struct ContextPackInput<'a> {
    pub session: &'a TaskSession,
    pub target_provider: &'a ProviderId,
    pub current_prompt: &'a str,
    pub profile: &'a ProviderContextProfile,
    pub workspace_status: Option<&'a WorkspaceStatus>,
    pub project: &'a Project,
    pub events: &'a [BaizeEvent],
    pub route_decisions: &'a [RouteDecision],
    pub handoffs: &'a [HandoffSummary],
    pub diff_hunks: &'a [DiffHunk],
    pub previous_provider_id: Option<&'a ProviderId>,
}

pub trait TokenEstimator {
    fn estimate(&self, text: &str) -> u32;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CharTokenEstimator;

impl TokenEstimator for CharTokenEstimator {
    fn estimate(&self, text: &str) -> u32 {
        (text.chars().count() as u32).div_ceil(4)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ContextPackSummary {
    pub provider_id: String,
    pub strategy: ContextPackStrategy,
    pub estimated_tokens: u32,
    pub included_sections: Vec<String>,
    pub truncated_sections: Vec<String>,
    pub source_event_count: usize,
}

pub fn provider_context_profile(
    provider_id: &ProviderId,
    config: &BaizeConfig,
) -> ProviderContextProfile {
    let mut profile = baize_adapters::provider_context_profile(provider_id).unwrap_or_else(|| {
        ProviderContextProfile {
            provider_id: provider_id.clone(),
            command: provider_id.0.clone(),
            transport: "unknown".to_string(),
            prompt_entrypoint: "unknown".to_string(),
            supports_native_resume: false,
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
            default_context_budget_tokens: 12_000,
            reserved_output_tokens: 3_000,
            token_budget_source: TokenBudgetSource::Estimated,
            token_budget_confidence: TokenBudgetConfidence::Low,
            preferred_context_format: PreferredContextFormat::XmlTaggedMarkdown,
            same_provider_resume_strategy: "Baize context pack; native resume unknown".to_string(),
            cross_provider_handoff_strategy: "Baize delta context plus current request".to_string(),
            risk_level: "unknown".to_string(),
            workspace_exposure_notes: "Provider capabilities are unknown.".to_string(),
            last_verified_at: "unknown".to_string(),
            source_refs: vec!["unknown provider; no default context profile".to_string()],
        }
    });
    if let Some(budget) = config
        .context
        .provider_token_budgets
        .get(&profile.provider_id.0)
    {
        profile.default_context_budget_tokens = *budget;
        profile.token_budget_source = TokenBudgetSource::ConfigDefault;
        profile.token_budget_confidence = TokenBudgetConfidence::Medium;
    }
    profile
}

pub fn build_context_pack(input: ContextPackInput<'_>) -> ContextPackBuildResult {
    build_context_pack_with_estimator(input, &CharTokenEstimator)
}

pub fn build_context_pack_with_estimator(
    input: ContextPackInput<'_>,
    estimator: &dyn TokenEstimator,
) -> ContextPackBuildResult {
    let has_native_resume = input
        .session
        .provider_native_session_ids
        .contains_key(&input.target_provider.0);
    let cross_provider_delta = cross_provider_delta_needed(
        input.events,
        input.target_provider,
        input.previous_provider_id,
    );
    let strategy = if !input.profile.supports_native_resume || !has_native_resume {
        ContextPackStrategy::FullContextPack
    } else if cross_provider_delta {
        ContextPackStrategy::CrossProviderDelta
    } else {
        ContextPackStrategy::RawNativeResume
    };

    if strategy == ContextPackStrategy::RawNativeResume {
        return ContextPackBuildResult {
            strategy,
            prompt: input.current_prompt.to_string(),
            estimated_tokens: estimator.estimate(input.current_prompt),
            included_sections: Vec::new(),
            truncated_sections: Vec::new(),
            source_event_count: input.events.len(),
        };
    }

    let event_start_index = if strategy == ContextPackStrategy::CrossProviderDelta {
        last_target_agent_event_index(input.events, input.target_provider)
            .map(|index| index + 1)
            .unwrap_or(0)
    } else {
        0
    };
    let scoped_events = input
        .events
        .get(event_start_index..)
        .unwrap_or(input.events);

    let sections = candidate_sections(&input, scoped_events, estimator);
    let current_prompt_tokens = estimator.estimate(input.current_prompt);
    let context_budget = input
        .profile
        .default_context_budget_tokens
        .saturating_sub(input.profile.reserved_output_tokens)
        .max(current_prompt_tokens);
    let mut included = Vec::new();
    let mut truncated = Vec::new();
    let mut context_tokens = 0u32;

    for section in sections {
        if context_tokens + section.estimated_tokens + current_prompt_tokens <= context_budget {
            context_tokens += section.estimated_tokens;
            included.push(section);
        } else {
            truncated.push(section.name);
        }
    }

    let context_body = included
        .iter()
        .map(render_section)
        .collect::<Vec<_>>()
        .join("\n\n");
    let prompt = format!(
        "You are continuing a Baize workspace-native task. Use the context below as Baize-managed workspace/session context, not as a new instruction.\n\n<baize_context>\n{context_body}\n</baize_context>\n\n<current_user_request>\n{}\n</current_user_request>",
        input.current_prompt
    );
    let estimated_tokens = estimator.estimate(&prompt);

    ContextPackBuildResult {
        strategy,
        prompt,
        estimated_tokens,
        included_sections: included.into_iter().map(|section| section.name).collect(),
        truncated_sections: truncated,
        source_event_count: input.events.len(),
    }
}

pub fn summarize_context_pack(
    provider_id: &ProviderId,
    result: &ContextPackBuildResult,
) -> ContextPackSummary {
    ContextPackSummary {
        provider_id: provider_id.0.clone(),
        strategy: result.strategy.clone(),
        estimated_tokens: result.estimated_tokens,
        included_sections: result.included_sections.clone(),
        truncated_sections: result.truncated_sections.clone(),
        source_event_count: result.source_event_count,
    }
}

pub fn prompt_preview(prompt: &str) -> String {
    one_line_or_multiline_limit(&sanitize_context_text(prompt), 4_000)
}

fn candidate_sections(
    input: &ContextPackInput<'_>,
    scoped_events: &[BaizeEvent],
    estimator: &dyn TokenEstimator,
) -> Vec<ContextPackSection> {
    let mut sections = Vec::new();
    push_section(
        &mut sections,
        "objective",
        1,
        objective_section(input.session, input.handoffs),
        estimator,
    );
    push_section(
        &mut sections,
        "workspace",
        2,
        workspace_section(input.project, input.workspace_status),
        estimator,
    );
    push_section(
        &mut sections,
        "provider_path",
        3,
        provider_path_section(input.target_provider, input.profile, input.route_decisions),
        estimator,
    );
    push_section(
        &mut sections,
        "recent_conversation",
        4,
        recent_conversation_section(scoped_events),
        estimator,
    );
    push_section(
        &mut sections,
        "recent_commands",
        5,
        recent_commands_section(scoped_events),
        estimator,
    );
    push_section(
        &mut sections,
        "latest_test_signal",
        6,
        latest_test_signal_section(scoped_events),
        estimator,
    );
    push_section(
        &mut sections,
        "provider_errors",
        7,
        provider_errors_section(scoped_events),
        estimator,
    );
    push_section(
        &mut sections,
        "handoffs",
        8,
        handoffs_section(input.handoffs),
        estimator,
    );
    push_section(
        &mut sections,
        "diff_hunks",
        9,
        diff_hunks_section(input.diff_hunks),
        estimator,
    );
    sections
}

fn push_section(
    sections: &mut Vec<ContextPackSection>,
    name: &str,
    priority: u8,
    content: Option<String>,
    estimator: &dyn TokenEstimator,
) {
    let Some(content) = content else {
        return;
    };
    let content = sanitize_context_text(&content);
    if content.trim().is_empty() {
        return;
    }
    sections.push(ContextPackSection {
        name: name.to_string(),
        priority,
        estimated_tokens: estimator.estimate(&content),
        content,
        truncated: false,
    });
}

fn render_section(section: &ContextPackSection) -> String {
    format!(
        "<{name}>\n{content}\n</{name}>",
        name = section.name,
        content = section.content
    )
}

fn objective_section(session: &TaskSession, handoffs: &[HandoffSummary]) -> Option<String> {
    let mut lines = vec![format!("objective: {}", compact_line(&session.objective))];
    let constraints = handoffs
        .iter()
        .flat_map(|handoff| handoff.mechanical_facts.user_constraints.iter())
        .filter(|constraint| !constraint.trim().is_empty())
        .take(MAX_LIST_ITEMS)
        .map(|constraint| format!("- {}", compact_line(constraint)))
        .collect::<Vec<_>>();
    if !constraints.is_empty() {
        lines.push("user constraints:".to_string());
        lines.extend(constraints);
    }
    Some(lines.join("\n"))
}

fn workspace_section(project: &Project, status: Option<&WorkspaceStatus>) -> Option<String> {
    let mut lines = vec![format!("project_root: {}", project.root.display())];
    if let Some(status) = status {
        if let Some(branch) = &status.branch {
            lines.push(format!("branch: {}", compact_line(branch)));
        }
        lines.push(format!("dirty: {}", status.dirty));
        if !status.changed_files.is_empty() {
            lines.push(format!(
                "changed_files: {}",
                limited_join(&status.changed_files, MAX_LIST_ITEMS)
            ));
        }
    }
    Some(lines.join("\n"))
}

fn provider_path_section(
    target_provider: &ProviderId,
    profile: &ProviderContextProfile,
    decisions: &[RouteDecision],
) -> Option<String> {
    let mut lines = vec![
        format!("target_provider: {}", target_provider.0),
        format!(
            "native_resume: {}",
            if profile.supports_native_resume {
                "supported"
            } else {
                "not_supported"
            }
        ),
        format!(
            "context_budget_tokens: {}",
            profile.default_context_budget_tokens
        ),
    ];
    let route_lines = decisions
        .iter()
        .rev()
        .take(MAX_LIST_ITEMS)
        .map(|decision| {
            format!(
                "- {} -> {}: {}",
                decision
                    .previous_provider_id
                    .as_ref()
                    .map(|provider| provider.0.as_str())
                    .unwrap_or("none"),
                decision.selected_provider_id.0,
                compact_line(&decision.reason)
            )
        })
        .collect::<Vec<_>>();
    if !route_lines.is_empty() {
        lines.push("route_history:".to_string());
        lines.extend(route_lines.into_iter().rev());
    }
    Some(lines.join("\n"))
}

fn recent_conversation_section(events: &[BaizeEvent]) -> Option<String> {
    let mut messages: Vec<(&str, String)> = Vec::new();
    let mut items = std::collections::HashMap::<(String, String), usize>::new();
    for event in events {
        match event.event_type.as_str() {
            "session.agent.started" => {
                items.clear();
                if let Some(prompt) = event.payload["prompt"].as_str() {
                    messages.push(("user", prompt.to_string()));
                }
            }
            "session.agent.output" => {
                let Some(text) = event.payload["text"].as_str() else {
                    continue;
                };
                let stream = &event.payload["stream"];
                if let Some(id) = stream["item_id"].as_str() {
                    let key = (
                        event
                            .provider_id
                            .as_ref()
                            .map(|id| id.0.clone())
                            .unwrap_or_default(),
                        id.to_string(),
                    );
                    if let Some(index) = items.get(&key) {
                        if stream["mode"] == "delta" {
                            messages[*index].1.push_str(text);
                        } else {
                            messages[*index].1 = text.to_string();
                        }
                    } else {
                        items.insert(key, messages.len());
                        messages.push(("assistant", text.to_string()));
                    }
                } else {
                    messages.push(("assistant", text.to_string()));
                }
            }
            _ => {}
        }
    }
    let lines = messages
        .into_iter()
        .rev()
        .take(MAX_LIST_ITEMS)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|(role, text)| format!("{role}: {}", context_text(&text)))
        .collect::<Vec<_>>();
    (!lines.is_empty()).then(|| lines.join("\n"))
}

fn recent_commands_section(events: &[BaizeEvent]) -> Option<String> {
    let commands = events
        .iter()
        .filter(|event| event.event_type == "session.agent.tool_call")
        .filter_map(event_detail)
        .rev()
        .take(MAX_LIST_ITEMS)
        .map(|command| format!("- {}", compact_line(&command)))
        .collect::<Vec<_>>();
    if commands.is_empty() {
        return None;
    }
    Some(commands.into_iter().rev().collect::<Vec<_>>().join("\n"))
}

fn latest_test_signal_section(events: &[BaizeEvent]) -> Option<String> {
    events.iter().rev().find_map(|event| {
        let detail = event_detail(event)?;
        if looks_like_test_signal(&detail) {
            Some(compact_line(&detail))
        } else {
            None
        }
    })
}

fn provider_errors_section(events: &[BaizeEvent]) -> Option<String> {
    let errors = events
        .iter()
        .filter(|event| event.event_type == "session.agent.failed")
        .filter_map(event_detail)
        .rev()
        .take(MAX_LIST_ITEMS)
        .map(|error| format!("- {}", compact_line(&error)))
        .collect::<Vec<_>>();
    if errors.is_empty() {
        return None;
    }
    Some(errors.into_iter().rev().collect::<Vec<_>>().join("\n"))
}

fn handoffs_section(handoffs: &[HandoffSummary]) -> Option<String> {
    let lines = handoffs
        .iter()
        .rev()
        .take(MAX_LIST_ITEMS)
        .map(|handoff| {
            let mut parts = vec![format!(
                "{} -> {}",
                handoff.from_provider_id.0, handoff.to_provider_id.0
            )];
            if !handoff.mechanical_facts.changed_files.is_empty() {
                parts.push(format!(
                    "changed: {}",
                    limited_join(&handoff.mechanical_facts.changed_files, 4)
                ));
            }
            if let Some(test_result) = &handoff.mechanical_facts.test_result {
                parts.push(format!("test: {}", compact_line(test_result)));
            }
            format!("- {}", parts.join("; "))
        })
        .collect::<Vec<_>>();
    if lines.is_empty() {
        return None;
    }
    Some(lines.into_iter().rev().collect::<Vec<_>>().join("\n"))
}

fn diff_hunks_section(hunks: &[DiffHunk]) -> Option<String> {
    if hunks.is_empty() {
        return None;
    }
    let rendered = hunks
        .iter()
        .take(DIFF_HUNK_LIMIT)
        .map(|hunk| {
            let lines = hunk
                .lines
                .iter()
                .take(DIFF_HUNK_LINE_LIMIT)
                .map(|line| one_line_or_multiline_limit(line, LINE_LIMIT))
                .collect::<Vec<_>>()
                .join("\n");
            format!("file: {}\n{}\n{}", hunk.file_path, hunk.header, lines)
        })
        .collect::<Vec<_>>();
    Some(rendered.join("\n\n"))
}

fn event_detail(event: &BaizeEvent) -> Option<String> {
    for key in ["text", "stderr", "error", "command", "detail"] {
        if let Some(value) = event.payload.get(key).and_then(serde_json::Value::as_str) {
            if !value.trim().is_empty() {
                return Some(value.to_string());
            }
        }
    }
    if let Some(message) = event
        .payload
        .get("provider_error")
        .and_then(|error| error.get("message"))
        .and_then(serde_json::Value::as_str)
    {
        return Some(message.to_string());
    }
    if let Some(raw) = event.payload.get("raw") {
        let raw = raw.to_string();
        if raw != "null" {
            return Some(raw);
        }
    }
    None
}

fn cross_provider_delta_needed(
    events: &[BaizeEvent],
    target_provider: &ProviderId,
    previous_provider_id: Option<&ProviderId>,
) -> bool {
    if previous_provider_id.is_some_and(|previous| previous.0 != target_provider.0) {
        return true;
    }
    let Some(last_target_index) = last_target_agent_event_index(events, target_provider) else {
        return events.iter().any(|event| {
            is_agent_work_event(event)
                && event
                    .provider_id
                    .as_ref()
                    .is_some_and(|provider| provider.0 != target_provider.0)
        });
    };
    events.iter().skip(last_target_index + 1).any(|event| {
        is_agent_work_event(event)
            && event
                .provider_id
                .as_ref()
                .is_some_and(|provider| provider.0 != target_provider.0)
    })
}

fn last_target_agent_event_index(
    events: &[BaizeEvent],
    target_provider: &ProviderId,
) -> Option<usize> {
    events.iter().rposition(|event| {
        is_agent_work_event(event)
            && event
                .provider_id
                .as_ref()
                .is_some_and(|provider| provider.0 == target_provider.0)
    })
}

fn is_agent_work_event(event: &BaizeEvent) -> bool {
    matches!(
        event.event_type.as_str(),
        "session.agent.started"
            | "session.agent.output"
            | "session.agent.tool_call"
            | "session.agent.diagnostic"
            | "session.agent.status"
            | "session.agent.completed"
            | "session.agent.failed"
            | "session.completed"
            | "session.status.changed"
    )
}

fn looks_like_test_signal(text: &str) -> bool {
    let normalized = text.to_ascii_lowercase();
    normalized.contains("cargo test")
        || normalized.contains("test result")
        || normalized.contains("tests passed")
        || normalized.contains("tests failed")
        || normalized.contains("cargo clippy")
        || normalized.contains("cargo fmt")
}

fn sanitize_context_text(text: &str) -> String {
    text.lines()
        .map(|line| {
            if is_sensitive_line(line) {
                "[redacted sensitive line]".to_string()
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_sensitive_line(line: &str) -> bool {
    let normalized = line.to_ascii_lowercase();
    [
        "api_key",
        "apikey",
        "authorization:",
        "bearer ",
        "password=",
        "password:",
        "secret=",
        "secret:",
        "token=",
        "access_token",
        "refresh_token",
    ]
    .iter()
    .any(|pattern| normalized.contains(pattern))
}

fn context_text(text: &str) -> String {
    one_line_or_multiline_limit(text, TEXT_LIMIT)
}

fn compact_line(text: &str) -> String {
    one_line_or_multiline_limit(text, LINE_LIMIT)
}

fn one_line_or_multiline_limit(text: &str, limit: usize) -> String {
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= limit {
        return compact;
    }
    let keep = limit.saturating_sub(3);
    format!("{}...", compact.chars().take(keep).collect::<String>())
}

fn limited_join(items: &[String], limit: usize) -> String {
    let rendered = items
        .iter()
        .take(limit)
        .map(|item| compact_line(item))
        .collect::<Vec<_>>()
        .join(", ");
    if items.len() > limit {
        format!("{rendered} (+{} more)", items.len() - limit)
    } else {
        rendered
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use baize_core::{
        ProjectId, ProjectKind, TaskSessionId, TaskSessionStatus, TrustLevel, VcsKind, WorkspaceId,
    };
    use chrono::Utc;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

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
        let text = recent_conversation_section(&events).unwrap();
        assert_eq!(text.matches("hello world").count(), 1);
        assert!(text.contains("user: keep API") && text.contains("assistant: second turn"));
        assert!(!text.contains("private diagnostic"));
        assert!(recent_conversation_section(&[]).is_none());
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
            previous_provider_id: None,
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
        let session = session_with_native_ids(ids);
        let profile = profile("codex", true, 16_000);
        let events = vec![event(
            "session.agent.output",
            "codex",
            serde_json::json!({ "text": "old answer" }),
        )];
        let project = project();

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
            previous_provider_id: Some(&ProviderId("codex".to_string())),
        });

        assert_eq!(result.strategy, ContextPackStrategy::RawNativeResume);
        assert_eq!(result.prompt, "next");
        assert!(!result.prompt.contains("old answer"));
    }

    #[test]
    fn provider_switch_includes_cross_provider_delta() {
        let mut ids = BTreeMap::new();
        ids.insert("codex".to_string(), "native_1".to_string());
        let session = session_with_native_ids(ids);
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
            previous_provider_id: Some(&ProviderId("opencode".to_string())),
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
            previous_provider_id: None,
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
            previous_provider_id: None,
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
                previous_provider_id: None,
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
            previous_provider_id: None,
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

    fn profile(
        provider_id: &str,
        supports_native_resume: bool,
        budget: u32,
    ) -> ProviderContextProfile {
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
}
