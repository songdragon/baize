use super::*;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

pub(super) fn cursor_byte(state: &TuiState) -> usize {
    let cursor = state.ui.cursor.min(state.input.len());
    state
        .input
        .grapheme_indices(true)
        .map(|(i, _)| i)
        .chain(std::iter::once(state.input.len()))
        .take_while(|i| *i <= cursor)
        .last()
        .unwrap_or(0)
}

pub(super) fn insert_text(state: &mut TuiState, text: &str) {
    let cursor = cursor_byte(state);
    // Control sequences from pasted text must never become terminal commands.
    let clean: String = text
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .chars()
        .filter(|ch| !ch.is_control() || matches!(ch, '\n' | '\t'))
        .collect();
    state.input.insert_str(cursor, &clean);
    state.ui.cursor = cursor + clean.len();
}

pub(super) fn edit_key(state: &mut TuiState, key: KeyEvent) -> bool {
    let cursor = cursor_byte(state);
    let previous = state.input[..cursor]
        .grapheme_indices(true)
        .next_back()
        .map(|(i, _)| i)
        .unwrap_or(0);
    let next = state.input[cursor..]
        .graphemes(true)
        .next()
        .map(|s| cursor + s.len())
        .unwrap_or(cursor);
    match key.code {
        KeyCode::Left => state.ui.cursor = previous,
        KeyCode::Right => state.ui.cursor = next,
        KeyCode::Up | KeyCode::Down => {
            let start = state.input[..cursor]
                .rfind('\n')
                .map(|i| i + 1)
                .unwrap_or(0);
            let column = state.input[start..cursor].graphemes(true).count();
            let target = if key.code == KeyCode::Up {
                if start == 0 {
                    0
                } else {
                    state.input[..start - 1]
                        .rfind('\n')
                        .map(|i| i + 1)
                        .unwrap_or(0)
                }
            } else {
                state.input[cursor..]
                    .find('\n')
                    .map(|i| cursor + i + 1)
                    .unwrap_or(start)
            };
            let end = state.input[target..]
                .find('\n')
                .map(|i| target + i)
                .unwrap_or(state.input.len());
            state.ui.cursor = state.input[target..end]
                .grapheme_indices(true)
                .nth(column)
                .map(|(i, _)| target + i)
                .unwrap_or(end);
        }
        KeyCode::Home => {
            state.ui.cursor = state.input[..cursor]
                .rfind('\n')
                .map(|i| i + 1)
                .unwrap_or(0)
        }
        KeyCode::End => {
            state.ui.cursor = state.input[cursor..]
                .find('\n')
                .map(|i| cursor + i)
                .unwrap_or(state.input.len())
        }
        KeyCode::Backspace => {
            state.input.replace_range(previous..cursor, "");
            state.ui.cursor = previous;
        }
        KeyCode::Delete => {
            state.input.replace_range(cursor..next, "");
            state.ui.cursor = cursor;
        }
        KeyCode::Enter
            if key
                .modifiers
                .intersects(KeyModifiers::ALT | KeyModifiers::SHIFT) =>
        {
            insert_text(state, "\n")
        }
        KeyCode::Char(ch)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            insert_text(state, &ch.to_string())
        }
        _ => return false,
    }
    true
}

// Render the same visual rows used for cursor placement, including CJK and emoji clusters.
pub(super) fn editor_rows(state: &TuiState, width: u16) -> (Vec<String>, usize, u16) {
    let width = usize::from(width.max(2));
    let cursor = cursor_byte(state);
    let mut rows = vec![String::new()];
    let (mut row, mut col) = (0, 0);
    let (mut cursor_row, mut cursor_col) = (0, 0);
    for (offset, grapheme) in state.input.grapheme_indices(true) {
        if offset == cursor {
            cursor_row = row;
            cursor_col = col;
        }
        if grapheme == "\n" {
            rows.push(String::new());
            row += 1;
            col = 0;
            continue;
        }
        let display = if grapheme == "\t" { "    " } else { grapheme };
        let size = display.width();
        if col + size >= width && col > 0 {
            rows.push(String::new());
            row += 1;
            col = 0;
        }
        if offset == cursor {
            cursor_row = row;
            cursor_col = col;
        }
        rows[row].push_str(display);
        col += size;
    }
    if cursor == state.input.len() {
        cursor_row = row;
        cursor_col = col;
    }
    (rows, cursor_row, cursor_col.min(width - 1) as u16)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn edits_graphemes_in_middle_and_preserves_multiline_paste() {
        let mut state = TuiState::default();
        insert_text(&mut state, "中e\u{301}👩‍💻\nlast");
        state.ui.cursor = "中e\u{301}👩‍💻".len();
        edit_key(
            &mut state,
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
        );
        assert_eq!(state.input, "中e\u{301}\nlast");
        edit_key(&mut state, KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        insert_text(&mut state, "X");
        assert_eq!(state.input, "中Xe\u{301}\nlast");
        insert_text(&mut state, "\r\n  code\u{1b}[31m");
        assert!(state.input.contains("\n  code[31m"));
    }
    #[test]
    fn handles_empty_invalid_cursor_and_wraps_wide_text() {
        let mut state = TuiState::default();
        edit_key(
            &mut state,
            KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE),
        );
        insert_text(&mut state, "中文abcd");
        state.ui.cursor = 1;
        assert_eq!(cursor_byte(&state), 0);
        state.ui.cursor = usize::MAX;
        let (rows, row, col) = editor_rows(&state, 6);
        assert_eq!(rows, vec!["中文a", "bcd"]);
        assert_eq!((row, col), (1, 3));
    }
}
