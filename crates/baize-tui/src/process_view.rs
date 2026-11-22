use super::*;

pub(super) fn output_turn_lines(
    turn: &OutputTurn,
    width: u16,
    focused: bool,
    workspace: &str,
) -> Vec<Line<'static>> {
    let mut lines = vec![Line::raw("")];
    let tools: Vec<_> = turn
        .items
        .iter()
        .filter(|item| item.role == OutputRole::Tool)
        .collect();
    let failed = turn.outcome.as_deref() == Some("failed")
        || tools.iter().any(|item| tool_state(item) == "failed");
    let elapsed = turn
        .elapsed_ms
        .unwrap_or_else(|| turn.started.elapsed().as_millis().min(u64::MAX as u128) as u64);
    let label = match turn.outcome.as_deref() {
        Some("failed") => "处理失败",
        Some("canceled") => "已取消 · provider 可能仍在运行",
        Some("not_submitted") => "请求未提交",
        Some("unconfirmed") => "执行结果待确认",
        Some(_) => "已完成",
        None => "正在处理",
    };
    let summary = format!(
        "{}{}  ·  {} 项操作  ·  {:.1}s  [{} {}]",
        if turn.expanded { "▾ " } else { "▸ " },
        label,
        tools.len(),
        elapsed as f64 / 1000.0,
        if focused { "Space" } else { "F9" },
        if turn.expanded { "收起" } else { "展开" }
    );
    append_wrapped(
        &mut lines,
        &summary,
        width,
        if failed {
            Style::default().fg(Color::Red)
        } else if focused {
            accent_style()
        } else {
            muted_style()
        },
    );
    let entries: Vec<_> = turn
        .items
        .iter()
        .filter(|item| !matches!(item.role, OutputRole::Answer | OutputRole::Status))
        .collect();
    if turn.expanded {
        for (index, item) in entries.iter().enumerate() {
            let selected = focused && turn.selected == index;
            let (label, status) = if item.role == OutputRole::Tool {
                (tool_label(item, workspace), tool_state(item))
            } else {
                (
                    match item.role {
                        OutputRole::Diagnostic => "诊断信息".into(),
                        _ => safe_output_text(&item.text)
                            .lines()
                            .next()
                            .unwrap_or("过程说明")
                            .into(),
                    },
                    "info",
                )
            };
            let icon = match status {
                "failed" => "!",
                "running" => "•",
                "completed" => "✓",
                _ => "·",
            };
            let style = if status == "failed" {
                Style::default().fg(Color::Red)
            } else if selected {
                Style::default().fg(Color::Rgb(218, 225, 236))
            } else {
                muted_style()
            };
            append_wrapped(
                &mut lines,
                &format!("  {} {icon} {label}", if selected { "›" } else { " " }),
                width,
                style,
            );
            if turn.details && turn.selected == index {
                for line in safe_output_text(&item.text).split('\n') {
                    append_wrapped(&mut lines, &format!("      {line}"), width, muted_style());
                }
                if !item.raw.is_null() {
                    for line in serde_json::to_string_pretty(&item.raw)
                        .unwrap_or_default()
                        .lines()
                    {
                        append_wrapped(&mut lines, &format!("      {line}"), width, muted_style());
                    }
                }
            }
        }
        // Native statuses remain inspectable without flooding the operation list.
        if turn.details && entries.is_empty() {
            for item in &turn.items {
                append_wrapped(
                    &mut lines,
                    &safe_output_text(&item.text),
                    width,
                    muted_style(),
                );
            }
        }
    } else if turn.outcome.is_none() {
        if let Some(item) = tools
            .iter()
            .rev()
            .find(|item| tool_state(item) == "running")
            .or_else(|| tools.last())
        {
            append_wrapped(
                &mut lines,
                &format!("  {}", tool_label(item, workspace)),
                width,
                muted_style(),
            );
        }
    }
    for item in &turn.items {
        if item.role == OutputRole::Diagnostic
            || (item.role == OutputRole::Tool && tool_state(item) == "failed")
        {
            let text = safe_output_text(&item.text);
            append_wrapped(
                &mut lines,
                &format!(
                    "  ! {}",
                    short_text(
                        text.lines().next().unwrap_or("操作失败"),
                        usize::from(width.saturating_sub(4)) / 2
                    )
                ),
                width,
                Style::default().fg(Color::Yellow),
            );
        }
    }
    let answers: Vec<_> = turn
        .items
        .iter()
        .filter(|item| item.role == OutputRole::Answer)
        .collect();
    if !answers.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::styled(turn.provider.clone(), accent_style()));
        lines.push(Line::raw(""));
        for item in answers {
            let mut code = false;
            for line in safe_output_text(&item.text).split('\n') {
                if line.trim_start().starts_with("```") {
                    code = !code;
                    continue;
                }
                let heading = !code && line.starts_with('#');
                let text = if heading {
                    line.trim_start_matches('#').trim_start()
                } else {
                    line
                };
                for chunk in wrap_text_chunks(text, usize::from(width.max(1))) {
                    lines.push(if code {
                        Line::styled(chunk, Style::default().fg(Color::Cyan))
                    } else {
                        prose_line(&chunk, heading)
                    });
                }
            }
        }
    }
    lines
}

