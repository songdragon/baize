use super::*;

pub(super) fn candidate_sections(
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

pub(super) fn push_section(
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

pub(super) fn render_section(section: &ContextPackSection) -> String {
    format!(
        "<{name}>\n{content}\n</{name}>",
        name = section.name,
        content = section.content
    )
}

pub(super) fn objective_section(
    session: &TaskSession,
    handoffs: &[HandoffSummary],
) -> Option<String> {
    let mut lines = vec![format!("objective: {}", session.objective)];
    lines.push(
        "Saved user requirements (active entries apply; revoked entries no longer apply):".into(),
    );
    for (key, requirement) in &session.continuity.requirements {
        lines.push(format!(
            "- {key} [r{} {} source={}]: {}",
            requirement.revision,
            if requirement.active {
                "active"
            } else {
                "revoked"
            },
            requirement.source_ref,
            requirement.text
        ));
    }
    for handoff in handoffs
        .iter()
        .filter(|handoff| !session.continuity.imported_handoffs.contains(&handoff.id.0))
    {
        for constraint in &handoff.mechanical_facts.user_constraints {
            lines.push(format!("- legacy handoff {}: {}", handoff.id.0, constraint));
        }
    }
    Some(lines.join("\n"))
}

pub(super) fn workspace_section(
    project: &Project,
    status: Option<&WorkspaceStatus>,
) -> Option<String> {
    let mut lines = vec![format!("project_root: {}", project.root.display())];
    if let Some(status) = status {
        if let Some(branch) = &status.branch {
            lines.push(format!("branch: {}", compact_line(branch)));
        }
        lines.push(format!(
            "HEAD: {}",
            status
                .head
                .as_deref()
                .unwrap_or("unborn or non-Git workspace")
        ));
        lines.push(format!("dirty: {}", status.dirty));
        if !status.changed_files.is_empty() {
            lines.push(format!(
                "changed_files: {}",
                status.changed_files.join(", ")
            ));
        }
    }
    Some(lines.join("\n"))
}

pub(super) fn provider_path_section(
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

pub(super) fn recent_commands_section(events: &[BaizeEvent]) -> Option<String> {
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

pub(super) fn latest_test_signal_section(events: &[BaizeEvent]) -> Option<String> {
    events.iter().rev().find_map(|event| {
        let detail = event_detail(event)?;
        if looks_like_test_signal(&detail) {
            Some(compact_line(&detail))
        } else {
            None
        }
    })
}

pub(super) fn provider_errors_section(events: &[BaizeEvent]) -> Option<String> {
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

pub(super) fn handoffs_section(handoffs: &[HandoffSummary]) -> Option<String> {
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

pub(super) fn diff_hunks_section(hunks: &[DiffHunk]) -> Option<String> {
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

pub(super) fn event_detail(event: &BaizeEvent) -> Option<String> {
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

pub(super) fn looks_like_test_signal(text: &str) -> bool {
    let normalized = text.to_ascii_lowercase();
    normalized.contains("cargo test")
        || normalized.contains("test result")
        || normalized.contains("tests passed")
        || normalized.contains("tests failed")
        || normalized.contains("cargo clippy")
        || normalized.contains("cargo fmt")
}

pub(super) fn sanitize_context_text(text: &str) -> String {
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

pub(super) fn is_sensitive_line(line: &str) -> bool {
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

pub(super) fn compact_line(text: &str) -> String {
    one_line_or_multiline_limit(text, LINE_LIMIT)
}

pub(super) fn one_line_or_multiline_limit(text: &str, limit: usize) -> String {
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= limit {
        return compact;
    }
    let keep = limit.saturating_sub(3);
    format!("{}...", compact.chars().take(keep).collect::<String>())
}

pub(super) fn limited_join(items: &[String], limit: usize) -> String {
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
