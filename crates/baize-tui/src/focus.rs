use super::*;

pub(super) fn move_focus(state: &mut TuiState, backwards: bool) {
    state.ui.focus = match (state.ui.focus, backwards) {
        (Focus::Composer, false) | (Focus::Sessions, true) => Focus::Conversation,
        (Focus::Conversation, false) | (Focus::Inspector, true) => Focus::Sessions,
        (Focus::Sessions, false) | (Focus::Composer, true) => Focus::Inspector,
        (Focus::Inspector, false) | (Focus::Conversation, true) => Focus::Composer,
    };
    if state.ui.focus == Focus::Inspector {
        state.detail_mode = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    #[test]
    fn focus_shortcuts_traverse_both_directions_from_every_start_without_changing_drafts() {
        let order = [
            Focus::Composer,
            Focus::Conversation,
            Focus::Sessions,
            Focus::Inspector,
        ];
        for (index, start) in order.iter().enumerate() {
            for (key, step) in [(KeyCode::F(6), 1), (KeyCode::BackTab, 3)] {
                let mut state = TuiState {
                    input: "草稿 👩‍💻".into(),
                    image_paths: vec!["image.png".into()],
                    ..TuiState::default()
                };
                state.ui.cursor = 2;
                state.ui.focus = *start;
                let (mut prompt, mut ui) = (None, None);
                for turn in 1..=4 {
                    handle_key(
                        &mut state,
                        KeyEvent::new(key, KeyModifiers::NONE),
                        &mut prompt,
                        &mut ui,
                    )
                    .unwrap();
                    assert_eq!(state.ui.focus, order[(index + turn * step) % 4]);
                    if state.ui.focus == Focus::Inspector {
                        assert!(state.detail_mode);
                    }
                }
                handle_key(
                    &mut state,
                    KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
                    &mut prompt,
                    &mut ui,
                )
                .unwrap();
                assert_eq!(state.ui.focus, Focus::Composer);
                assert_eq!(state.input, "草稿 👩‍💻");
                assert_eq!(state.ui.cursor, 2);
                assert_eq!(state.image_paths, [PathBuf::from("image.png")]);
                assert!(prompt.is_none() && ui.is_none());
            }
        }
    }

    #[test]
    fn reverse_focus_opens_the_inspector_on_narrow_screens_and_handles_an_empty_session() {
        let mut state = TuiState::default();
        move_focus(&mut state, true);
        for (width, height) in [(36, 10), (80, 24), (112, 36)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| render(frame, &state)).unwrap();
            let text = format!("{:?}", terminal.backend().buffer());
            assert!(text.contains("Session"));
        }
        assert!(state.session_id.is_none() && state.input.is_empty());
    }
}
