use super::*;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

pub(super) fn render_transcript(frame: &mut Frame<'_>, area: Rect, state: &TuiState) {
    let wrap_width = transcript_wrap_width(area);
    let text = transcript_text_for_width(state, wrap_width);
    let line_count = text.lines.len();
    let visible_height = usize::from(area.height.saturating_sub(2));
    let max_scroll = line_count.saturating_sub(visible_height);
    let effective_scroll = if state.scroll_offset > max_scroll {
        max_scroll
    } else {
        state.scroll_offset
    };
    let is_scrolled = effective_scroll > 0;
    let viewport_top = max_scroll.saturating_sub(effective_scroll);
    let title = if is_scrolled {
        let current_top = viewport_top + 1;
        format!(
            "Conversation ({}-{}/{})",
            current_top,
            (current_top + visible_height.saturating_sub(1)).min(line_count),
            line_count
        )
    } else {
        "Conversation".to_string()
    };
    frame.render_widget(
        Paragraph::new(Text::from(
            text.lines
                .into_iter()
                .skip(viewport_top)
                .take(visible_height)
                .collect::<Vec<_>>(),
        ))
        .block(panel_block(
            &title,
            matches!(state.ui.focus, Focus::Composer | Focus::Conversation),
        )),
        area,
    );
}

pub(super) fn transcript_wrap_width(area: Rect) -> u16 {
    area.width.saturating_sub(4).max(1)
}

pub(super) fn transcript_text_for_width(state: &TuiState, wrap_width: u16) -> Text<'static> {
    let mut lines = vec![];
    let mut turns = state.ui.output.turns.iter().enumerate().peekable();
    for (index, line) in state.transcript.iter().enumerate() {
        if let Some((turn_index, turn)) = turns.peek().filter(|(_, turn)| turn.anchor == index) {
            lines.extend(output_turn_lines(
                turn,
                wrap_width,
                state.ui.focus == Focus::Conversation && state.ui.output.selected == *turn_index,
                &state.workspace,
            ));
            turns.next();
        } else {
            lines.extend(
                line.split('\n')
                    .flat_map(|part| wrap_transcript_raw_line(part, wrap_width))
                    .map(|line| transcript_line(&line)),
            );
        }
    }
    Text::from(lines)
}

pub(super) fn wrap_transcript_raw_line(line: &str, wrap_width: u16) -> Vec<String> {
    let wrap_width = usize::from(wrap_width.max(1));
    if line.trim().is_empty() {
        return vec![line.to_string()];
    }
    if line.starts_with("╭─") || line.starts_with("╰─") || transcript_section_label(line).is_some()
    {
        return vec![line.to_string()];
    }

    let (raw_prefix, content, render_prefix_width) = if let Some(content) = line.strip_prefix("  ")
    {
        ("  ", content, 4usize)
    } else if let Some(content) = line.strip_prefix("> ") {
        ("> ", content, 6usize)
    } else if line.starts_with("turn result:")
        || line.starts_with("prompt result:")
        || line.starts_with("workspace clean")
        || line.starts_with("workspace dirty")
        || line.starts_with("changed files:")
    {
        ("", line, 3usize)
    } else {
        ("", line, 0usize)
    };
    let available = wrap_width.saturating_sub(render_prefix_width).max(1);
    let chunks = wrap_text_chunks(content, available);
    chunks
        .into_iter()
        .map(|chunk| format!("{raw_prefix}{chunk}"))
        .collect()
}

pub(super) fn wrap_text_chunks(text: &str, width: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut size = 0;
    for grapheme in text.graphemes(true) {
        let display = if grapheme == "\t" { "    " } else { grapheme };
        let next = display.width();
        if !current.is_empty() && size + next > width {
            let boundary = current
                .rfind(' ')
                .filter(|i| !current[..*i].trim().is_empty());
            if let Some(index) = boundary {
                let tail = current[index + 1..].to_string();
                chunks.push(current[..index].to_string());
                current = tail;
                size = current.width();
            } else {
                chunks.push(std::mem::take(&mut current));
                size = 0;
            }
            if !current.is_empty() && size + next > width {
                chunks.push(std::mem::take(&mut current));
                size = 0;
            }
        }
        current.push_str(display);
        size += next;
    }
    chunks.push(current);
    chunks
}

