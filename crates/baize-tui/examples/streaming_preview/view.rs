use super::model::{Preview, TOOLS};
use baize_tui::TuiState;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Wrap},
    Frame,
};

const BG: Color = Color::Rgb(18, 22, 30);
const TEXT: Color = Color::Rgb(218, 225, 236);
const MUTED: Color = Color::Rgb(132, 146, 165);
const ACCENT: Color = Color::Rgb(109, 211, 194);

pub(super) fn render(frame: &mut Frame<'_>, preview: &Preview) {
    let area = frame.area();
    let mut shell = TuiState::default();
    shell.workspace = "/Users/songdragon/Documents/baize".into();
    shell.providers = vec!["antigravity".into()];
    shell.active_provider = Some("antigravity".into());
    shell.activity_status = match preview.stage {
        0..=4 => "正在查看项目",
        5..=9 => "正在回答",
        _ => "idle",
    }
    .into();
    baize_tui::render(frame, &shell);
    if area.width < 48 || area.height < 16 {
        frame.render_widget(Clear, area);
        frame.render_widget(
            Paragraph::new("请将终端调整到至少 48 × 16\nQ 退出示例")
                .style(Style::default().bg(BG).fg(TEXT)),
            area,
        );
        return;
    }
    let subtitle = Rect::new(area.x, area.y + 1, area.width, 1);
    frame.render_widget(Clear, subtitle);
    frame.render_widget(
        Paragraph::new(" 解释项目  ·  过程折叠预览  ·  离线演示数据")
            .style(Style::default().bg(BG).fg(MUTED)),
        subtitle,
    );
    let conversation = Rect::new(area.x, area.y + 2, area.width, area.height - 7);
    frame.render_widget(Clear, conversation);
    let block = Block::default()
        .title(" Conversation ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(ACCENT))
        .style(Style::default().bg(BG).fg(TEXT))
        .padding(Padding::horizontal(2));
    let inner = block.inner(conversation);
    frame.render_widget(block, conversation);
    let lines = conversation_lines(preview);
    let visual_height: usize = lines
        .iter()
        .map(|line| {
            line.width()
                .max(1)
                .div_ceil(usize::from(inner.width.max(1)))
        })
        .sum();
    let max_scroll = visual_height.saturating_sub(usize::from(inner.height));
    let top = max_scroll.saturating_sub(preview.scroll.min(max_scroll));
    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default().bg(BG).fg(TEXT))
            .wrap(Wrap { trim: false })
            .scroll((top.min(u16::MAX as usize) as u16, 0)),
        inner,
    );
    let composer = Rect::new(area.x, area.bottom() - 5, area.width, 4);
    frame.render_widget(Clear, composer);
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(
                "› 确认展示效果后，再接入正式对话",
                Style::default().fg(MUTED),
            ),
            Line::styled(
                "  离线交互示例 · 不发送请求 · 不影响当前会话",
                Style::default().fg(MUTED),
            ),
        ])
        .block(
            Block::default()
                .title(" Message ")
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Color::Rgb(56, 66, 82)))
                .padding(Padding::horizontal(1)),
        )
        .style(Style::default().bg(Color::Rgb(24, 29, 39)).fg(TEXT)),
        composer,
    );
    let footer = if area.width >= 100 {
        " Space 展开/收起 · ↑↓ 选择 · Enter 详情 · 1 运行中  2 完成  3 展开 · R 重播 · P 暂停 · Q 退出"
    } else {
        " Space 折叠 · Enter 详情 · 1/2/3 状态 · R 重播 · Q 退出"
    };
    frame.render_widget(
        Paragraph::new(footer).style(Style::default().bg(BG).fg(MUTED)),
        Rect::new(area.x, area.bottom() - 1, area.width, 1),
    );
}

