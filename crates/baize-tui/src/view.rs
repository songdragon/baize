use super::*;
use ratatui::widgets::BorderType;

pub(super) const BG: Color = Color::Rgb(18, 22, 30);
pub(super) const PANEL: Color = Color::Rgb(24, 29, 39);
pub(super) const TEXT: Color = Color::Rgb(218, 225, 236);
pub(super) const ACCENT: Color = Color::Rgb(109, 211, 194);

pub(super) fn panel_block(title: &str, focused: bool) -> Block<'static> {
    Block::default()
        .title(format!(" {title} "))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(if focused {
            ACCENT
        } else {
            Color::Rgb(56, 66, 82)
        }))
        .padding(Padding::horizontal(1))
}

pub fn render(frame: &mut Frame<'_>, state: &TuiState) {
    let area = frame.area();
    frame.render_widget(
        Block::default().style(Style::default().bg(BG).fg(TEXT)),
        area,
    );
    if area.width < 36 || area.height < 10 {
        render_resize_screen(frame, area, state);
        return;
    }
    let (_, cursor_row, _) = editor_rows(state, area.width.saturating_sub(6));
    let composer_height = (cursor_row as u16 + 4)
        .clamp(4, 8)
        .min(area.height.saturating_sub(6));
    let chunks = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(3),
        Constraint::Length(composer_height),
        Constraint::Length(1),
    ])
    .split(area);
    render_header(frame, chunks[0], state);
    let inspector = state.detail_mode
        || !state.pending_permissions.is_empty()
        || state.pending_handoff_id.is_some();
    let sidebar = area.width >= 120;
    let (main, side) = if sidebar {
        let parts =
            Layout::horizontal([Constraint::Length(25), Constraint::Min(36)]).split(chunks[1]);
        render_sessions(frame, parts[0], state);
        (parts[1], Some(parts[0]))
    } else {
        (chunks[1], None)
    };
    if inspector && area.width >= 100 {
        let parts = Layout::horizontal([
            Constraint::Min(35),
            Constraint::Length(if area.width >= 150 { 49 } else { 40 }),
        ])
        .split(main);
        render_transcript(frame, parts[0], state);
        render_inspector(frame, parts[1], state);
    } else if inspector && state.ui.focus == Focus::Inspector {
        render_inspector(frame, main, state);
    } else if state.ui.focus == Focus::Sessions && side.is_none() {
        render_sessions(frame, main, state);
    } else {
        render_transcript(frame, main, state);
    }
    render_composer(frame, chunks[2], state);
    let status = state.ui.draft_error.as_deref().or(state.ui.notice.as_deref()).unwrap_or(
        "F9 process · F6 focus · Space fold / Enter detail when focused · Esc compose · F1 help",
    );
    frame.render_widget(
        Paragraph::new(status).style(Style::default().fg(
            if state.ui.notice.is_some() || state.ui.draft_error.is_some() {
                Color::Yellow
            } else {
                Color::Gray
            },
        )),
        chunks[3],
    );
    if let Some(overlay) = state.ui.overlay {
        render_overlay(frame, area, state, overlay);
    }
}

fn render_header(frame: &mut Frame<'_>, area: Rect, state: &TuiState) {
    let active = state.active_provider.as_deref().unwrap_or("none");
    let target = state.selected_provider();
    let switching = if active != target && active != "none" {
        format!("  → {target} next turn")
    } else {
        String::new()
    };
    let title = Line::from(vec![
        Span::styled(
            " BAIZE ",
            Style::default()
                .bg(ACCENT)
                .fg(BG)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            short_text(&one_line(&state.workspace), 42),
            Style::default().fg(TEXT),
        ),
        Span::styled(
            format!("   {active} · CLI{switching}"),
            Style::default().fg(ACCENT),
        ),
        Span::styled(
            format!("   {}", state.activity_status),
            Style::default().fg(activity_color(&state.activity_status)),
        ),
    ]);
    let status = Line::from(vec![
        Span::styled(
            format!(
                " {}",
                state.session_id.as_deref().unwrap_or("New conversation")
            ),
            muted_style(),
        ),
        Span::styled(
            format!(
                "  · {}  · {}",
                state.ui.session_status,
                state.command_policy_label()
            ),
            Style::default().fg(Color::Gray),
        ),
        Span::styled(
            format!(
                "  · {}",
                state
                    .background_status
                    .as_deref()
                    .unwrap_or(&state.daemon_status)
            ),
            muted_style(),
        ),
    ]);
    frame.render_widget(Paragraph::new(vec![title, status]), area);
}

fn render_sessions(frame: &mut Frame<'_>, area: Rect, state: &TuiState) {
    let mut lines = vec![
        Line::styled("Ctrl-O search · ^N new", muted_style()),
        Line::raw(""),
    ];
    let selected = state.selected_session_index;
    // Keep the keyboard selection visible even for long session lists.
    let capacity = usize::from(area.height.saturating_sub(5)) / 3;
    let indices: Vec<_> = (0..state.recent_sessions.len()).rev().collect();
    let position = indices.iter().position(|i| *i == selected).unwrap_or(0);
    let start = position.saturating_sub(capacity.saturating_sub(1));
    for index in indices.into_iter().skip(start).take(capacity.max(1)) {
        let session = &state.recent_sessions[index];
        let current = state.session_id.as_deref() == Some(&session.id);
        let marker = if index == selected { "›" } else { " " };
        lines.push(Line::styled(
            format!(
                "{marker} {}",
                short_text(
                    &session.objective,
                    usize::from(area.width.saturating_sub(7))
                )
            ),
            if index == selected {
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(TEXT)
            },
        ));
        lines.push(Line::styled(
            format!(
                "  {} · {}{}",
                session.active_provider_id.as_deref().unwrap_or("none"),
                session.status,
                if current { " *" } else { "" }
            ),
            muted_style(),
        ));
        lines.push(Line::raw(""));
    }
    if state.recent_sessions.is_empty() {
        lines.push(Line::raw("No saved conversations"));
        lines.push(Line::styled("Send a message to begin", muted_style()));
    }
    frame.render_widget(
        Paragraph::new(lines).block(panel_block(
            "Conversations",
            state.ui.focus == Focus::Sessions,
        )),
        area,
    );
}