pub(super) fn transcript_line(line: &str) -> Line<'static> {
    if line.trim().is_empty() {
        return Line::raw("");
    }
    if line.starts_with("╭─") {
        return Line::from(Span::styled(
            line.to_string(),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));
    }
    if line.starts_with("╰─") {
        return Line::from(Span::styled(line.to_string(), muted_style()));
    }
    if let Some(label) = transcript_section_label(line) {
        return Line::from(vec![
            Span::styled("╭─ ", muted_style()),
            Span::styled(
                label.to_string(),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" ", muted_style()),
            Span::styled("────────", muted_style()),
        ]);
    }
    if let Some(prompt) = line.strip_prefix("> ") {
        return Line::from(vec![
            Span::styled("you ", muted_style()),
            Span::styled("› ", accent_style()),
            Span::raw(prompt.to_string()),
        ]);
    }

    let lower = line.to_ascii_lowercase();
    if line.starts_with("turn result:")
        || line.starts_with("prompt result:")
        || line.starts_with("workspace clean")
    {
        Line::from(vec![
            Span::styled("╰─ ", muted_style()),
            Span::styled(line.to_string(), Style::default().fg(Color::Green)),
        ])
    } else if line.starts_with("workspace dirty") || line.starts_with("changed files:") {
        Line::from(vec![
            Span::styled("╰─ ", muted_style()),
            Span::styled(line.to_string(), Style::default().fg(Color::Yellow)),
        ])
    } else if lower.contains("error") || lower.contains("failed") {
        Line::from(Span::styled(
            line.to_string(),
            Style::default().fg(Color::Red),
        ))
    } else if line.ends_with(':') {
        Line::from(Span::styled(
            line.to_string(),
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        ))
    } else if line.starts_with("  ") {
        let content = line.strip_prefix("  ").unwrap_or(line);
        Line::from(vec![
            Span::styled("  │ ", muted_style()),
            Span::raw(content.to_string()),
        ])
    } else {
        Line::from(Span::raw(line.to_string()))
    }
}

pub(super) fn transcript_section_label(line: &str) -> Option<&'static str> {
    match line {
        "assistant:" => Some("assistant"),
        "command/tool events:" => Some("tools"),
        "status:" => Some("status"),
        "routes:" => Some("routes"),
        "test results:" => Some("tests"),
        "provider diagnostics:" => Some("provider diagnostics"),
        "provider setup issues:" => Some("provider setup"),
        "sessions:" => Some("sessions"),
        _ => None,
    }
}

pub(super) fn activity_color(status: &str) -> Color {
    if status == "idle" {
        Color::Green
    } else if status == "failed" {
        Color::Red
    } else {
        Color::Yellow
    }
}

pub(super) fn accent_style() -> Style {
    Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD)
}

pub(super) fn muted_style() -> Style {
    Style::default().fg(Color::Rgb(132, 146, 165))
}

pub(super) fn health_style(health: &str) -> Style {
    match health {
        "ok" => Style::default().fg(Color::Green),
        "warn" => Style::default().fg(Color::Yellow),
        "down" => Style::default().fg(Color::Red),
        _ => muted_style(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    #[test]
    fn shows_latest_and_earliest_output_beyond_u16_line_limit() {
        let mut state = TuiState {
            transcript: (0..66000).map(|i| format!("record {i}")).collect(),
            ..TuiState::default()
        };
        let mut terminal = Terminal::new(TestBackend::new(80, 16)).unwrap();
        terminal.draw(|frame| render(frame, &state)).unwrap();
        assert!(format!("{:?}", terminal.backend().buffer()).contains("record 65999"));
        state.scroll_to_top();
        terminal.draw(|frame| render(frame, &state)).unwrap();
        assert!(format!("{:?}", terminal.backend().buffer()).contains("record 0"));
    }
    #[test]
    fn wraps_words_without_splitting_emoji_or_combining_characters() {
        assert_eq!(
            wrap_text_chunks("hello world again", 12),
            vec!["hello world", "again"]
        );
        let wrapped = wrap_text_chunks("中👩‍💻e\u{301}", 3);
        assert_eq!(wrapped.concat(), "中👩‍💻e\u{301}");
        assert!(wrapped.iter().any(|line| line.contains("👩‍💻")));
    }
}
