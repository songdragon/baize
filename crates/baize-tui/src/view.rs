use super::*;
use ratatui::widgets::{BorderType, Clear};

const BG: Color = Color::Rgb(18, 22, 30);
const PANEL: Color = Color::Rgb(24, 29, 39);
const TEXT: Color = Color::Rgb(218, 225, 236);
const ACCENT: Color = Color::Rgb(109, 211, 194);

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
        frame.render_widget(
            Paragraph::new("Baize\nResize terminal to at least 36 × 10\nCtrl-Q to quit")
                .wrap(Wrap { trim: false }),
            area,
        );
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
    let status =
        state.ui.notice.as_deref().unwrap_or(
            "F1 help  Ctrl-K commands  Ctrl-O sessions  F6 focus  F7 inspect  F8 details",
        );
    frame.render_widget(
        Paragraph::new(status).style(Style::default().fg(if state.ui.notice.is_some() {
            Color::Yellow
        } else {
            Color::Gray
        })),
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

fn render_overlay(frame: &mut Frame<'_>, area: Rect, state: &TuiState, overlay: Overlay) {
    let width = area.width.saturating_sub(4).min(90);
    let height = area
        .height
        .saturating_sub(2)
        .min(if overlay == Overlay::Quit { 9 } else { 25 });
    let popup = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, popup);
    let title = match overlay {
        Overlay::Help => "Keyboard help · ↑↓ scroll",
        Overlay::Commands => "Commands",
        Overlay::Sessions => "Open conversation",
        Overlay::Quit => "Leave Baize?",
    };
    let mut lines = vec![];
    match overlay {
        Overlay::Quit => {
            lines.push(Line::raw(
                "Your unsent draft will be lost when this client exits.",
            ));
            lines.push(Line::raw(
                "The daemon and any provider execution remain active.",
            ));
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                "Enter quit · Esc stay",
                Style::default().fg(ACCENT),
            ));
        }
        Overlay::Help => {
            lines.push(Line::raw(
                "One conversation. Interchangeable coding agents.",
            ));
            lines.push(Line::styled(
                "Enter send · Alt-Enter / Ctrl-J newline · arrows edit",
                Style::default().fg(ACCENT),
            ));
            lines.push(Line::raw(
                "F6 focus · F7 inspector tab · F8 details · Esc return",
            ));
            lines.push(Line::raw(
                "PgUp/PgDn scroll conversation or focused inspector",
            ));
            lines.push(Line::raw(
                "Tab target provider · next turn keeps this Baize session",
            ));
            lines.push(Line::raw(
                "Ctrl-V image · /image path · /unimage clears attachments",
            ));
            lines.push(Line::raw(""));
            for (_, label, key) in COMMANDS {
                lines.push(Line::raw(format!("{key:<9} {label}")));
            }
        }
        Overlay::Commands | Overlay::Sessions => {
            lines.push(Line::styled(
                format!("Search: {}▏", state.ui.query),
                Style::default().fg(ACCENT),
            ));
            lines.push(Line::styled(
                "↑↓ select · Enter open · Esc dismiss",
                muted_style(),
            ));
            lines.push(Line::raw(""));
            let entries: Vec<String> = if overlay == Overlay::Commands {
                command_matches(state)
                    .iter()
                    .map(|i| format!("{}  {}", COMMANDS[*i].1, COMMANDS[*i].2))
                    .collect()
            } else {
                session_matches(state)
                    .iter()
                    .map(|i| {
                        let s = &state.recent_sessions[*i];
                        format!(
                            "{} · {} · {}",
                            s.objective,
                            s.status,
                            s.active_provider_id.as_deref().unwrap_or("none")
                        )
                    })
                    .collect()
            };
            let visible = usize::from(height.saturating_sub(6)).max(1);
            let start = state.ui.menu_index.saturating_sub(visible - 1);
            for (i, entry) in entries.iter().enumerate().skip(start).take(visible) {
                lines.push(Line::styled(
                    format!(
                        "{} {}",
                        if i == state.ui.menu_index { "›" } else { " " },
                        entry
                    ),
                    if i == state.ui.menu_index {
                        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(TEXT)
                    },
                ));
            }
            if entries.is_empty() {
                lines.push(Line::raw("No matches"));
            }
        }
    }
    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default().bg(PANEL).fg(TEXT))
            .block(panel_block(title, true))
            .scroll((
                if overlay == Overlay::Help {
                    state.ui.menu_index as u16
                } else {
                    0
                },
                0,
            ))
            .wrap(Wrap { trim: false }),
        popup,
    );
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