fn append_wrapped(lines: &mut Vec<Line<'static>>, text: &str, width: u16, style: Style) {
    lines.extend(
        wrap_text_chunks(text, usize::from(width.max(1)))
            .into_iter()
            .map(|text| Line::styled(text, style)),
    );
}

fn prose_line(text: &str, heading: bool) -> Line<'static> {
    let mut spans = vec![];
    let (mut bold, mut code) = (heading, false);
    let mut remaining = text;
    while !remaining.is_empty() {
        if let Some(rest) = remaining.strip_prefix("**") {
            bold = !bold;
            remaining = rest;
            continue;
        }
        if let Some(rest) = remaining.strip_prefix('`') {
            code = !code;
            remaining = rest;
            continue;
        }
        let end = [remaining.find("**"), remaining.find('`')]
            .into_iter()
            .flatten()
            .min()
            .unwrap_or(remaining.len());
        let mut style = Style::default().fg(if code {
            Color::Cyan
        } else {
            Color::Rgb(218, 225, 236)
        });
        if bold {
            style = style.add_modifier(Modifier::BOLD);
        }
        spans.push(Span::styled(remaining[..end].to_string(), style));
        remaining = &remaining[end..];
    }
    Line::from(spans)
}

pub(super) fn tool_state(item: &OutputItem) -> &'static str {
    let lower = item.text.lines().next().unwrap_or("").to_ascii_lowercase();
    let raw = &item.raw;
    if lower.contains("[failed]")
        || lower.contains("[error]")
        || raw
            .pointer("/item/exit_code")
            .and_then(Value::as_i64)
            .is_some_and(|code| code != 0)
    {
        "failed"
    } else if lower.contains("[active]")
        || lower.contains("[running]")
        || lower.contains("[in_progress]")
    {
        "running"
    } else if lower.contains("[done]")
        || lower.contains("[completed]")
        || lower.contains("[success]")
    {
        "completed"
    } else {
        "info"
    }
}

