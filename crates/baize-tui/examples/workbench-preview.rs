//! Render an offline fixture without a daemon or provider calls.
use anyhow::Result;
use baize_tui::{render, ProviderHealthView, SessionView, TuiState};
use ratatui::{backend::TestBackend, Terminal};

fn main() -> Result<()> {
    let width = std::env::args()
        .nth(1)
        .and_then(|v| v.parse().ok())
        .unwrap_or(150);
    let height = std::env::args()
        .nth(2)
        .and_then(|v| v.parse().ok())
        .unwrap_or(32);
    anyhow::ensure!(
        width > 0 && height > 0,
        "preview dimensions must be nonzero"
    );
    let mut state = TuiState::default();
    state.workspace = "/workspaces/baize".into();
    state.daemon_status = "daemon: offline preview fixture".into();
    state.detail_mode = true;
    state.session_id = Some("task_preview".into());
    state.active_provider = Some("codex".into());
    state.provider_health = vec![ProviderHealthView {
        provider_id: "codex".into(),
        status: "Healthy".into(),
        last_error: None,
    }];
    state.recent_sessions = vec![SessionView {
        id: "task_preview".into(),
        workspace_id: "ws_preview".into(),
        objective: "Preserve a conversation across agents".into(),
        active_provider_id: Some("codex".into()),
        status: "Running".into(),
    }];
    state.transcript = vec![
        "> 请保留公共 API，先讨论，再实现。".into(),
        "codex · CLI".into(),
        "  We can preserve the API and split the work at a turn boundary.".into(),
        "".into(),
        "> 换 OpenCode 接着验证。".into(),
        "opencode · CLI".into(),
        "  I will check the current workspace and the confirmed constraints.".into(),
        "  cargo test --workspace".into(),
        "  Validation results belong to the recorded run; refresh changes before review.".into(),
        "".into(),
        "> 切回 Codex，继续检查边界情况。".into(),
        "codex · CLI".into(),
        "  The Baize conversation remains the same. I will use the intervening dialogue".into(),
        "  and workspace changes to continue.".into(),
    ];
    state.input = "继续检查 Unicode 输入和会话恢复。".into();
    let mut terminal = Terminal::new(TestBackend::new(width, height))?;
    terminal.draw(|frame| render(frame, &state))?;
    let buffer = terminal.backend().buffer();
    for row in buffer.content.chunks(width as usize) {
        let mut line = String::new();
        let mut skip = 0;
        for cell in row {
            if skip > 0 {
                skip -= 1;
                continue;
            }
            let symbol = cell.symbol();
            skip = unicode_width::UnicodeWidthStr::width(symbol).saturating_sub(1);
            line.push_str(symbol);
        }
        println!("{}", line.trim_end());
    }
    Ok(())
}
