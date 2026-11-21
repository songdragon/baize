use super::*;

pub(super) fn render_inspector(frame: &mut Frame<'_>, area: Rect, state: &TuiState) {
    let panel = if !state.pending_permissions.is_empty() && !state.detail_mode {
        Panel::Permissions
    } else {
        state.ui.panel
    };
    let block = panel_block(
        &format!("{} · F7 next", panel.title()),
        state.ui.focus == Focus::Inspector,
    );
    let inner = block.inner(area);
    let lines = inspector_lines(state, panel);
    let text = Text::from(lines);
    let visual_height: usize = text
        .lines
        .iter()
        .map(|line| {
            line.width()
                .max(1)
                .div_ceil(usize::from(inner.width.max(1)))
        })
        .sum();
    let scroll = state.ui.inspector_scroll.min(
        visual_height
            .saturating_sub(inner.height as usize)
            .min(u16::MAX as usize) as u16,
    );
    frame.render_widget(
        Paragraph::new(text)
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        area,
    );
}

pub(super) fn inspector_lines(state: &TuiState, panel: Panel) -> Vec<Line<'static>> {
    let mut lines = vec![];
    match panel {
        Panel::Overview => {
            heading(&mut lines, "CURRENT CONVERSATION");
            field(
                &mut lines,
                "Session",
                state.session_id.as_deref().unwrap_or("New"),
            );
            field(&mut lines, "State", &state.ui.session_status);
            field(
                &mut lines,
                "Active",
                state.active_provider.as_deref().unwrap_or("none"),
            );
            field(&mut lines, "Target", state.selected_provider());
            field(
                &mut lines,
                "Entry",
                "CLI · tools depend on provider configuration",
            );
            field(&mut lines, "Policy", &state.command_policy_label());
            field(&mut lines, "Execution", &state.execution_mode());
            field(&mut lines, "Routing", &state.routing_status());
            if let Some(reason) = &state.route_reason {
                field(&mut lines, "Reason", reason);
            }
            lines.push(Line::raw(""));
            heading(&mut lines, "PROVIDERS");
            provider_rows(&mut lines, state, false);
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                "Tab changes the next executor in this session.",
                muted_style(),
            ));
            lines.push(Line::styled(
                "Ctrl-H prepares a reviewable handoff. Ctrl-Y accepts it.",
                muted_style(),
            ));
            lines.push(Line::styled(
                "F5 refreshes workspace changes and target context.",
                muted_style(),
            ));
            lines.push(Line::styled(
                "Desktop capabilities are not implied by CLI availability.",
                muted_style(),
            ));
            if !state.pending_permissions.is_empty() {
                field(
                    &mut lines,
                    "Pending approvals",
                    &state.pending_permissions.len().to_string(),
                );
            }
        }
        Panel::Changes => {
            heading(&mut lines, "WORKSPACE CHANGES");
            if state.ui.diff.is_null() {
                lines.push(Line::raw("F5 to inspect current workspace changes"));
            } else {
                field(
                    &mut lines,
                    "Workspace",
                    if state.ui.diff["dirty"].as_bool() == Some(true) {
                        "dirty"
                    } else {
                        "clean"
                    },
                );
                lines.push(Line::styled(
                    "Snapshot from last refresh; F5 updates it.",
                    muted_style(),
                ));
                if let Some(files) = state.ui.diff["changed_files"].as_array() {
                    for file in files {
                        lines.push(Line::raw(format!(
                            "  {}",
                            file.as_str().unwrap_or("unknown")
                        )));
                    }
                }
                lines.push(Line::raw(""));
                if let Some(hunks) = state.ui.diff["hunks"].as_array() {
                    for hunk in hunks {
                        heading(
                            &mut lines,
                            hunk["file_path"]
                                .as_str()
                                .or(hunk["path"].as_str())
                                .unwrap_or("diff"),
                        );
                        if let Some(header) = hunk["header"].as_str() {
                            lines.push(Line::styled(
                                header.to_string(),
                                Style::default().fg(Color::Cyan),
                            ));
                        }
                        if let Some(content) = hunk["lines"].as_array() {
                            for line in content {
                                diff_line(&mut lines, line.as_str().unwrap_or(""));
                            }
                        }
                        if let Some(content) = hunk["diff"].as_str() {
                            for line in content.lines() {
                                diff_line(&mut lines, line);
                            }
                        }
                    }
                }
                lines.push(Line::styled(
                    "Untracked files may have no Git hunk. Review their contents separately.",
                    muted_style(),
                ));
            }
        }
        Panel::Context => {
            heading(&mut lines, "TARGET CONTEXT");
            field(&mut lines, "Target", state.selected_provider());
            if state.ui.context.is_null() {
                lines.push(Line::raw("F5 to preview context for the selected provider"));
            } else {
                lines.push(Line::styled(
                    "Preview uses a placeholder request, not your unsent draft.",
                    muted_style(),
                ));
                if let Some(context) = state.ui.context.get("context") {
                    for line in serde_json::to_string_pretty(context)
                        .unwrap_or_default()
                        .lines()
                    {
                        lines.push(Line::raw(line.to_string()));
                    }
                }
                heading(&mut lines, "PROMPT PREVIEW");
                if let Some(preview) = state.ui.context["prompt_preview"].as_str() {
                    for line in preview.lines() {
                        lines.push(Line::raw(line.to_string()));
                    }
                }
            }
        }
        Panel::Handoff => {
            heading(&mut lines, "REVIEW HANDOFF");
            if state.ui.handoff.is_null() {
                lines.push(Line::raw(
                    "Tab selects a target. Ctrl-H prepares a handoff.",
                ));
            } else {
                field(
                    &mut lines,
                    "From",
                    state.ui.handoff["from_provider_id"]
                        .as_str()
                        .unwrap_or("unknown"),
                );
                field(
                    &mut lines,
                    "To",
                    state.ui.handoff["to_provider_id"]
                        .as_str()
                        .unwrap_or("unknown"),
                );
                field(
                    &mut lines,
                    "Status",
                    state.ui.handoff["status"].as_str().unwrap_or("unknown"),
                );
                lines.push(Line::styled(
                    if state.pending_handoff_id.is_some() {
                        "Ctrl-Y accepts this preview; session ID stays the same."
                    } else {
                        "Handoff record; continue through the message composer."
                    },
                    Style::default().fg(Color::Yellow),
                ));
                lines.push(Line::raw(""));
                if let Some(summary) = state.ui.handoff["summary_markdown"].as_str() {
                    for line in summary.lines() {
                        lines.push(Line::raw(line.to_string()));
                    }
                }
                if let Some(facts) = state.ui.handoff.get("mechanical_facts") {
                    heading(&mut lines, "WORKSPACE EVIDENCE");
                    for line in serde_json::to_string_pretty(facts)
                        .unwrap_or_default()
                        .lines()
                    {
                        lines.push(Line::raw(line.to_string()));
                    }
                }
            }
        }
        Panel::Permissions => {
            heading(&mut lines, "PENDING PERMISSIONS");
            lines.push(Line::styled(
                "↑↓ select · Ctrl-A approve · Ctrl-D deny",
                muted_style(),
            ));
            lines.push(Line::styled(
                "Baize approvals are distinct from provider-native prompts.",
                muted_style(),
            ));
            if state.pending_permissions.is_empty() {
                lines.push(Line::raw("No pending permissions. Ctrl-P refreshes."));
            }
            for (index, p) in state
                .pending_permissions
                .iter()
                .enumerate()
                .filter(|(i, _)| *i == state.selected_permission_index)
            {
                lines.push(Line::raw(""));
                heading(
                    &mut lines,
                    &format!(
                        "{} {}",
                        if index == state.selected_permission_index {
                            "›"
                        } else {
                            " "
                        },
                        p.id
                    ),
                );
                field(
                    &mut lines,
                    "Selection",
                    &format!("{} / {}", index + 1, state.pending_permissions.len()),
                );
                field(
                    &mut lines,
                    "Session",
                    p.session_id.as_deref().unwrap_or("workspace"),
                );
                field(
                    &mut lines,
                    "Risk",
                    p.risk_level.as_deref().unwrap_or("unknown"),
                );
                field(&mut lines, "Command", &p.command);
                field(&mut lines, "Reason", &p.reason);
            }
        }
        Panel::Providers => {
            heading(&mut lines, "EXECUTION ENVIRONMENTS");
            provider_rows(&mut lines, state, true);
            lines.push(Line::styled(
                "Health is availability evidence; it does not prove every native tool capability.",
                muted_style(),
            ));
            lines.push(Line::styled(
                "Ctrl-R refreshes. Tab selects the next executor.",
                muted_style(),
            ));
        }
    }
    lines
}

