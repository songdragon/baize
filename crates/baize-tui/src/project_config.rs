use super::*;

pub(super) fn project_config_lines(value: &Value) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::styled(
            "SHARED PROJECT CONFIGURATION",
            Style::default().fg(Color::Cyan),
        ),
        Line::raw(format!(
            "Plan: {}",
            value["status"].as_str().unwrap_or("unknown")
        )),
        Line::raw("Inventory/preview only; native loading/tool execution NOT verified."),
        Line::raw(format!(
            "{} -> {}",
            value["source"].as_str().unwrap_or("?"),
            value["target"].as_str().unwrap_or("?")
        )),
    ];
    for (key, label) in [
        ("shared_instructions", "Shared instructions"),
        ("shared_skills", "Shared skills (one source)"),
        ("mcp_servers", "Shared MCP servers"),
        ("native_only_servers", "Preserved Codex-only servers"),
        ("changes", "Proposed changes"),
        ("conflicts", "BLOCKED"),
        ("warnings", "Scope and limits"),
    ] {
        if let Some(items) = value[key].as_array().filter(|items| !items.is_empty()) {
            lines.push(Line::styled(
                label,
                Style::default().fg(if key == "conflicts" {
                    Color::Yellow
                } else {
                    Color::Cyan
                }),
            ));
            for item in items {
                if let Some(text) = item.as_str() {
                    lines.extend(text.lines().map(|line| Line::raw(format!("  {line}"))));
                }
            }
        }
    }
    lines.push(Line::raw(
        "Run from project root: baize project-config plan",
    ));
    if value["status"] == "ready" {
        lines.push(Line::raw(format!(
            "Review first, then: baize project-config apply {}",
            value["plan_id"].as_str().unwrap_or("<plan-id>")
        )));
        lines.push(Line::raw(
            "Keep the same --replace names used in the reviewed CLI plan.",
        ));
    }
    lines.push(Line::raw(
        "Apply returns a backup ID; restore checks for later edits.",
    ));
    lines.push(Line::raw("Lost the receipt? baize project-config history"));
    lines.push(Line::raw(
        "No global configuration, trust or approval policy is changed.",
    ));
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    #[test]
    fn renders_configuration_as_a_review_without_claiming_execution() {
        let mut state = TuiState {
            workspace_id: Some("ws_fixture".into()),
            input: "keep draft".into(),
            ..Default::default()
        };
        apply_action(
            &mut state,
            ActionResult {
                action: Action::ProjectConfig,
                session_id: None,
                workspace_id: Some("ws_fixture".into()),
                response: json!({"project_config":{"status":"blocked","source":".agents/mcp_config.json","target":".codex/config.toml","shared_skills":[".agents/skills/fixture/SKILL.md"],"conflicts":["fixture: requires explicit replacement"]}}),
                extra: Value::Null,
            },
        );
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal
            .draw(|frame| {
                frame.render_widget(
                    Paragraph::new(project_config_lines(&state.ui.context["project_config"])),
                    frame.area(),
                )
            })
            .unwrap();
        let screen = format!("{:?}", terminal.backend().buffer());
        assert!(screen.contains("NOT verified"));
        assert!(screen.contains("BLOCKED"));
        assert!(screen.contains("one source"));
        assert_eq!(state.input, "keep draft");
        assert_eq!(state.ui.panel, Panel::Context);
    }

    #[test]
    fn stale_workspace_responses_do_not_replace_current_context() {
        let mut state = TuiState {
            workspace_id: Some("ws_new".into()),
            ..Default::default()
        };
        state.ui.context = json!({"keep":"current"});
        apply_action(
            &mut state,
            ActionResult {
                action: Action::ProjectConfig,
                session_id: None,
                workspace_id: Some("ws_old".into()),
                response: json!({"project_config":{}}),
                extra: Value::Null,
            },
        );
        assert_eq!(state.ui.context, json!({"keep":"current"}));
        let mut task = None;
        state.workspace_id = None;
        request_action(&mut state, &mut task, Action::ProjectConfig);
        assert!(task.is_none());
        assert!(state.ui.notice.as_ref().unwrap().contains("workspace"));
    }
}
