use super::*;
use ratatui::widgets::Clear;

pub(super) fn render_resize_screen(frame: &mut Frame<'_>, area: Rect, state: &TuiState) {
    let text = match state.ui.overlay {
        Some(Overlay::Quit) => format!(
            "Leave Baize?\nEnter quit\nEsc stay\nDrafts: {}\nDaemon/provider stay active.",
            if state.ui.drafts_persisted {
                "saved locally"
            } else {
                "not saved"
            }
        ),
        Some(Overlay::Cancel) => {
            let mut text = format!(
                "Stop current execution?\nEnter stop\nEsc stay\n{}\nWait for confirmed stop evidence.",
                state
                    .ui
                    .cancel_session_id
                    .as_deref()
                    .unwrap_or("No session selected")
            );
            if let Some(notice) = state.ui.notice.as_deref() {
                text.push('\n');
                text.push_str(notice);
            }
            text
        }
        _ => match state.ui.notice.as_deref() {
            Some(notice) => format!("Baize\nResize terminal to at least 36 × 10\n{notice}\nCtrl-Q / Ctrl-C: review exit"),
            None => {
                "Baize\nResize terminal to at least 36 × 10\nCtrl-Q / Ctrl-C: review exit".into()
            }
        },
    };
    frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), area);
}

pub(super) fn render_overlay(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &TuiState,
    overlay: Overlay,
) {
    let width = area.width.saturating_sub(4).min(90);
    let height =
        area.height
            .saturating_sub(2)
            .min(if matches!(overlay, Overlay::Quit | Overlay::Cancel) {
                9
            } else {
                25
            });
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
        Overlay::Cancel => "Stop current execution?",
    };
    let mut lines = vec![];
    match overlay {
        Overlay::Cancel => {
            lines.push(Line::raw(
                state.ui.cancel_session_id.clone().unwrap_or_default(),
            ));
            lines.push(Line::raw("Wait for confirmed stop evidence."));
            lines.push(Line::raw("Conversation and partial changes are kept."));
            lines.push(Line::styled(
                "Enter stop · Esc stay",
                Style::default().fg(ACCENT),
            ));
        }
        Overlay::Quit => {
            let has_draft = !state.input.is_empty()
                || !state.image_paths.is_empty()
                || !state.ui.recovery_drafts.is_empty()
                || state
                    .ui
                    .drafts
                    .values()
                    .any(|(input, _, images)| !input.is_empty() || !images.is_empty());
            lines.push(Line::raw(if has_draft && state.ui.drafts_persisted {
                "Drafts are saved locally for the next launch."
            } else if has_draft {
                "Your unsent drafts and attachments will be lost."
            } else {
                "Close this terminal client?"
            }));
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
                "F6 / Shift-Tab focus forward / back · F7 tab · F8 details · Esc return",
            ));
            lines.push(Line::raw(
                "F9 process · Space fold · ↑↓ operation · Enter details · ←→ turn",
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
            lines.push(Line::raw("/remember <key> <text> saves/replaces a user requirement; /forget <key> revokes it"));
            lines.push(Line::raw(
                "/requirements lists sources; F5 previews the selected target and current draft",
            ));
            lines.push(Line::raw(
                "/reset-provider <provider> clears an invalid native reference without replaying",
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

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    fn draw(state: &TuiState, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| render(frame, state)).unwrap();
        format!("{:?}", terminal.backend().buffer())
    }

    #[test]
    fn compact_exit_keeps_confirmation_visible_and_escape_returns_to_resize_guidance() {
        let mut state = TuiState {
            input: "draft".into(),
            ..TuiState::default()
        };
        let (mut prompt, mut task) = (None, None);
        for shortcut in ['q', 'c', 'c'] {
            assert!(!handle_key(
                &mut state,
                KeyEvent::new(KeyCode::Char(shortcut), KeyModifiers::CONTROL),
                &mut prompt,
                &mut task
            )
            .unwrap());
            for (width, height) in [(35, 9), (35, 5), (20, 8)] {
                let text = draw(&state, width, height);
                assert!(text.contains("Leave Baize?"));
                assert!(text.contains("Enter quit") && text.contains("Esc stay"));
                assert!(text.contains("not saved"));
            }
        }
        state.ui.drafts_persisted = true;
        assert!(draw(&state, 35, 9).contains("saved locally"));
        assert!(draw(&state, 35, 9).contains("Daemon/provider stay active"));
        handle_key(
            &mut state,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &mut prompt,
            &mut task,
        )
        .unwrap();
        assert!(draw(&state, 35, 9).contains("Resize terminal"));
        assert_eq!(state.input, "draft");
    }

    #[test]
    fn compact_cancel_shows_target_and_actions_and_zero_sized_screens_remain_safe() {
        let mut state = TuiState {
            session_id: Some("task_target".into()),
            ..TuiState::default()
        };
        request_cancel_confirmation(&mut state);
        let text = draw(&state, 35, 9);
        for expected in [
            "Stop current execution?",
            "Enter stop",
            "Esc stay",
            "task_target",
            "Wait for confirmed stop evidence",
        ] {
            assert!(text.contains(expected));
        }
        assert!(!confirm_cancel(&mut state, true));
        assert!(draw(&state, 35, 9).contains("Wait for the current"));
        for size in [(0, 0), (1, 1), (10, 2)] {
            draw(&state, size.0, size.1);
        }
    }
}