pub(super) fn tool_label(item: &OutputItem, workspace: &str) -> String {
    let raw = &item.raw;
    let step = &raw["step_update"];
    let part = raw.get("item").or_else(|| raw.get("part")).unwrap_or(raw);
    let name = step["tool_name"]
        .as_str()
        .or_else(|| part["tool"].as_str())
        .or_else(|| part["name"].as_str())
        .unwrap_or("");
    let input = step
        .pointer("/tool_info/parameters")
        .or_else(|| part.pointer("/state/input"))
        .or_else(|| part.get("input"))
        .unwrap_or(&Value::Null);
    let path = input["Path"]
        .as_str()
        .or_else(|| input["path"].as_str())
        .or_else(|| input["filePath"].as_str())
        .or_else(|| input["file_path"].as_str());
    if let Some(path) = path {
        let path = Path::new(path)
            .strip_prefix(workspace)
            .ok()
            .and_then(Path::to_str)
            .unwrap_or(path);
        return safe_output_text(&format!(
            "{} {path}",
            if name.contains("read") {
                "读取"
            } else {
                name
            }
        ));
    }
    if let Some(command) = part["command"]
        .as_str()
        .or_else(|| input["command"].as_str())
    {
        return safe_output_text(command);
    }
    safe_output_text(item.text.lines().next().unwrap_or("工具操作"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    fn fixture() -> Vec<Value> {
        vec![
            json!({"id":"1","provider_id":"antigravity","timestamp":"2026-11-22T12:00:00Z","event_type":"session.agent.started","payload":{"prompt":"解释下这个项目"}}),
            json!({"id":"2","provider_id":"antigravity","event_type":"session.agent.output","payload":{"text":"我会先阅读项目说明。","stream":{"mode":"delta","item_id":"intro"}}}),
            json!({"id":"3","provider_id":"antigravity","event_type":"session.agent.tool_call","payload":{"text":"read_file [ACTIVE]","stream":{"mode":"snapshot","item_id":"tool_1"},"raw":{"event":"step_update","step_update":{"tool_name":"read_file","tool_info":{"parameters":{"Path":"/workspaces/baize/README.md"}}}}}}),
            json!({"id":"4","provider_id":"antigravity","event_type":"session.agent.tool_call","payload":{"text":"read_file [DONE]\n88 lines\nFULL_OUTPUT_TAIL","stream":{"mode":"snapshot","item_id":"tool_1"},"raw":{"event":"step_update","step_update":{"tool_name":"read_file","tool_info":{"parameters":{"Path":"/workspaces/baize/README.md"}}}}}}),
            json!({"id":"5","provider_id":"antigravity","event_type":"session.agent.output","payload":{"text":"Baize 是一个保留开发上下文的工具。\n\n## 项目架构\nTUI → daemon → providers\n\n**目标**：让 agent 可以更换，开发上下文能够延续。","stream":{"mode":"delta","item_id":"answer"}}}),
            json!({"id":"6","provider_id":"antigravity","timestamp":"2026-11-22T12:00:08.200Z","event_type":"session.agent.completed","payload":{"success":true}}),
        ]
    }

    fn live(events: &[Value]) -> TuiState {
        let mut state = TuiState::default();
        state.transcript.clear();
        state.workspace = "/workspaces/baize".into();
        state.active_provider = Some("antigravity".into());
        state.providers = vec!["antigravity".into()];
        for event in events {
            apply_prompt_stream_event(&mut state, event);
        }
        state
    }

    fn text(state: &TuiState, width: u16) -> String {
        transcript_text_for_width(state, width)
            .lines
            .iter()
            .map(Line::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn folded_process_keeps_tool_details_out_of_plain_markdown_answer() {
        let state = live(&fixture());
        let output = text(&state, 100);
        assert!(output.contains("1 项操作") && output.contains("8.2s"));
        assert_eq!(output.matches("Baize 是一个").count(), 1);
        assert!(output.contains("项目架构"));
        for hidden in [
            "read_file",
            "FULL_OUTPUT_TAIL",
            "我会先阅读",
            "│ Baize",
            "## 项目",
            "**目标**",
        ] {
            assert!(
                !output.contains(hidden),
                "unexpected visible detail: {hidden}"
            );
        }
        let mut terminal = Terminal::new(TestBackend::new(112, 38)).unwrap();
        terminal.draw(|frame| render(frame, &state)).unwrap();
        assert!(format!("{:?}", terminal.backend().buffer()).contains("daemon"));
        if let Ok(directory) = std::env::var("BAIZE_TUI_CAPTURE_DIR") {
            let state = if let Ok(path) = std::env::var("BAIZE_TUI_REPLAY_EVENTS") {
                let mut recorded = state.clone();
                let events: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
                replay_session(&mut recorded, &events).unwrap();
                if let Some(workspace) = events["workspace"].as_str() {
                    recorded.workspace = workspace.into();
                }
                recorded.daemon_status = "daemon: recorded replay".into();
                recorded.session_id = events["events"][0]["session_id"]
                    .as_str()
                    .map(str::to_owned);
                recorded.ui.session_status = "Recorded".into();
                recorded
            } else {
                state
            };
            for (name, expanded, details) in [
                ("completed", false, false),
                ("expanded", true, false),
                ("detail", true, true),
            ] {
                let mut state = state.clone();
                state.ui.focus = Focus::Conversation;
                let turn = &mut state.ui.output.turns[0];
                turn.expanded = expanded;
                turn.details = details;
                turn.selected = 1;
                state.scroll_to_top();
                let captured = terminal.draw(|frame| render(frame, &state)).unwrap();
                let cells: Vec<_> = captured.buffer.content.iter().map(|cell| json!({"symbol":cell.symbol(),"fg":format!("{:?}",cell.fg),"bg":format!("{:?}",cell.bg),"bold":cell.modifier.contains(Modifier::BOLD)})).collect();
                std::fs::create_dir_all(&directory).unwrap();
                std::fs::write(
                    Path::new(&directory).join(format!("{name}.json")),
                    serde_json::to_vec(&json!({"width":112,"height":38,"cells":cells})).unwrap(),
                )
                .unwrap();
            }
        }
    }

    #[test]
    fn native_tool_updates_merge_and_details_keep_complete_raw_output() {
        let mut state = live(&fixture());
        assert_eq!(
            state.ui.output.turns[0]
                .items
                .iter()
                .filter(|item| item.role == OutputRole::Tool)
                .count(),
            1
        );
        process_key(&mut state, KeyEvent::new(KeyCode::F(9), KeyModifiers::NONE));
        assert!(text(&state, 100).contains("读取 README.md"));
        assert!(!text(&state, 100).contains("FULL_OUTPUT_TAIL"));
        process_key(&mut state, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        process_key(
            &mut state,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );
        let output = text(&state, 100);
        assert!(output.contains("FULL_OUTPUT_TAIL") && output.contains("Path"));
        assert!(output.contains("Baize 是一个"));
    }

    #[test]
    fn replay_and_reconnect_match_live_output_across_turns_with_reused_tool_ids() {
        let mut events = fixture();
        let mut second = fixture();
        for event in &mut second {
            event["id"] = json!(format!("second_{}", event["id"].as_str().unwrap()));
        }
        events.extend(second);
        let live = live(&events);
        assert_eq!(live.ui.output.turns.len(), 2);
        let mut restored = TuiState {
            workspace: "/workspaces/baize".into(),
            ..TuiState::default()
        };
        replay_session(&mut restored, &json!({"events":events})).unwrap();
        assert_eq!(text(&restored, 100), text(&live, 100));
        let before = text(&restored, 100);
        for event in &events {
            apply_prompt_stream_event(&mut restored, event);
        }
        assert_eq!(text(&restored, 100), before);
    }

    #[test]
    fn failure_warning_stays_visible_when_folded_and_narrow_output_is_safe() {
        let mut state = live(&fixture()[..3]);
        apply_prompt_stream_event(
            &mut state,
            &json!({"id":"failure","event_type":"session.agent.tool_call","payload":{"text":"read_file [failed]\nPermission denied","stream":{"item_id":"tool_1","mode":"snapshot"}}}),
        );
        assert!(text(&state, 80).contains("read_file [failed]"));
        finish_output_turn(&mut state, "canceled", None);
        assert!(text(&state, 80).contains("provider 可能仍在运行"));
        let mut terminal = Terminal::new(TestBackend::new(48, 16)).unwrap();
        terminal.draw(|frame| render(frame, &state)).unwrap();
        assert!(!text(&state, 20).is_empty());
    }
}
