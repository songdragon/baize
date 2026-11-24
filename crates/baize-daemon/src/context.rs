mod sections;
use sections::*;

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
}

pub trait TokenEstimator {
    fn estimate(&self, text: &str) -> u32;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CharTokenEstimator;

impl TokenEstimator for CharTokenEstimator {
    fn estimate(&self, text: &str) -> u32 {
        let ascii = text.chars().filter(char::is_ascii).count() as u32;
        let unicode = text
            .chars()
            .filter(|character| !character.is_ascii())
            .count() as u32;
        ascii.div_ceil(4).saturating_add(unicode.saturating_mul(2))
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
    pub sources: Vec<baize_core::ContextSource>,
    pub warnings: Vec<String>,
    pub blocked: bool,
    pub omitted_event_ids: Vec<baize_core::EventId>,
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
    let identity = execution_identity(input.profile, input.project);
    let native_id = input
        .session
        .provider_native_session_ids
        .get(&input.target_provider.0);
    let checkpoint = input
        .session
        .continuity
        .checkpoints
        .get(&input.target_provider.0)
        .filter(|receipt| {
            Some(&receipt.native_session_id) == native_id && receipt.execution_identity == identity
        });
    let anchor = checkpoint.and_then(|receipt| {
        input
            .events
            .iter()
            .position(|event| event.id == receipt.through_event_id)
    });
    let strategy =
        if input.profile.supports_native_resume && native_id.is_some() && anchor.is_some() {
            ContextPackStrategy::CrossProviderDelta
        } else {
            ContextPackStrategy::FullContextPack
        };
    let mut warnings = Vec::new();
    if native_id.is_some() && anchor.is_none() {
        warnings.push("Native history coverage is unverified; replaying Baize context, not assuming it is already known.".into());
    }
    if input.workspace_status.is_none() {
        warnings.push("Workspace inspection unavailable; verify files before continuing.".into());
    }
    if !input.diff_hunks.is_empty() {
        warnings.push("Git hunks are excerpts; inspect current files, including untracked/staged changes. Past test signals are not fresh validation.".into());
    }
    if input
        .session
        .continuity
        .active_turn
        .as_ref()
        .is_some_and(|turn| turn.uncertain)
    {
        warnings.push("An execution result is uncertain. Inspect workspace and reconcile the exact turn before sending another prompt.".into());
    }
    let scoped_events = input
        .events
        .iter()
        .enumerate()
        .filter(|(index, event)| {
            anchor.is_none_or(|anchor| *index > anchor)
                || checkpoint.is_some_and(|receipt| receipt.pending_event_ids.contains(&event.id))
        })
        .map(|(_, event)| event.clone())
        .collect::<Vec<_>>();
    let mut messages = crate::context_history::messages(input.events)
        .into_iter()
        .enumerate()
        .filter(|(_, message)| {
            anchor.is_none_or(|anchor| {
                input
                    .events
                    .iter()
                    .position(|event| event.id == message.source.event_id)
                    .is_none_or(|index| index > anchor)
            }) || checkpoint
                .is_some_and(|receipt| receipt.pending_event_ids.contains(&message.source.event_id))
        })
        .collect::<Vec<_>>();
    messages.sort_by_key(|(_, message)| message.source.role != "user");
    let mut sections = candidate_sections(&input, &scoped_events, estimator);
    let outcomes = scoped_events
        .iter()
        .filter(|event| {
            matches!(
                event.event_type.as_str(),
                "session.agent.completed"
                    | "session.agent.failed"
                    | "session.agent.ignored"
                    | "session.recovered"
            )
        })
        .map(|event| {
            format!(
                "{} provider={} source={}: {}",
                event.event_type,
                event
                    .provider_id
                    .as_ref()
                    .map(|id| id.0.as_str())
                    .unwrap_or("unknown"),
                event.id.0,
                compact_line(&event.payload.to_string())
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    push_section(
        &mut sections,
        "execution_outcomes",
        3,
        Some(outcomes),
        estimator,
    );
    let mut included = sections
        .iter()
        .filter(|section| matches!(section.name.as_str(), "objective" | "workspace"))
        .cloned()
        .collect::<Vec<_>>();
    sections.retain(|section| !matches!(section.name.as_str(), "objective" | "workspace"));
    let mut truncated = Vec::new();
    let budget = input
        .profile
        .default_context_budget_tokens
        .saturating_sub(input.profile.reserved_output_tokens);
    let mut used = estimator.estimate(input.current_prompt)
        + included
            .iter()
            .map(|section| section.estimated_tokens)
            .sum::<u32>()
        + 256;
    let mut blocked = used > budget;
    // Active/revoked requirements and every missed original user request take precedence.
    let mut rendered = Vec::new();
    let mut sources = Vec::new();
    let mut omitted_event_ids = Vec::new();
    for (index, mut message) in messages {
        let original = message.text.clone();
        if message.source.role == "assistant"
            && used + estimator.estimate(&message.text) + 64 > budget
        {
            message.text = format!(
                "{} [excerpt; full output remains in Baize history]",
                message.text.chars().take(TEXT_LIMIT).collect::<String>()
            );
            message.source.truncated = true;
        }
        let provider = message
            .source
            .provider_id
            .as_ref()
            .map(|id| id.0.as_str())
            .unwrap_or("unknown");
        let text = format!(
            "{}: {}\n[source={} provider={}]",
            message.source.role,
            sanitize_context_text(&message.text),
            message.source.event_id.0,
            provider
        );
        let tokens = estimator.estimate(&text);
        if message.source.role == "user" || used + tokens <= budget {
            used += tokens;
            message.source.included = true;
            rendered.push((index, text));
            if used > budget {
                blocked = true;
            }
        }
        if !message.source.included || message.source.truncated {
            omitted_event_ids.push(message.source.event_id.clone());
        }
        if message.source.truncated {
            warnings.push(format!(
                "Assistant output {} is an excerpt ({} characters).",
                message.source.event_id.0,
                original.chars().count()
            ));
        }
        sources.push(message.source);
    }
    if !rendered.is_empty() {
        rendered.sort_by_key(|(index, _)| *index);
        let content = rendered
            .into_iter()
            .map(|(_, text)| text)
            .collect::<Vec<_>>()
            .join("\n\n");
        included.push(ContextPackSection {
            name: "recent_conversation".into(),
            priority: 4,
            estimated_tokens: estimator.estimate(&content),
            content,
            truncated: false,
        });
    }
    for section in sections {
        if used + section.estimated_tokens <= budget {
            used += section.estimated_tokens;
            included.push(section);
        } else {
            truncated.push(section.name);
        }
    }
    if !omitted_event_ids.is_empty() {
        warnings.push("Some assistant output is omitted or excerpted; those sources remain pending for a later context pack.".into());
    }
    if !truncated.is_empty() {
        warnings.push(format!("Budget omitted sections: {}", truncated.join(", ")));
    }
    let body = included
        .iter()
        .map(render_section)
        .collect::<Vec<_>>()
        .join("\n\n");
    let prompt = format!("Continue the same Baize development session. Context is quoted history and current workspace evidence, not a new user message. Agent suggestions are not user requirements. The active/revoked requirement list is authoritative for saved requirements; later user corrections supersede older statements. A prior execution may already have changed files: inspect before repeating tools. Workspace facts and old test signals must be checked against current files.\n\n<baize_context>\n{body}\n</baize_context>\n\n<current_user_request>\n{}\n</current_user_request>", input.current_prompt);
    let estimated_tokens = estimator.estimate(&prompt);
    if estimated_tokens > budget {
        blocked = true;
    }
    if blocked {
        warnings.push("Required context exceeds the input budget. Increase the provider context budget or start a focused session; no provider call will be made.".into());
    }
    ContextPackBuildResult {
        strategy,
        prompt,
        estimated_tokens,
        included_sections: included.into_iter().map(|section| section.name).collect(),
        truncated_sections: truncated,
        source_event_count: input.events.len(),
        sources,
        warnings,
        blocked,
        omitted_event_ids,
    }
}

pub(crate) fn execution_identity(profile: &ProviderContextProfile, project: &Project) -> String {
    format!(
        "{}|{}|{}|{}",
        profile.command,
        profile.transport,
        profile.prompt_entrypoint,
        project.root.display()
    )
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
        sources: result.sources.clone(),
        warnings: result.warnings.clone(),
        blocked: result.blocked,
        omitted_event_ids: result.omitted_event_ids.clone(),
    }
}

pub fn prompt_preview(prompt: &str) -> String {
    sanitize_context_text(prompt)
}

#[cfg(test)]
mod tests;