fn conversation_lines(preview: &Preview) -> Vec<Line<'static>> {
    let muted = Style::default().fg(MUTED);
    let accent = Style::default().fg(ACCENT);
    let mut lines = vec![
        Line::raw(""),
        Line::from(vec![
            Span::styled("you  ›  ", accent),
            Span::raw("解释下这个项目"),
        ]),
        Line::raw(""),
        Line::raw("我会先阅读项目说明和架构文档，再解释它的目标与当前实现。"),
        Line::raw(""),
    ];
    let running = preview.stage < 5;
    let summary = if preview.failed {
        "1 项操作失败 · 读取 doc/vision.md".to_string()
    } else if running {
        format!(
            "正在查看项目  ·  已完成 {}/5 项  ·  {:.1}s",
            preview.stage,
            preview.stage as f64 * 1.6 + 0.5
        )
    } else {
        "已查看 4 个文件，梳理项目架构  ·  5 项操作  ·  8.2s".into()
    };
    lines.push(Line::from(vec![
        Span::styled(if preview.expanded { "▾ " } else { "▸ " }, accent),
        Span::styled(
            summary,
            if preview.failed {
                Style::default().fg(Color::Red)
            } else {
                muted
            },
        ),
        Span::styled(
            if preview.expanded {
                "  [Space 收起]"
            } else {
                "  [Space 展开]"
            },
            muted,
        ),
    ]));
    if preview.expanded {
        for (index, (action, path, result)) in TOOLS.iter().enumerate() {
            let done = index < preview.stage;
            let active = index == preview.stage;
            let icon = if done {
                "✓"
            } else if active {
                "•"
            } else {
                "·"
            };
            let selected = index == preview.selected;
            lines.push(Line::from(vec![
                Span::styled(if selected { "  › " } else { "    " }, accent),
                Span::styled(format!("{icon} "), if active { accent } else { muted }),
                Span::styled(
                    format!("{action} {path}"),
                    if selected {
                        Style::default().fg(TEXT)
                    } else {
                        muted
                    },
                ),
                Span::styled(
                    format!(
                        "  {}",
                        if done {
                            result
                        } else if active {
                            "进行中"
                        } else {
                            "等待"
                        }
                    ),
                    muted,
                ),
            ]));
            if preview.details && selected {
                lines.push(Line::styled(format!("      路径  {path}"), muted));
                lines.push(Line::styled(
                    format!("      结果  {result}（演示数据）"),
                    muted,
                ));
                lines.push(Line::styled(
                    "      输出  Baize preserves one development session across agent switches.",
                    muted,
                ));
            }
        }
    } else if running {
        lines.push(Line::styled(
            format!("  {} {}", TOOLS[preview.stage].0, TOOLS[preview.stage].1),
            muted,
        ));
    }
    if preview.failed {
        lines.push(Line::styled(
            "  无法读取文件：Permission denied。失败信息始终可见。",
            Style::default().fg(Color::Red),
        ));
    }
    if !preview.response_lines().is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            "antigravity",
            accent.add_modifier(Modifier::BOLD),
        ));
        lines.push(Line::raw(""));
        for text in preview.response_lines() {
            let heading = matches!(*text, "它解决什么问题" | "项目如何组成");
            lines.push(Line::styled(
                *text,
                if heading {
                    Style::default().fg(TEXT).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(TEXT)
                },
            ));
        }
        if preview.stage < 10 {
            lines.push(Line::styled("▌", accent));
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};
    use unicode_width::UnicodeWidthStr;

    fn screen(preview: &Preview, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| render(frame, preview)).unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .chunks(usize::from(width))
            .map(|row| {
                let mut skip = 0;
                let mut text = String::new();
                for cell in row {
                    if skip > 0 {
                        skip -= 1;
                        continue;
                    }
                    skip = cell.symbol().width().saturating_sub(1);
                    text.push_str(cell.symbol());
                }
                text.push('\n');
                text
            })
            .collect()
    }

    #[test]
    fn running_shows_one_active_operation_without_parameters_or_premature_answer() {
        let text = screen(&Preview::snapshot(3, false), 112, 38);
        assert!(text.contains("已完成 3/5 项"));
        assert!(text.contains("读取 doc/mvp-implementation-plan.md"));
        assert!(!text.contains("README.md"));
        assert!(!text.contains("Baize 是一个"));
        assert!(!text.contains("parameters"));
        assert!(!text.contains("not checked"));
    }

    #[test]
    fn completed_keeps_process_folded_and_displays_answer_as_normal_prose() {
        let text = screen(&Preview::snapshot(10, false), 112, 38);
        assert!(text.contains("已查看 4 个文件"));
        assert!(text.contains("Baize 是一个"));
        assert!(text.contains("它解决什么问题"));
        assert!(!text.contains("│ Baize"));
        assert!(!text.contains("读取 README.md"));
    }

    #[test]
    fn expansion_and_tool_details_preserve_final_answer() {
        let mut preview = Preview::snapshot(10, true);
        let expanded = screen(&preview, 112, 38);
        assert!(expanded.contains("读取 README.md"));
        assert!(expanded.contains("433 行"));
        assert!(expanded.contains("Baize 是一个"));
        assert!(!expanded.contains("输出  Baize"));
        preview.selected = 2;
        preview.details = true;
        let detail = screen(&preview, 112, 44);
        assert!(detail.contains("路径  doc/vision.md"));
        assert!(detail.contains("输出  Baize"));
        assert!(detail.contains("Baize 是一个"));
    }

    #[test]
    fn failures_remain_visible_when_folded_and_narrow_terminals_wrap_safely() {
        let mut preview = Preview::snapshot(3, false);
        preview.failed = true;
        assert!(screen(&preview, 112, 38).contains("Permission denied"));
        assert!(screen(&Preview::snapshot(10, true), 64, 44).contains("离线交互示例"));
        assert!(screen(&preview, 32, 8).contains("至少 48"));
    }
}