fn heading(lines: &mut Vec<Line<'static>>, label: &str) {
    lines.push(Line::styled(
        label.to_string(),
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ));
}
fn field(lines: &mut Vec<Line<'static>>, label: &str, value: &str) {
    lines.push(Line::from(vec![
        Span::styled(format!("{label}: "), muted_style()),
        Span::raw(value.to_string()),
    ]));
}
fn diff_line(lines: &mut Vec<Line<'static>>, line: &str) {
    lines.push(Line::styled(
        line.to_string(),
        Style::default().fg(if line.starts_with('+') {
            Color::Green
        } else if line.starts_with('-') {
            Color::Red
        } else {
            Color::Gray
        }),
    ));
}
fn provider_rows(lines: &mut Vec<Line<'static>>, state: &TuiState, details: bool) {
    for provider in &state.providers {
        let diagnostic = state
            .provider_diagnostics
            .iter()
            .find(|d| &d.provider_id == provider);
        let health = health_status_for(&state.provider_health, provider)
            .map(short_health_status)
            .unwrap_or("unknown");
        lines.push(Line::styled(
            format!(
                "{} {provider} · CLI · {health}",
                if state.selected_provider() == provider {
                    "›"
                } else {
                    " "
                }
            ),
            health_style(health),
        ));
        if details {
            if let Some(d) = diagnostic {
                field(lines, "Readiness", &d.readiness);
                if let Some(error) = &d.health.last_error {
                    field(lines, "Last error", error);
                }
                for issue in &d.issues {
                    field(lines, "Issue", issue);
                }
                for action in &d.suggested_actions {
                    field(lines, "Next", action);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    #[test]
    fn renders_all_inspector_panels_with_real_details() {
        let mut state = TuiState {
            detail_mode: true,
            ..TuiState::default()
        };
        state.ui.focus = Focus::Inspector;
        state.ui.diff = json!({"dirty":true,"changed_files":["src/main.rs"],"hunks":[{"path":"src/main.rs","header":"@@ -1 +1 @@","lines":["-old","+new"]}]});
        state.ui.context =
            json!({"context":{"strategy":"CrossProviderDelta"},"prompt_preview":"user correction"});
        state.ui.handoff = json!({"from_provider_id":"codex","to_provider_id":"opencode","summary_markdown":"keep public API"});
        state.pending_permissions = vec![PermissionView {
            id: "perm_1".into(),
            session_id: Some("task_1".into()),
            command: "git push".into(),
            reason: "publish changes".into(),
            risk_level: Some("High".into()),
            status: "Pending".into(),
        }];
        for (panel, needle) in [
            (Panel::Overview, "CURRENT CONVERSATION"),
            (Panel::Changes, "+new"),
            (Panel::Context, "user correction"),
            (Panel::Handoff, "keep public API"),
            (Panel::Permissions, "git push"),
            (Panel::Providers, "EXECUTION ENVIRONMENTS"),
        ] {
            state.ui.panel = panel;
            let mut terminal = Terminal::new(TestBackend::new(100, 35)).unwrap();
            terminal.draw(|frame| render(frame, &state)).unwrap();
            assert!(
                format!("{:?}", terminal.backend().buffer()).contains(needle),
                "{panel:?}"
            );
        }
    }
    #[test]
    fn selected_permission_is_visible_instead_of_hidden_below_other_commands() {
        let mut state = TuiState {
            pending_permissions: (0..3)
                .map(|i| PermissionView {
                    id: format!("perm_{i}"),
                    session_id: Some("task_1".into()),
                    command: format!("command_{i}"),
                    reason: "review".into(),
                    status: "Pending".into(),
                    risk_level: Some("High".into()),
                })
                .collect(),
            selected_permission_index: 2,
            ..TuiState::default()
        };
        state.ui.panel = Panel::Permissions;
        let view = inspector_lines(&state, Panel::Permissions)
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(view.contains("command_2") && view.contains("3 / 3"));
        assert!(!view.contains("command_0"));
    }

    #[test]
    fn empty_panels_and_long_scroll_are_safe() {
        let mut state = TuiState::default();
        state.ui.inspector_scroll = u16::MAX;
        let mut terminal = Terminal::new(TestBackend::new(38, 12)).unwrap();
        for panel in [
            Panel::Overview,
            Panel::Changes,
            Panel::Context,
            Panel::Handoff,
            Panel::Permissions,
            Panel::Providers,
        ] {
            state.ui.panel = panel;
            terminal
                .draw(|frame| render_inspector(frame, frame.area(), &state))
                .unwrap();
        }
    }
}