pub(super) fn render_composer(frame: &mut Frame<'_>, area: Rect, state: &TuiState) {
    let block = panel_block(
        "Message · Enter send · Alt-Enter newline",
        state.ui.focus == Focus::Composer,
    )
    .style(Style::default().bg(PANEL));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }
    let text_width = inner.width.saturating_sub(2);
    let (rows, row, col) = editor_rows(state, text_width);
    let visible = usize::from(inner.height.saturating_sub(1).max(1));
    let top = row.saturating_sub(visible.saturating_sub(1));
    let mut lines: Vec<_> = rows
        .into_iter()
        .skip(top)
        .take(visible)
        .enumerate()
        .map(|(i, line)| {
            Line::from(vec![
                Span::styled(
                    if i + top == 0 { "› " } else { "  " },
                    Style::default().fg(ACCENT),
                ),
                Span::raw(line),
            ])
        })
        .collect();
    if state.input.is_empty() {
        lines[0] = Line::styled("› Ask Baize to work on…", Style::default().fg(Color::Gray));
    }
    let attachments = state
        .image_paths
        .iter()
        .map(|p| {
            p.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>()
        .join(", ");
    let label = format!(
        "{} · Tab provider · {} image(s) {}",
        state.selected_provider(),
        state.image_paths.len(),
        attachments
    );
    while lines.len() < visible {
        lines.push(Line::raw(""));
    }
    lines.push(Line::styled(label, muted_style()));
    frame.render_widget(Paragraph::new(lines), inner);
    if state.ui.focus == Focus::Composer && state.ui.overlay.is_none() {
        frame.set_cursor_position(Position::new(
            (inner.x + 2 + col).min(inner.right().saturating_sub(1)),
            (inner.y + (row - top) as u16).min(inner.bottom().saturating_sub(1)),
        ));
    }
}

pub(super) fn conversation_wrap_width(state: &TuiState, width: u16) -> u16 {
    let sidebar = if width >= 120 { 25 } else { 0 };
    let inspector = state.detail_mode
        || !state.pending_permissions.is_empty()
        || state.pending_handoff_id.is_some();
    let details = if inspector && width >= 100 {
        if width >= 150 {
            49
        } else {
            40
        }
    } else {
        0
    };
    width.saturating_sub(sidebar + details + 4).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::{Backend, TestBackend};
    #[test]
    fn quit_dialog_explains_client_exit_and_only_warns_about_existing_drafts() {
        let mut state = TuiState::default();
        open_overlay(&mut state, Overlay::Quit);
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| render(frame, &state)).unwrap();
        let text = format!("{:?}", terminal.backend().buffer());
        assert!(text.contains("Close this terminal client?"));
        assert!(text.contains("provider execution remain active"));
        assert!(text.contains("Enter quit") && text.contains("Esc stay"));
        assert!(!text.contains("will be lost"));
        state.ui.drafts.insert(
            "other".into(),
            ("".into(), 0, vec![PathBuf::from("/tmp/image.png")]),
        );
        terminal.draw(|frame| render(frame, &state)).unwrap();
        assert!(format!("{:?}", terminal.backend().buffer())
            .contains("drafts and attachments will be lost"));
    }
    #[test]
    fn renders_overlays_empty_results_and_small_terminals_without_changing_draft() {
        let mut state = TuiState {
            input: "unsent 中文".into(),
            ..TuiState::default()
        };
        for (width, height) in [(1, 1), (35, 9), (36, 10), (80, 24), (150, 38)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            for overlay in [
                Overlay::Help,
                Overlay::Commands,
                Overlay::Sessions,
                Overlay::Quit,
            ] {
                open_overlay(&mut state, overlay);
                state.ui.query = "no matching result".into();
                terminal.draw(|frame| render(frame, &state)).unwrap();
                if width >= 80 {
                    assert!(
                        format!("{:?}", terminal.backend().buffer()).contains(match overlay {
                            Overlay::Help => "Keyboard help · ↑↓ scroll",
                            Overlay::Commands => "No matches",
                            Overlay::Sessions => "No matches",
                            Overlay::Quit => "Enter quit",
                            Overlay::Cancel => "Enter cancel",
                        })
                    );
                }
            }
        }
        assert_eq!(state.input, "unsent 中文");
    }
    #[test]
    fn narrow_terminal_uses_focused_panel_and_keeps_editor_cursor_in_bounds() {
        let mut state = TuiState {
            input: "中文👩‍💻\nsecond line\nthird".into(),
            detail_mode: true,
            ..TuiState::default()
        };
        state.ui.focus = Focus::Inspector;
        state.ui.panel = Panel::Changes;
        let mut terminal = Terminal::new(TestBackend::new(50, 18)).unwrap();
        terminal.draw(|frame| render(frame, &state)).unwrap();
        assert!(format!("{:?}", terminal.backend().buffer()).contains("WORKSPACE CHANGES"));
        state.ui.focus = Focus::Composer;
        terminal.draw(|frame| render(frame, &state)).unwrap();
        let cursor = terminal.backend_mut().get_cursor_position().unwrap();
        assert!(cursor.x < 50 && cursor.y < 18);
        assert!(format!("{:?}", terminal.backend().buffer()).contains("Conversation"));
    }
}
