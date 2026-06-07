use anyhow::{anyhow, Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Padding, Paragraph, Wrap};
use ratatui::{Frame, Terminal};
use serde_json::{json, Value};
use std::io::{stdout, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::Duration;

const DAEMON_HOST: &str = "127.0.0.1";
const DAEMON_PORT: u16 = 7878;
const PROMPT_TIMEOUT_SECONDS: u64 = 10;
const DAEMON_START_ATTEMPTS: usize = 20;
const DAEMON_START_POLL_MS: u64 = 100;

#[derive(Debug, Clone)]
pub struct TuiState {
    pub workspace: String,
    pub transcript: Vec<String>,
    pub scroll_offset: u16,
    pub daemon_status: String,
    pub activity_status: String,
    pub providers: Vec<String>,
    pub provider_health: Vec<ProviderHealthView>,
    pub recent_sessions: Vec<SessionView>,
    pub selected_session_index: usize,
    pub pending_permissions: Vec<PermissionView>,
    pub selected_permission_index: usize,
    pub selected_provider_index: usize,
    pub active_provider: Option<String>,
    pub route_reason: Option<String>,
    pub input: String,
    pub workspace_id: Option<String>,
    pub session_id: Option<String>,
    pub pending_handoff_id: Option<String>,
    pub pending_handoff_session_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderHealthView {
    pub provider_id: String,
    pub status: String,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionView {
    pub id: String,
    pub session_id: Option<String>,
    pub command: String,
    pub reason: String,
    pub status: String,
    pub risk_level: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionView {
    pub id: String,
    pub workspace_id: String,
    pub objective: String,
    pub active_provider_id: Option<String>,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingPromptSubmission {
    prompt: String,
    provider_id: String,
}

impl Default for TuiState {
    fn default() -> Self {
        Self {
            workspace: "Baize MVP TUI".to_string(),
            transcript: vec![
                "boot sequence complete".to_string(),
                "mesh router standing by for code operations".to_string(),
                String::new(),
                "daemon link is managed automatically when possible".to_string(),
            ],
            scroll_offset: 0,
            daemon_status: "daemon: not checked".to_string(),
            activity_status: "idle".to_string(),
            providers: vec![
                "codex".to_string(),
                "gemini".to_string(),
                "copilot".to_string(),
                "opencode".to_string(),
            ],
            provider_health: Vec::new(),
            recent_sessions: Vec::new(),
            selected_session_index: 0,
            pending_permissions: Vec::new(),
            selected_permission_index: 0,
            selected_provider_index: 0,
            active_provider: None,
            route_reason: None,
            input: String::new(),
            workspace_id: None,
            session_id: None,
            pending_handoff_id: None,
            pending_handoff_session_id: None,
        }
    }
}

pub fn run() -> Result<()> {
    let daemon_status =
        ensure_daemon_running().unwrap_or_else(|error| format!("daemon: unavailable ({error:#})"));
    let provider_load = load_provider_ids();
    let health_load = load_provider_health();
    let permission_load = load_pending_permissions();

    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;

    let result = run_app(
        &mut terminal,
        daemon_status,
        provider_load,
        health_load,
        permission_load,
    );

    disable_raw_mode()?;
    terminal.backend_mut().execute(LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn run_app(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    daemon_status: String,
    provider_load: Result<Vec<String>>,
    health_load: Result<Vec<ProviderHealthView>>,
    permission_load: Result<Vec<PermissionView>>,
) -> Result<()> {
    let mut state = TuiState {
        daemon_status,
        ..TuiState::default()
    };
    match provider_load {
        Ok(providers) if !providers.is_empty() => {
            state.providers = providers;
            state.selected_provider_index = 0;
        }
        Ok(_) => state.push_message("daemon returned no enabled providers; using defaults"),
        Err(error) => state.push_message(format!("provider load failed: {error:#}")),
    }
    match health_load {
        Ok(health) => state.provider_health = health,
        Err(error) => state.push_message(format!("provider health failed: {error:#}")),
    }
    match permission_load {
        Ok(permissions) => state.pending_permissions = permissions,
        Err(error) => state.push_message(format!("permission load failed: {error:#}")),
    }
    loop {
        terminal.draw(|frame| render(frame, &state))?;

        if event::poll(Duration::from_millis(250))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Esc => break,
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
                    KeyCode::Char('h') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        if let Err(error) = request_handoff(&mut state) {
                            state.activity_status = "failed".to_string();
                            state.push_message(format!("handoff error: {error:#}"));
                        }
                    }
                    KeyCode::Char('y') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        if let Err(error) = accept_pending_handoff(&mut state) {
                            state.activity_status = "failed".to_string();
                            state.push_message(format!("handoff accept error: {error:#}"));
                        }
                    }
                    KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        if let Err(error) = refresh_provider_health(&mut state) {
                            state.activity_status = "failed".to_string();
                            state
                                .push_message(format!("provider health refresh failed: {error:#}"));
                        }
                    }
                    KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        if let Err(error) = refresh_permissions(&mut state) {
                            state.activity_status = "failed".to_string();
                            state.push_message(format!("permission refresh failed: {error:#}"));
                        }
                    }
                    KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        if let Err(error) = resolve_selected_permission(&mut state, true) {
                            state.activity_status = "failed".to_string();
                            state.push_message(format!("permission approve failed: {error:#}"));
                        }
                    }
                    KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        if let Err(error) = resolve_selected_permission(&mut state, false) {
                            state.activity_status = "failed".to_string();
                            state.push_message(format!("permission deny failed: {error:#}"));
                        }
                    }
                    KeyCode::Char('l') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        if let Err(error) = load_latest_session(&mut state) {
                            state.activity_status = "failed".to_string();
                            state.push_message(format!("load session failed: {error:#}"));
                        }
                    }
                    KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        start_new_session(&mut state);
                    }
                    KeyCode::Char('x') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        if let Err(error) = cancel_current_session(&mut state) {
                            state.activity_status = "failed".to_string();
                            state.push_message(format!("cancel error: {error:#}"));
                        }
                    }
                    KeyCode::Down => state.select_next_permission(),
                    KeyCode::Up => state.select_previous_permission(),
                    KeyCode::PageUp => state.scroll_up(10),
                    KeyCode::PageDown => state.scroll_down(10),
                    KeyCode::Home => state.scroll_to_top(),
                    KeyCode::End => state.scroll_to_bottom(),
                    KeyCode::Tab => state.cycle_provider(),
                    KeyCode::Char(ch) => state.input.push(ch),
                    KeyCode::Backspace => {
                        state.input.pop();
                    }
                    KeyCode::Enter => {
                        if let Some(submission) = begin_prompt_submission(&mut state) {
                            terminal.draw(|frame| render(frame, &state))?;
                            if let Err(error) = finish_prompt_submission(&mut state, submission) {
                                state.activity_status = "failed".to_string();
                                state.push_message(format!("error: {error:#}"));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    Ok(())
}

pub fn render(frame: &mut Frame<'_>, state: &TuiState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(4),
        ])
        .split(frame.area());

    render_header(frame, chunks[0], state);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(44), Constraint::Length(34)])
        .split(chunks[1]);

    render_transcript(frame, body[0], state);
    render_control_plane(frame, body[1], state);
    render_prompt(frame, chunks[2], state);
}

fn render_header(frame: &mut Frame<'_>, area: Rect, state: &TuiState) {
    let workspace = one_line(&state.workspace);
    let status_label = state.activity_status.as_str();
    let title = Line::from(vec![
        Span::styled(
            " BAIZE ",
            Style::default()
                .fg(Color::White)
                .bg(Color::Blue)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            " BAIZE://workspace-fabric",
            Style::default().fg(Color::White),
        ),
        Span::raw("  "),
        Span::styled(
            format!(" {status_label} "),
            Style::default()
                .fg(Color::Black)
                .bg(activity_color(&state.activity_status)),
        ),
        Span::raw("  "),
        Span::styled("[mesh:local]", muted_style()),
        Span::raw(" "),
        Span::styled("[router:sticky]", muted_style()),
    ]);
    let subtitle = Line::from(vec![
        Span::styled("workspace ", muted_style()),
        Span::raw(short_text(&workspace, 34)),
        Span::raw("   "),
        Span::styled("daemon ", muted_style()),
        Span::raw(state.daemon_status.trim_start_matches("daemon: ")),
        Span::raw("   "),
        Span::styled("target ", muted_style()),
        Span::styled(state.selected_provider().to_string(), accent_style()),
    ]);
    let rule = Line::from(Span::styled("─".repeat(area.width as usize), muted_style()));

    frame.render_widget(
        Paragraph::new(Text::from(vec![title, subtitle, rule])).alignment(Alignment::Left),
        area,
    );
}

fn render_transcript(frame: &mut Frame<'_>, area: Rect, state: &TuiState) {
    let line_count = state.transcript.len() as u16;
    let visible_height = area.height.saturating_sub(2);
    let is_scrolled = state.scroll_offset > 0;
    let max_scroll = line_count.saturating_sub(visible_height);
    let effective_scroll = if state.scroll_offset > max_scroll {
        max_scroll
    } else {
        state.scroll_offset
    };
    let title = if is_scrolled {
        let current_top = line_count.saturating_sub(effective_scroll + visible_height) + 1;
        format!(
            "Agent Stream ({}-{}/{})",
            current_top,
            current_top + visible_height.saturating_sub(1),
            line_count
        )
    } else {
        "Agent Stream".to_string()
    };
    frame.render_widget(
        Paragraph::new(transcript_text(state))
            .scroll((effective_scroll, 0))
            .wrap(Wrap { trim: false })
            .block(side_panel_block(title)),
        area,
    );
}

fn render_control_plane(frame: &mut Frame<'_>, area: Rect, state: &TuiState) {
    frame.render_widget(
        Paragraph::new(control_plane_text(state))
            .block(side_panel_block("Control Plane"))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn render_prompt(frame: &mut Frame<'_>, area: Rect, state: &TuiState) {
    let prompt = Line::from(vec![
        Span::styled("baize ", muted_style()),
        Span::styled("› ", accent_style()),
        Span::raw(state.input.as_str()),
    ]);
    let help = Line::from(vec![
        Span::styled("keys ", muted_style()),
        Span::raw(state.help_text()),
    ]);

    frame.render_widget(
        Paragraph::new(Text::from(vec![prompt, help]))
            .block(prompt_block("Prompt"))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn side_panel_block(title: impl Into<String>) -> Block<'static> {
    Block::default()
        .title(title.into())
        .borders(Borders::LEFT)
        .border_style(Style::default().fg(Color::DarkGray))
        .padding(Padding::horizontal(1))
}

fn prompt_block(title: impl Into<String>) -> Block<'static> {
    Block::default()
        .title(title.into())
        .borders(Borders::TOP)
        .border_style(Style::default().fg(Color::DarkGray))
        .padding(Padding::horizontal(1))
}

fn transcript_text(state: &TuiState) -> Text<'static> {
    let mut lines = state
        .transcript
        .iter()
        .map(|line| transcript_line(line))
        .collect::<Vec<_>>();
    if should_show_ready_panel(state) {
        lines.extend(ready_panel_lines(state));
    }
    Text::from(lines)
}

fn should_show_ready_panel(state: &TuiState) -> bool {
    state.session_id.is_none() && state.transcript.len() <= 4
}

fn ready_panel_lines(state: &TuiState) -> Vec<Line<'static>> {
    vec![
        Line::raw(""),
        Line::from(vec![
            Span::styled("╭─", muted_style()),
            Span::styled(" COMMAND MATRIX ", accent_style()),
            Span::styled("────────────────────────────╮", muted_style()),
        ]),
        ready_action_line("enter", "launch a task into the mesh"),
        ready_action_line("tab", "rotate target provider"),
        ready_action_line("^L", "re-attach latest session"),
        ready_action_line("^H", "generate handoff packet"),
        Line::from(vec![
            Span::styled("├─", muted_style()),
            Span::styled(" target ", muted_style()),
            Span::styled(
                state.selected_provider().to_string(),
                Style::default().fg(Color::White),
            ),
            Span::styled("  active ", muted_style()),
            Span::raw(
                state
                    .active_provider
                    .as_deref()
                    .unwrap_or("none")
                    .to_string(),
            ),
            Span::styled("  mode ", muted_style()),
            Span::raw("supervised"),
        ]),
        Line::from(vec![
            Span::styled("╰─", muted_style()),
            Span::styled(" await instructions ", muted_style()),
            Span::styled("────────────────────────────╯", muted_style()),
        ]),
    ]
}

fn ready_action_line(label: &'static str, text: &'static str) -> Line<'static> {
    Line::from(vec![
        Span::styled("│ ", muted_style()),
        Span::styled("▸ ", accent_style()),
        Span::styled(format!("{label:<7}"), Style::default().fg(Color::White)),
        Span::styled(":: ", muted_style()),
        Span::styled(text, muted_style()),
    ])
}

fn transcript_line(line: &str) -> Line<'static> {
    if line.trim().is_empty() {
        return Line::raw("");
    }
    if let Some(prompt) = line.strip_prefix("> ") {
        return Line::from(vec![
            Span::styled("you ", muted_style()),
            Span::styled("› ", accent_style()),
            Span::raw(prompt.to_string()),
        ]);
    }

    let lower = line.to_ascii_lowercase();
    if lower.contains("error") || lower.contains("failed") {
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
        Line::from(vec![
            Span::styled("  │ ", muted_style()),
            Span::raw(line.trim().to_string()),
        ])
    } else {
        Line::from(Span::raw(line.to_string()))
    }
}

fn control_plane_text(state: &TuiState) -> Text<'static> {
    let mut lines = Vec::new();
    lines.push(section_line("provider matrix"));
    lines.extend(provider_lines(state));
    lines.push(Line::raw(""));
    lines.push(section_line("router"));
    lines.push(key_value_line("current", &state.session_status()));
    lines.push(key_value_line("route", &state.route_status()));
    lines.push(Line::raw(""));
    lines.push(section_line("queue"));
    lines.push(key_value_line("perm", &state.permission_status()));
    lines.push(key_value_line("handoff", &state.handoff_status()));
    Text::from(lines)
}

fn provider_lines(state: &TuiState) -> Vec<Line<'static>> {
    state
        .providers
        .iter()
        .map(|provider| {
            let selected = provider == state.selected_provider();
            let active = state.active_provider.as_deref() == Some(provider.as_str());
            let health = health_status_for(&state.provider_health, provider)
                .map(short_health_status)
                .unwrap_or("?");
            let marker = if selected { "›" } else { " " };
            let active_label = if active { " active" } else { "" };
            let signal = health_signal(health);
            Line::from(vec![
                Span::styled(format!("{marker} "), accent_style()),
                Span::styled(
                    format!("{:<9}", short_text(provider, 9)),
                    if selected {
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                    },
                ),
                Span::styled(format!(" {signal} "), health_style(health)),
                Span::styled(format!("{health:<4}"), health_style(health)),
                Span::styled(active_label, muted_style()),
            ])
        })
        .collect()
}

fn section_line(label: &'static str) -> Line<'static> {
    Line::from(Span::styled(
        label.to_ascii_uppercase(),
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ))
}

fn key_value_line(key: &'static str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{key:<8}"), muted_style()),
        Span::raw(short_text(value, 32)),
    ])
}

fn activity_color(status: &str) -> Color {
    if status == "idle" {
        Color::Green
    } else if status == "failed" {
        Color::Red
    } else {
        Color::Yellow
    }
}

fn accent_style() -> Style {
    Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD)
}

fn muted_style() -> Style {
    Style::default().fg(Color::DarkGray)
}

fn health_style(health: &str) -> Style {
    match health {
        "ok" => Style::default().fg(Color::Green),
        "warn" => Style::default().fg(Color::Yellow),
        "down" => Style::default().fg(Color::Red),
        _ => muted_style(),
    }
}

fn health_signal(health: &str) -> &'static str {
    match health {
        "ok" => "▰▰▰",
        "warn" => "▰▰▱",
        "down" => "▰▱▱",
        _ => "▱▱▱",
    }
}

fn ensure_daemon_running() -> Result<String> {
    if daemon_healthy() {
        return Ok(daemon_connected_message());
    }

    start_daemon_process()?;
    for _ in 0..DAEMON_START_ATTEMPTS {
        if daemon_healthy() {
            return Ok(format!("{} (auto-started)", daemon_connected_message()));
        }
        sleep(Duration::from_millis(DAEMON_START_POLL_MS));
    }

    Err(anyhow!(
        "started daemon process, but health check did not pass at http://{DAEMON_HOST}:{DAEMON_PORT}/health"
    ))
}

fn daemon_healthy() -> bool {
    get_json("/health")
        .ok()
        .and_then(|response| {
            response
                .get("status")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .as_deref()
        == Some("ok")
}

fn start_daemon_process() -> Result<()> {
    let executable = std::env::current_exe().context("resolve current executable")?;
    Command::new(executable)
        .args(daemon_start_args())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("start baize daemon")?;
    Ok(())
}

fn daemon_start_args() -> [&'static str; 1] {
    ["daemon"]
}

fn daemon_connected_message() -> String {
    format!("daemon: connected at {DAEMON_HOST}:{DAEMON_PORT}")
}

fn load_provider_ids() -> Result<Vec<String>> {
    let response = get_json("/providers")?;
    parse_provider_ids(&response)
}

fn load_provider_health() -> Result<Vec<ProviderHealthView>> {
    let response = post_json("/providers/check", json!({}))?;
    parse_provider_health(&response)
}

fn load_pending_permissions() -> Result<Vec<PermissionView>> {
    let response = get_json("/permissions?status=pending")?;
    parse_permissions(&response)
}

fn prompt_request_body(provider_id: &str, prompt: &str) -> Value {
    json!({
        "prompt": prompt,
        "provider_id": provider_id,
        "timeout_seconds": PROMPT_TIMEOUT_SECONDS,
    })
}

fn refresh_provider_health(state: &mut TuiState) -> Result<()> {
    state.activity_status = "refreshing provider health".to_string();
    let health = load_provider_health()?;
    let summary = summarize_provider_health(&health);
    state.provider_health = health;
    state.activity_status = "idle".to_string();
    state.push_message(format!("provider health refreshed: {summary}"));
    Ok(())
}

fn refresh_permissions(state: &mut TuiState) -> Result<()> {
    state.activity_status = "refreshing permissions".to_string();
    let permissions = load_pending_permissions()?;
    state.set_pending_permissions(permissions);
    state.activity_status = "idle".to_string();
    state.push_message(format!(
        "permissions refreshed: {} pending",
        state.pending_permissions.len()
    ));
    Ok(())
}

fn parse_provider_ids(response: &Value) -> Result<Vec<String>> {
    let providers = response
        .get("providers")
        .and_then(Value::as_array)
        .ok_or_else(|| response_error("providers", response))?;

    Ok(providers
        .iter()
        .filter(|provider| {
            provider
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(true)
        })
        .filter_map(|provider| provider.get("id").and_then(Value::as_str))
        .map(ToOwned::to_owned)
        .collect())
}

fn parse_permissions(response: &Value) -> Result<Vec<PermissionView>> {
    let permissions = response
        .get("permissions")
        .and_then(Value::as_array)
        .ok_or_else(|| response_error("permissions", response))?;

    Ok(permissions
        .iter()
        .filter_map(|permission| {
            let id = permission.get("id").and_then(permission_id)?;
            let command = permission.get("command").and_then(Value::as_str)?;
            let reason = permission
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let status = permission
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("Pending");
            let risk_level = permission
                .get("risk")
                .and_then(|risk| risk.get("level"))
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
            Some(PermissionView {
                id: id.to_string(),
                session_id: permission
                    .get("session_id")
                    .and_then(task_session_id)
                    .map(ToOwned::to_owned),
                command: command.to_string(),
                reason: reason.to_string(),
                status: status.to_string(),
                risk_level,
            })
        })
        .collect())
}

fn parse_provider_health(response: &Value) -> Result<Vec<ProviderHealthView>> {
    let health = response
        .get("health")
        .and_then(Value::as_array)
        .ok_or_else(|| response_error("health", response))?;

    Ok(health
        .iter()
        .filter_map(|entry| {
            let provider_id = entry.get("provider_id").and_then(provider_id)?;
            let status = entry.get("status").and_then(Value::as_str)?;
            Some(ProviderHealthView {
                provider_id: provider_id.to_string(),
                status: status.to_string(),
                last_error: entry
                    .get("last_error")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
            })
        })
        .collect())
}

fn health_status_for<'a>(health: &'a [ProviderHealthView], provider_id: &str) -> Option<&'a str> {
    health
        .iter()
        .find(|entry| entry.provider_id == provider_id)
        .map(|entry| entry.status.as_str())
}

fn summarize_provider_health(health: &[ProviderHealthView]) -> String {
    if health.is_empty() {
        return "none".to_string();
    }
    health
        .iter()
        .map(|entry| {
            format!(
                "{}:{}",
                entry.provider_id,
                short_health_status(&entry.status)
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn begin_prompt_submission(state: &mut TuiState) -> Option<PendingPromptSubmission> {
    let prompt = state.input.trim().to_string();
    if prompt.is_empty() {
        return None;
    }

    let provider_id = state.selected_provider().to_string();
    state.activity_status = format!("running {provider_id}");
    state.push_message(format!("> {prompt}"));
    state.push_message(format!("mesh: dispatching to {provider_id}"));
    state.input.clear();

    Some(PendingPromptSubmission {
        prompt,
        provider_id,
    })
}

fn finish_prompt_submission(
    state: &mut TuiState,
    submission: PendingPromptSubmission,
) -> Result<()> {
    let session_id = match state.session_id.clone() {
        Some(session_id) => session_id,
        None => {
            let workspace_id = ensure_workspace(state)?;
            ensure_session(state, &workspace_id, &submission.prompt)?
        }
    };

    let response = post_json(
        &format!("/sessions/{session_id}/prompt"),
        prompt_request_body(&submission.provider_id, &submission.prompt),
    )?;
    append_prompt_response(state, &response);

    let events = get_json(&format!("/sessions/{session_id}/events"))?;
    append_recent_events_with_sections(state, &events);
    refresh_route_history(state)?;
    refresh_session_diff(state)?;
    state.activity_status = "idle".to_string();
    Ok(())
}

fn load_latest_session(state: &mut TuiState) -> Result<()> {
    state.activity_status = "loading latest session".to_string();
    let response = get_json("/sessions")?;
    let sessions = parse_session_views(&response)?;
    state.set_recent_sessions(sessions);
    append_session_list(state);
    let Some(session) = latest_session(&response) else {
        state.push_message("no existing sessions");
        state.activity_status = "idle".to_string();
        return Ok(());
    };

    apply_loaded_session(state, session)?;
    refresh_route_history(state)?;
    refresh_session_diff(state)?;
    state.activity_status = "idle".to_string();
    Ok(())
}

fn start_new_session(state: &mut TuiState) {
    state.session_id = None;
    state.active_provider = None;
    state.route_reason = None;
    state.pending_handoff_id = None;
    state.pending_handoff_session_id = None;
    state.input.clear();
    state.activity_status = "idle".to_string();
    state.push_message("new session: next prompt will create a fresh task");
}

fn cancel_current_session(state: &mut TuiState) -> Result<()> {
    let Some(session_id) = state.session_id.clone() else {
        state.push_message("start a session before canceling");
        return Ok(());
    };

    state.activity_status = "canceling session".to_string();
    let response = post_json(&format!("/sessions/{session_id}/cancel"), json!({}))?;
    append_cancel_response(state, &response);
    state.activity_status = "idle".to_string();
    Ok(())
}

fn append_cancel_response(state: &mut TuiState, response: &Value) {
    let session = response.get("session").unwrap_or(&Value::Null);
    let id = session
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let status = session
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("unknown");

    state.session_id = session
        .get("id")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    state.active_provider = session
        .get("active_provider_id")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    state.route_reason = None;
    state.pending_handoff_id = None;
    state.pending_handoff_session_id = None;
    state.push_message(format!("session {id}: {status}"));
}

fn latest_session(response: &Value) -> Option<&Value> {
    response.get("sessions")?.as_array()?.last()
}

fn parse_session_views(response: &Value) -> Result<Vec<SessionView>> {
    let sessions = response
        .get("sessions")
        .and_then(Value::as_array)
        .ok_or_else(|| response_error("sessions", response))?;

    Ok(sessions
        .iter()
        .filter_map(|session| {
            let id = session.get("id").and_then(task_session_id)?;
            let workspace_id = session.get("workspace_id").and_then(workspace_id)?;
            let objective = session
                .get("objective")
                .and_then(Value::as_str)
                .unwrap_or("session");
            let status = session
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("Unknown");
            Some(SessionView {
                id: id.to_string(),
                workspace_id: workspace_id.to_string(),
                objective: objective.to_string(),
                active_provider_id: session
                    .get("active_provider_id")
                    .and_then(provider_id)
                    .map(ToOwned::to_owned),
                status: status.to_string(),
            })
        })
        .collect())
}

fn apply_loaded_session(state: &mut TuiState, session: &Value) -> Result<()> {
    let id = session
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| response_error("session.id", session))?;
    let workspace_id = session
        .get("workspace_id")
        .and_then(Value::as_str)
        .ok_or_else(|| response_error("session.workspace_id", session))?;
    let objective = session
        .get("objective")
        .and_then(Value::as_str)
        .unwrap_or("session");
    let provider = session
        .get("active_provider_id")
        .and_then(Value::as_str)
        .unwrap_or("unknown");

    state.session_id = Some(id.to_string());
    state.workspace_id = Some(workspace_id.to_string());
    state.active_provider = Some(provider.to_string());
    state.route_reason = None;
    state.pending_handoff_id = None;
    state.pending_handoff_session_id = None;
    state.workspace = format!("Workspace: loaded\n{workspace_id}");
    state.select_recent_session(id);
    state.push_message(format!("loaded session: {id} ({provider})"));
    state.push_message(format!("objective: {}", one_line(objective)));
    Ok(())
}

fn append_session_list(state: &mut TuiState) {
    if state.recent_sessions.is_empty() {
        return;
    }

    state.push_message("sessions:".to_string());
    let start = state.recent_sessions.len().saturating_sub(5);
    let visible_sessions = state
        .recent_sessions
        .iter()
        .cloned()
        .enumerate()
        .skip(start)
        .collect::<Vec<_>>();
    for (index, session) in visible_sessions {
        let selected = if index == state.selected_session_index {
            ">"
        } else {
            " "
        };
        let provider = session.active_provider_id.as_deref().unwrap_or("none");
        state.push_message(format!(
            "  {selected} {} {} {} - {}",
            session.id,
            session.status,
            provider,
            short_text(&session.objective, 40)
        ));
    }
}

fn request_handoff(state: &mut TuiState) -> Result<()> {
    let Some(session_id) = state.session_id.clone() else {
        state.push_message("start a session before requesting handoff");
        return Ok(());
    };
    let target_provider = state.selected_provider().to_string();
    if state.active_provider.as_deref() == Some(target_provider.as_str()) {
        state.push_message("choose a different provider before handoff");
        return Ok(());
    }

    state.activity_status = format!("handoff to {target_provider}");
    let handoff = post_json(
        &format!("/sessions/{session_id}/handoff"),
        json!({
            "to_provider_id": target_provider,
            "user_constraints": ["requested from TUI"]
        }),
    )?;
    let handoff_id = handoff
        .get("handoff")
        .and_then(|handoff| handoff.get("id"))
        .and_then(Value::as_str)
        .ok_or_else(|| response_error("handoff.id", &handoff))?
        .to_string();
    append_handoff_preview(state, &handoff);
    state.pending_handoff_id = Some(handoff_id);
    state.pending_handoff_session_id = Some(session_id);
    state.activity_status = "idle".to_string();
    Ok(())
}

fn accept_pending_handoff(state: &mut TuiState) -> Result<()> {
    let (Some(session_id), Some(handoff_id)) = (
        state.pending_handoff_session_id.clone(),
        state.pending_handoff_id.clone(),
    ) else {
        state.push_message("create a handoff preview before accepting");
        return Ok(());
    };

    state.activity_status = "accepting handoff".to_string();
    let accepted = post_json(
        &format!("/sessions/{session_id}/handoff/{handoff_id}/accept"),
        json!({}),
    )?;
    append_handoff_response(state, &accepted);
    state.pending_handoff_id = None;
    state.pending_handoff_session_id = None;
    refresh_route_history(state)?;
    refresh_session_diff(state)?;
    state.activity_status = "idle".to_string();
    Ok(())
}

fn append_handoff_preview(state: &mut TuiState, response: &Value) {
    let handoff = response.get("handoff").unwrap_or(&Value::Null);
    let id = handoff.get("id").and_then(handoff_id).unwrap_or("unknown");
    let from_provider = handoff
        .get("from_provider_id")
        .and_then(provider_id)
        .unwrap_or("unknown");
    let to_provider = handoff
        .get("to_provider_id")
        .and_then(provider_id)
        .unwrap_or("unknown");
    state.push_message(format!(
        "handoff preview: {id} {from_provider} -> {to_provider}"
    ));
    append_handoff_detail(state, handoff);
    state.push_message("press Ctrl-Y to accept handoff");
}

fn append_handoff_detail(state: &mut TuiState, handoff: &Value) {
    if let Some(summary) = handoff.get("summary_markdown").and_then(Value::as_str) {
        let lines = summary
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .filter(|line| !line.starts_with('#'))
            .map(one_line)
            .collect::<Vec<_>>();
        if !lines.is_empty() {
            state.push_message("  summary detail:".to_string());
            for line in lines.iter().take(10) {
                state.push_message(format!("    {line}"));
            }
            if lines.len() > 10 {
                state.push_message(format!("    (+{} more summary lines)", lines.len() - 10));
            }
        }
    }

    let facts = handoff.get("mechanical_facts").unwrap_or(&Value::Null);
    append_handoff_fact_list(state, facts, "changed_files", "changed files");
    append_handoff_fact_list(state, facts, "commands_run", "commands");
    append_handoff_fact_value(state, facts, "test_result", "test result");
    append_handoff_fact_list(state, facts, "provider_errors", "provider errors");
    append_handoff_fact_list(state, facts, "checkpoint_refs", "checkpoints");
    append_handoff_fact_list(state, facts, "user_constraints", "constraints");
}

fn append_handoff_fact_list(state: &mut TuiState, facts: &Value, key: &str, label: &str) {
    let Some(values) = facts.get(key).and_then(Value::as_array) else {
        return;
    };
    if values.is_empty() {
        return;
    }

    let rendered = values
        .iter()
        .filter_map(Value::as_str)
        .map(one_line)
        .filter(|value| !value.is_empty())
        .take(5)
        .collect::<Vec<_>>();
    if rendered.is_empty() {
        return;
    }

    let suffix = values
        .len()
        .checked_sub(rendered.len())
        .filter(|remaining| *remaining > 0)
        .map(|remaining| format!(" (+{remaining} more)"))
        .unwrap_or_default();
    state.push_message(format!("  {label}: {}{}", rendered.join(", "), suffix));
}

fn append_handoff_fact_value(state: &mut TuiState, facts: &Value, key: &str, label: &str) {
    let Some(value) = facts.get(key).and_then(Value::as_str) else {
        return;
    };
    if value.trim().is_empty() {
        return;
    }

    state.push_message(format!("  {label}: {}", one_line(value)));
}

fn append_handoff_response(state: &mut TuiState, response: &Value) {
    let handoff = response.get("handoff").unwrap_or(&Value::Null);
    let from_provider = handoff
        .get("from_provider_id")
        .and_then(provider_id)
        .unwrap_or("unknown");
    let to_provider = handoff
        .get("to_provider_id")
        .and_then(provider_id)
        .unwrap_or("unknown");
    let status = handoff
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    state.push_message(format!(
        "handoff {status}: {from_provider} -> {to_provider}"
    ));

    if let Some(session) = response.get("session") {
        state.active_provider = session
            .get("active_provider_id")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
    }
    if let Some(reason) = response
        .get("route_decision")
        .and_then(|decision| decision.get("reason"))
        .and_then(Value::as_str)
    {
        state.route_reason = Some(reason.to_string());
        state.push_message(format!("route: {reason}"));
    }
}

fn resolve_selected_permission(state: &mut TuiState, approve: bool) -> Result<()> {
    let Some(permission) = state.selected_permission().cloned() else {
        state.push_message("no pending permission selected");
        return Ok(());
    };
    let action = if approve { "approve" } else { "deny" };
    state.activity_status = format!("{action} permission");
    let response = post_json(
        &format!("/permissions/{}/{action}", permission.id),
        json!({}),
    )?;
    append_permission_resolution(state, &response);
    refresh_permissions(state)?;
    Ok(())
}

fn append_permission_resolution(state: &mut TuiState, response: &Value) {
    let permission = response.get("permission").unwrap_or(&Value::Null);
    let id = permission
        .get("id")
        .and_then(permission_id)
        .unwrap_or("unknown");
    let status = permission
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let command = permission
        .get("command")
        .and_then(Value::as_str)
        .map(one_line)
        .unwrap_or_default();

    if command.is_empty() {
        state.push_message(format!("permission {status}: {id}"));
    } else {
        state.push_message(format!("permission {status}: {id} - {command}"));
    }
}

fn ensure_workspace(state: &mut TuiState) -> Result<String> {
    if let Some(id) = &state.workspace_id {
        return Ok(id.clone());
    }

    let cwd = std::env::current_dir().context("read current directory")?;
    let name = workspace_name(&cwd);
    let response = post_json(
        "/workspaces",
        json!({
            "path": cwd,
            "name": name,
        }),
    )?;
    let workspace = response
        .get("workspace")
        .ok_or_else(|| response_error("workspace", &response))?;
    let id = workspace
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| response_error("workspace.id", &response))?
        .to_string();
    let display_name = workspace
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("workspace");

    state.workspace = format!("Workspace: {display_name}\n{id}");
    state.workspace_id = Some(id.clone());
    Ok(id)
}

fn ensure_session(state: &mut TuiState, workspace_id: &str, prompt: &str) -> Result<String> {
    if let Some(id) = &state.session_id {
        return Ok(id.clone());
    }

    let response = post_json(
        "/sessions",
        json!({
            "workspace_id": workspace_id,
            "objective": prompt,
            "provider_id": state.selected_provider(),
        }),
    )?;
    let session = response
        .get("session")
        .ok_or_else(|| response_error("session", &response))?;
    let id = session
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| response_error("session.id", &response))?
        .to_string();
    let provider = session
        .get("active_provider_id")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let route_reason = response
        .get("route_decision")
        .and_then(|decision| decision.get("reason"))
        .and_then(Value::as_str);

    state.push_message(format!("session: {id} ({provider})"));
    if let Some(reason) = route_reason {
        state.push_message(format!("route: {reason}"));
        state.route_reason = Some(reason.to_string());
    }
    state.active_provider = Some(provider.to_string());
    state.session_id = Some(id.clone());
    refresh_route_history(state)?;
    Ok(id)
}

fn refresh_route_history(state: &mut TuiState) -> Result<()> {
    let Some(session_id) = state.session_id.clone() else {
        return Ok(());
    };
    let response = get_json(&format!("/sessions/{session_id}/routes"))?;
    append_route_history(state, &response);
    Ok(())
}

fn append_route_history(state: &mut TuiState, response: &Value) {
    let Some(routes) = response.get("routes").and_then(Value::as_array) else {
        return;
    };
    if routes.is_empty() {
        return;
    }

    state.push_message("routes:");
    for route in routes
        .iter()
        .rev()
        .take(3)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
        let selected = route
            .get("selected_provider_id")
            .and_then(provider_id)
            .unwrap_or("unknown");
        let previous = route
            .get("previous_provider_id")
            .and_then(provider_id)
            .unwrap_or("none");
        let reason = route
            .get("reason")
            .and_then(Value::as_str)
            .map(one_line)
            .unwrap_or_default();
        state.push_message(format!("  {previous} -> {selected}: {reason}"));
    }
}

fn refresh_session_diff(state: &mut TuiState) -> Result<()> {
    let Some(session_id) = state.session_id.clone() else {
        return Ok(());
    };
    let response = get_json(&format!("/sessions/{session_id}/diff"))?;
    append_session_diff(state, &response);
    Ok(())
}

fn append_session_diff(state: &mut TuiState, response: &Value) {
    let Some(diff) = response.get("diff") else {
        return;
    };
    let dirty = diff.get("dirty").and_then(Value::as_bool).unwrap_or(false);
    let changed_files = diff
        .get("changed_files")
        .and_then(Value::as_array)
        .map(|files| {
            files
                .iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if !dirty {
        state.push_message("workspace clean");
        return;
    }

    let shown = changed_files.iter().take(5).cloned().collect::<Vec<_>>();
    let suffix = changed_files
        .len()
        .checked_sub(shown.len())
        .filter(|remaining| *remaining > 0)
        .map(|remaining| format!(" (+{remaining} more)"))
        .unwrap_or_default();
    state.push_message(format!(
        "workspace dirty: {}{}",
        if shown.is_empty() {
            "changed files unknown".to_string()
        } else {
            shown.join(", ")
        },
        suffix
    ));
}

fn append_prompt_response(state: &mut TuiState, response: &Value) {
    let status = response
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let provider = response
        .get("provider_id")
        .and_then(provider_id)
        .unwrap_or("unknown");
    state.push_message(format!("prompt result: {status} via {provider}"));

    if let Some(error) = response.get("error").and_then(Value::as_str) {
        state.push_message(format!("  error: {error}"));
    }
    if let Some(stderr) = response.get("stderr").and_then(Value::as_str) {
        if !stderr.trim().is_empty() {
            state.push_message(format!("  stderr: {}", one_line(stderr)));
        }
    }
    if let Some(hint) = provider_error_hint(response) {
        state.push_message(hint);
    }
}

fn append_recent_events(state: &mut TuiState, response: &Value) {
    let Some(events) = response.get("events").and_then(Value::as_array) else {
        return;
    };

    let recent_events = events
        .iter()
        .rev()
        .take(5)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>();
    let mut tool_lines = Vec::new();
    let mut output_lines = Vec::new();
    for event in recent_events {
        let event_type = event
            .get("event_type")
            .and_then(Value::as_str)
            .unwrap_or("event");
        let payload = event.get("payload").unwrap_or(&Value::Null);
        let detail = payload
            .get("text")
            .and_then(Value::as_str)
            .or_else(|| payload.get("error").and_then(Value::as_str))
            .or_else(|| payload.get("stderr").and_then(Value::as_str))
            .map(one_line);

        let line = match detail {
            Some(detail) if !detail.is_empty() => format!("{event_type}: {detail}"),
            _ => event_type.to_string(),
        };
        if event_type == "session.agent.tool_call" {
            tool_lines.push(line);
        } else {
            output_lines.push(line);
        }
    }

    if !tool_lines.is_empty() {
        state.push_message("command/tool events:".to_string());
        for line in tool_lines {
            state.push_message(format!("  {line}"));
        }
    }
    if !output_lines.is_empty() {
        state.push_message("agent output:".to_string());
        for line in output_lines {
            state.push_message(format!("  {line}"));
        }
    }
}

fn append_test_result_section(state: &mut TuiState, response: &Value) {
    let Some(events) = response.get("events").and_then(Value::as_array) else {
        return;
    };
    let test_lines = events
        .iter()
        .filter_map(|event| event.get("payload"))
        .filter_map(|payload| {
            payload
                .get("text")
                .and_then(Value::as_str)
                .or_else(|| payload.get("stderr").and_then(Value::as_str))
        })
        .map(one_line)
        .filter(|line| {
            let lower = line.to_ascii_lowercase();
            contains_any(
                &lower,
                &[
                    "test result",
                    "test failed",
                    "test passed",
                    "tests passed",
                    "test result:",
                    "cargo test",
                    "pytest",
                ],
            )
        })
        .take(3)
        .collect::<Vec<_>>();
    if test_lines.is_empty() {
        return;
    }

    state.push_message("test results:".to_string());
    for line in test_lines {
        state.push_message(format!("  {line}"));
    }
}

fn append_recent_events_with_sections(state: &mut TuiState, response: &Value) {
    append_recent_events(state, response);
    append_test_result_section(state, response);
}

impl TuiState {
    fn selected_provider(&self) -> &str {
        self.providers
            .get(self.selected_provider_index)
            .map(String::as_str)
            .unwrap_or("codex")
    }

    fn cycle_provider(&mut self) {
        if self.providers.is_empty() {
            return;
        }
        self.selected_provider_index = (self.selected_provider_index + 1) % self.providers.len();
    }

    fn selected_permission(&self) -> Option<&PermissionView> {
        self.pending_permissions.get(self.selected_permission_index)
    }

    fn select_next_permission(&mut self) {
        if self.pending_permissions.is_empty() {
            return;
        }
        self.selected_permission_index =
            (self.selected_permission_index + 1) % self.pending_permissions.len();
    }

    fn select_previous_permission(&mut self) {
        if self.pending_permissions.is_empty() {
            return;
        }
        self.selected_permission_index = self
            .selected_permission_index
            .checked_sub(1)
            .unwrap_or(self.pending_permissions.len() - 1);
    }

    fn set_pending_permissions(&mut self, permissions: Vec<PermissionView>) {
        self.pending_permissions = permissions;
        if self.pending_permissions.is_empty() {
            self.selected_permission_index = 0;
        } else if self.selected_permission_index >= self.pending_permissions.len() {
            self.selected_permission_index = self.pending_permissions.len() - 1;
        }
    }

    fn set_recent_sessions(&mut self, sessions: Vec<SessionView>) {
        self.recent_sessions = sessions;
        if self.recent_sessions.is_empty() {
            self.selected_session_index = 0;
        } else {
            self.selected_session_index = self.recent_sessions.len() - 1;
        }
    }

    fn select_recent_session(&mut self, session_id: &str) {
        if let Some(index) = self
            .recent_sessions
            .iter()
            .position(|session| session.id == session_id)
        {
            self.selected_session_index = index;
        }
    }

    #[cfg(test)]
    fn provider_status(&self) -> String {
        let selected = self.selected_provider();
        self.providers
            .iter()
            .map(|provider| {
                let active = self.active_provider.as_deref() == Some(provider.as_str());
                let health = health_status_for(&self.provider_health, provider)
                    .map(short_health_status)
                    .unwrap_or("?");
                let selected_marker = if provider == selected { ">" } else { " " };
                let active_marker = if active { "*" } else { "" };
                format!("{selected_marker}{provider}:{health}{active_marker}")
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn route_status(&self) -> String {
        match (&self.active_provider, &self.route_reason) {
            (Some(provider), Some(reason)) => format!(
                "{provider} active; target {} with Ctrl-H - {reason}",
                self.selected_provider()
            ),
            (Some(provider), None) => format!(
                "{provider} active; target {} with Ctrl-H",
                self.selected_provider()
            ),
            (None, _) => format!(
                "{} selected; Tab switches before first prompt",
                self.selected_provider()
            ),
        }
    }

    fn session_status(&self) -> String {
        if let Some(session) = self.recent_sessions.get(self.selected_session_index) {
            let current_marker = if self.session_id.as_deref() == Some(session.id.as_str()) {
                "*"
            } else {
                ""
            };
            let provider = session.active_provider_id.as_deref().unwrap_or("none");
            return format!(
                "> [{}/{}] {}{} {} {}",
                self.selected_session_index + 1,
                self.recent_sessions.len(),
                session.id,
                current_marker,
                session.status,
                provider
            );
        }

        self.session_id
            .as_ref()
            .map(|id| format!("current {id}"))
            .unwrap_or_else(|| "none loaded".to_string())
    }

    fn permission_status(&self) -> String {
        let Some(permission) = self.selected_permission() else {
            return "none pending".to_string();
        };
        let session = permission
            .session_id
            .as_deref()
            .map(|session_id| format!(" {session_id}"))
            .unwrap_or_default();
        let risk = permission
            .risk_level
            .as_deref()
            .map(|risk| format!(" risk {risk}"))
            .unwrap_or_default();
        format!(
            "> [{}/{}] {}{}{}",
            self.selected_permission_index + 1,
            self.pending_permissions.len(),
            short_text(&permission.command, 36),
            risk,
            session
        )
    }

    fn handoff_status(&self) -> String {
        match (&self.pending_handoff_id, &self.pending_handoff_session_id) {
            (Some(handoff_id), Some(session_id)) => {
                format!("> {handoff_id} on {session_id}; Ctrl-Y accepts")
            }
            (Some(handoff_id), None) => format!("> {handoff_id}; Ctrl-Y accepts"),
            _ => "none pending".to_string(),
        }
    }

    fn help_text(&self) -> &'static str {
        "Ent|Tab|Up/Dn|^N new|^L load|^H hand|^Y yes|^A ok|^D no|^X stop"
    }

    fn push_message(&mut self, message: impl Into<String>) {
        self.transcript.push(message.into());
        self.scroll_offset = 0;
    }

    fn scroll_up(&mut self, amount: u16) {
        self.scroll_offset = self.scroll_offset.saturating_add(amount);
        let max_scroll = self.transcript.len().saturating_sub(1) as u16;
        if self.scroll_offset > max_scroll {
            self.scroll_offset = max_scroll;
        }
    }

    fn scroll_down(&mut self, amount: u16) {
        self.scroll_offset = self.scroll_offset.saturating_sub(amount);
    }

    fn scroll_to_bottom(&mut self) {
        self.scroll_offset = 0;
    }

    fn scroll_to_top(&mut self) {
        self.scroll_offset = self.transcript.len().saturating_sub(1) as u16;
    }

    #[cfg(test)]
    fn transcript_text(&self) -> String {
        self.transcript.join("\n")
    }
}

fn short_health_status(status: &str) -> &str {
    match status {
        "Healthy" => "ok",
        "Degraded" => "warn",
        "Unavailable" => "down",
        "Unknown" => "?",
        _ => "?",
    }
}

fn get_json(path: &str) -> Result<Value> {
    request_json("GET", path, None)
}

fn post_json(path: &str, body: Value) -> Result<Value> {
    request_json("POST", path, Some(body))
}

fn request_json(method: &str, path: &str, body: Option<Value>) -> Result<Value> {
    let body = body.map(|body| body.to_string()).unwrap_or_default();
    let mut stream = TcpStream::connect((DAEMON_HOST, DAEMON_PORT))
        .with_context(|| format!("connect to baize daemon at {DAEMON_HOST}:{DAEMON_PORT}"))?;
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: {DAEMON_HOST}:{DAEMON_PORT}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(request.as_bytes())
        .context("send daemon request")?;

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .context("read daemon response")?;
    parse_http_json_response(&response)
}

fn parse_http_json_response(response: &str) -> Result<Value> {
    let (head, body) = response
        .split_once("\r\n\r\n")
        .ok_or_else(|| anyhow!("invalid daemon response"))?;
    let status_line = head.lines().next().unwrap_or_default();
    let value: Value = serde_json::from_str(body.trim()).context("parse daemon JSON response")?;
    if !status_line.contains(" 200 ") {
        if value.get("status").and_then(Value::as_str) == Some("failed") {
            return Ok(value);
        }
        if let Some(error) = value.get("error").and_then(Value::as_str) {
            return Err(anyhow!("daemon returned {status_line}: {error}"));
        }
        return Err(anyhow!("daemon returned {status_line}"));
    }
    if let Some(error) = value.get("error").and_then(Value::as_str) {
        return Err(anyhow!(error.to_string()));
    }
    Ok(value)
}

fn response_error(field: &str, response: &Value) -> anyhow::Error {
    anyhow!("daemon response missing {field}: {response}")
}

fn provider_id(value: &Value) -> Option<&str> {
    value
        .as_str()
        .or_else(|| value.as_object()?.get("0")?.as_str())
}

fn permission_id(value: &Value) -> Option<&str> {
    value
        .as_str()
        .or_else(|| value.as_object()?.get("0")?.as_str())
}

fn handoff_id(value: &Value) -> Option<&str> {
    value
        .as_str()
        .or_else(|| value.as_object()?.get("0")?.as_str())
}

fn task_session_id(value: &Value) -> Option<&str> {
    value
        .as_str()
        .or_else(|| value.as_object()?.get("0")?.as_str())
}

fn workspace_id(value: &Value) -> Option<&str> {
    value
        .as_str()
        .or_else(|| value.as_object()?.get("0")?.as_str())
}

fn workspace_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "workspace".to_string())
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn provider_error_hint(response: &Value) -> Option<String> {
    if let Some(kind) = response
        .get("limit_inference")
        .and_then(|limit| limit.get("kind"))
        .and_then(Value::as_str)
    {
        return match kind {
            "RateLimit" => Some(
                "provider hint: rate limit detected; switch provider or wait for the quota window"
                    .to_string(),
            ),
            "QuotaExceeded" => Some(
                "provider hint: quota exhausted; switch provider or wait for quota reset"
                    .to_string(),
            ),
            _ => None,
        };
    }

    let detail = [
        response.get("error").and_then(Value::as_str),
        response.get("stderr").and_then(Value::as_str),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" ");
    if detail.trim().is_empty() {
        return None;
    }

    let detail = detail.to_ascii_lowercase();
    if contains_any(
        &detail,
        &[
            "timed out",
            "timeout",
            "deadline",
            "interactive prompt",
            "would block",
        ],
    ) {
        return Some(
            "provider hint: command timed out; check CLI login/interactivity or increase timeout"
                .to_string(),
        );
    }
    if contains_any(
        &detail,
        &[
            "auth",
            "login",
            "oauth",
            "api key",
            "credential",
            "permission denied",
            "unauthorized",
            "not authenticated",
        ],
    ) {
        return Some(
            "provider hint: authentication/setup required; run the provider CLI login flow"
                .to_string(),
        );
    }

    None
}

fn contains_any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| text.contains(needle))
}

fn short_text(text: &str, max_chars: usize) -> String {
    let text = one_line(text);
    if text.chars().count() <= max_chars {
        return text;
    }

    let keep = max_chars.saturating_sub(3);
    format!("{}...", text.chars().take(keep).collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    #[test]
    fn renders_mvp_dashboard_text() {
        let backend = TestBackend::new(80, 18);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let state = TuiState::default();

        terminal.draw(|frame| render(frame, &state)).expect("draw");
        let buffer = terminal.backend().buffer();
        let rendered = format!("{buffer:?}");

        assert!(rendered.contains("BAIZE"));
        assert!(rendered.contains("BAIZE://workspace-fabric"));
        assert!(rendered.contains("Baize MVP TUI"));
        assert!(rendered.contains("daemon"));
        assert!(rendered.contains("not checked"));
        assert!(rendered.contains("Agent Stream"));
        assert!(rendered.contains("COMMAND MATRIX"));
        assert!(rendered.contains("enter"));
        assert!(rendered.contains("target"));
        assert!(rendered.contains("Control Plane"));
        assert!(rendered.contains("PROVIDER MATRIX"));
        assert!(rendered.contains("ROUTER"));
        assert!(rendered.contains("codex"));
        assert!(rendered.contains("opencode"));
        assert!(rendered.contains("Prompt"));
        assert!(rendered.contains("keys"));
        assert!(rendered.contains("Ent|Tab|Up/Dn|^N new|^L load|^H hand"));
    }

    #[test]
    fn parses_http_json_response() {
        let response =
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\r\n{\"status\":\"ok\"}";
        let value = parse_http_json_response(response).expect("response");

        assert_eq!(value["status"], "ok");
    }

    #[test]
    fn parse_http_json_response_rejects_error_payload() {
        let response =
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\r\n{\"error\":\"no daemon\"}";
        let error = parse_http_json_response(response).expect_err("error");

        assert!(error.to_string().contains("no daemon"));
    }

    #[test]
    fn parse_http_json_response_allows_structured_prompt_failure() {
        let response = "HTTP/1.1 500 Internal Server Error\r\ncontent-type: application/json\r\n\r\n{\"status\":\"failed\",\"error\":\"provider failed\"}";
        let value = parse_http_json_response(response).expect("structured failure");

        assert_eq!(value["status"], "failed");
        assert_eq!(value["error"], "provider failed");
    }

    #[test]
    fn parse_http_json_response_includes_error_for_non_prompt_failure() {
        let response = "HTTP/1.1 404 Not Found\r\ncontent-type: application/json\r\n\r\n{\"error\":\"session not found\"}";
        let error = parse_http_json_response(response).expect_err("error");

        assert!(error.to_string().contains("HTTP/1.1 404 Not Found"));
        assert!(error.to_string().contains("session not found"));
    }

    #[test]
    fn renders_prompt_input() {
        let backend = TestBackend::new(80, 19);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let state = TuiState {
            input: "hello baize".to_string(),
            ..TuiState::default()
        };

        terminal.draw(|frame| render(frame, &state)).expect("draw");
        let buffer = terminal.backend().buffer();
        let rendered = format!("{buffer:?}");

        assert!(rendered.contains("baize"));
        assert!(rendered.contains("hello baize"));
    }

    #[test]
    fn prompt_request_body_includes_target_provider() {
        let body = prompt_request_body("gemini", "continue task");

        assert_eq!(body["prompt"], "continue task");
        assert_eq!(body["provider_id"], "gemini");
        assert_eq!(body["timeout_seconds"], PROMPT_TIMEOUT_SECONDS);
    }

    #[test]
    fn begin_prompt_submission_immediately_shows_running_state() {
        let mut state = TuiState {
            selected_provider_index: 1,
            input: "你能做什么".to_string(),
            ..TuiState::default()
        };

        let submission = begin_prompt_submission(&mut state).expect("submission");

        assert_eq!(submission.provider_id, "gemini");
        assert_eq!(submission.prompt, "你能做什么");
        assert!(state.input.is_empty());
        assert_eq!(state.activity_status, "running gemini");
        assert!(state.transcript_text().contains("> 你能做什么"));
        assert!(state
            .transcript_text()
            .contains("mesh: dispatching to gemini"));
    }

    #[test]
    fn begin_prompt_submission_ignores_blank_input() {
        let mut state = TuiState {
            input: "   ".to_string(),
            ..TuiState::default()
        };

        let submission = begin_prompt_submission(&mut state);

        assert!(submission.is_none());
        assert_eq!(state.activity_status, "idle");
        assert_eq!(state.input, "   ");
    }

    #[test]
    fn renders_provider_mesh_as_individual_rows() {
        let backend = TestBackend::new(100, 22);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let state = TuiState {
            provider_health: vec![
                ProviderHealthView {
                    provider_id: "codex".to_string(),
                    status: "Unavailable".to_string(),
                    last_error: None,
                },
                ProviderHealthView {
                    provider_id: "gemini".to_string(),
                    status: "Healthy".to_string(),
                    last_error: None,
                },
                ProviderHealthView {
                    provider_id: "copilot".to_string(),
                    status: "Unavailable".to_string(),
                    last_error: None,
                },
                ProviderHealthView {
                    provider_id: "opencode".to_string(),
                    status: "Unavailable".to_string(),
                    last_error: None,
                },
            ],
            ..TuiState::default()
        };

        terminal.draw(|frame| render(frame, &state)).expect("draw");
        let buffer = terminal.backend().buffer();
        let rendered = format!("{buffer:?}");

        assert!(rendered.contains("codex"));
        assert!(rendered.contains("gemini"));
        assert!(rendered.contains("copilot"));
        assert!(rendered.contains("opencode"));
        assert!(!rendered.contains("ope..."));
    }

    #[test]
    fn cycles_selected_provider_before_session_exists() {
        let mut state = TuiState::default();

        assert_eq!(state.selected_provider(), "codex");
        state.cycle_provider();

        assert_eq!(state.selected_provider(), "gemini");
    }

    #[test]
    fn cycles_handoff_target_after_session_exists() {
        let mut state = TuiState {
            session_id: Some("task_1".to_string()),
            active_provider: Some("codex".to_string()),
            ..TuiState::default()
        };

        state.cycle_provider();

        assert_eq!(state.selected_provider(), "gemini");
        assert!(state.route_status().contains("codex active"));
        assert!(state.route_status().contains("target gemini"));
    }

    #[test]
    fn appends_handoff_response_and_updates_active_provider() {
        let mut state = TuiState::default();
        let response = json!({
            "handoff": {
                "from_provider_id": "codex",
                "to_provider_id": "gemini",
                "status": "Accepted"
            },
            "session": {
                "active_provider_id": "gemini"
            },
            "route_decision": {
                "reason": "Accepted handoff handoff_1 to gemini."
            }
        });

        append_handoff_response(&mut state, &response);

        assert_eq!(state.active_provider.as_deref(), Some("gemini"));
        assert!(state
            .transcript_text()
            .contains("handoff Accepted: codex -> gemini"));
        assert!(state
            .transcript_text()
            .contains("Accepted handoff handoff_1"));
    }

    #[test]
    fn appends_handoff_preview_and_records_pending_handoff() {
        let mut state = TuiState::default();
        let response = json!({
            "handoff": {
                "id": "handoff_1",
                "from_provider_id": "codex",
                "to_provider_id": "gemini",
                "summary_markdown": "# Handoff\n\nObjective: continue task\n\nChanged files: none\n",
                "mechanical_facts": {
                    "changed_files": [],
                    "commands_run": [],
                    "test_result": null,
                    "route_history": [],
                    "provider_errors": [],
                    "checkpoint_refs": [],
                    "user_constraints": []
                }
            }
        });

        append_handoff_preview(&mut state, &response);
        state.pending_handoff_id = Some("handoff_1".to_string());
        state.pending_handoff_session_id = Some("task_1".to_string());

        assert_eq!(state.pending_handoff_id.as_deref(), Some("handoff_1"));
        assert_eq!(state.pending_handoff_session_id.as_deref(), Some("task_1"));
        assert!(state
            .transcript_text()
            .contains("handoff preview: handoff_1 codex -> gemini"));
        assert!(state.transcript_text().contains("Objective: continue task"));
        assert!(state
            .transcript_text()
            .contains("press Ctrl-Y to accept handoff"));
    }

    #[test]
    fn appends_handoff_preview_detail_and_facts() {
        let mut state = TuiState::default();
        let response = json!({
            "handoff": {
                "id": "handoff_1",
                "from_provider_id": "codex",
                "to_provider_id": "gemini",
                "summary_markdown": "# Handoff\n\nObjective: continue task\n\nContext: keep routing stable\n\nFiles: a.rs\n\nRisk: medium\n\nNext: run tests\n\nNote 1\n\nNote 2\n\nNote 3\n\nNote 4\n\nNote 5\n\nNote 6\n",
                "mechanical_facts": {
                    "changed_files": ["a.rs", "b.rs", "c.rs", "d.rs", "e.rs", "f.rs"],
                    "commands_run": ["cargo test", "cargo fmt"],
                    "test_result": "cargo test passed",
                    "route_history": ["codex -> gemini"],
                    "provider_errors": ["429 rate limit"],
                    "checkpoint_refs": ["checkpoint_1"],
                    "user_constraints": ["keep TUI simple"]
                }
            }
        });

        append_handoff_preview(&mut state, &response);

        let transcript = state.transcript_text();
        assert!(transcript.contains("summary detail:"));
        assert!(transcript.contains("Objective: continue task"));
        assert!(transcript.contains("Note 3"));
        assert!(transcript.contains("(+1 more summary lines)"));
        assert!(transcript.contains("changed files: a.rs, b.rs, c.rs, d.rs, e.rs (+1 more)"));
        assert!(transcript.contains("commands: cargo test, cargo fmt"));
        assert!(transcript.contains("test result: cargo test passed"));
        assert!(transcript.contains("provider errors: 429 rate limit"));
        assert!(transcript.contains("constraints: keep TUI simple"));
    }

    #[test]
    fn handoff_status_shows_pending_handoff() {
        let state = TuiState {
            pending_handoff_id: Some("handoff_1".to_string()),
            pending_handoff_session_id: Some("task_1".to_string()),
            ..TuiState::default()
        };

        assert_eq!(
            state.handoff_status(),
            "> handoff_1 on task_1; Ctrl-Y accepts"
        );
    }

    #[test]
    fn accept_pending_handoff_without_preview_is_noop_message() {
        let mut state = TuiState::default();

        accept_pending_handoff(&mut state).expect("accept");

        assert!(state
            .transcript_text()
            .contains("create a handoff preview before accepting"));
    }

    #[test]
    fn selects_latest_session_from_response() {
        let response = json!({
            "sessions": [
                { "id": "task_old" },
                { "id": "task_new" }
            ]
        });

        let session = latest_session(&response).expect("session");

        assert_eq!(session["id"], "task_new");
    }

    #[test]
    fn applies_loaded_session_state() {
        let mut state = TuiState {
            pending_handoff_id: Some("handoff_1".to_string()),
            pending_handoff_session_id: Some("task_old".to_string()),
            ..TuiState::default()
        };
        let session = json!({
            "id": "task_1",
            "workspace_id": "ws_1",
            "objective": "continue this task",
            "active_provider_id": "gemini"
        });

        apply_loaded_session(&mut state, &session).expect("load");

        assert_eq!(state.session_id.as_deref(), Some("task_1"));
        assert_eq!(state.workspace_id.as_deref(), Some("ws_1"));
        assert_eq!(state.active_provider.as_deref(), Some("gemini"));
        assert!(state.pending_handoff_id.is_none());
        assert!(state.pending_handoff_session_id.is_none());
        assert!(state.transcript_text().contains("loaded session: task_1"));
        assert!(state
            .transcript_text()
            .contains("objective: continue this task"));
    }

    #[test]
    fn set_recent_sessions_selects_latest_and_status_marks_current() {
        let mut state = TuiState::default();
        state.set_recent_sessions(vec![
            SessionView {
                id: "task_1".to_string(),
                workspace_id: "ws_1".to_string(),
                objective: "write docs".to_string(),
                active_provider_id: Some("codex".to_string()),
                status: "Running".to_string(),
            },
            SessionView {
                id: "task_2".to_string(),
                workspace_id: "ws_1".to_string(),
                objective: "debug tests".to_string(),
                active_provider_id: Some("gemini".to_string()),
                status: "Failed".to_string(),
            },
        ]);
        state.session_id = Some("task_2".to_string());

        assert_eq!(state.selected_session_index, 1);
        assert_eq!(state.session_status(), "> [2/2] task_2* Failed gemini");
    }

    #[test]
    fn appends_recent_session_list_with_selected_marker() {
        let mut state = TuiState::default();
        state.set_recent_sessions(vec![
            SessionView {
                id: "task_1".to_string(),
                workspace_id: "ws_1".to_string(),
                objective: "write docs".to_string(),
                active_provider_id: Some("codex".to_string()),
                status: "Running".to_string(),
            },
            SessionView {
                id: "task_2".to_string(),
                workspace_id: "ws_1".to_string(),
                objective: "debug a very long failing provider session".to_string(),
                active_provider_id: Some("gemini".to_string()),
                status: "Failed".to_string(),
            },
        ]);

        append_session_list(&mut state);

        let transcript = state.transcript_text();
        assert!(transcript.contains("sessions:"));
        assert!(transcript.contains("    task_1 Running codex - write docs"));
        assert!(transcript
            .contains("  > task_2 Failed gemini - debug a very long failing provider se..."));
    }

    #[test]
    fn start_new_session_clears_session_binding_but_keeps_workspace() {
        let mut state = TuiState {
            session_id: Some("task_1".to_string()),
            workspace_id: Some("ws_1".to_string()),
            active_provider: Some("codex".to_string()),
            route_reason: Some("old route".to_string()),
            pending_handoff_id: Some("handoff_1".to_string()),
            pending_handoff_session_id: Some("task_1".to_string()),
            input: "draft prompt".to_string(),
            activity_status: "running codex".to_string(),
            ..TuiState::default()
        };

        start_new_session(&mut state);

        assert_eq!(state.workspace_id.as_deref(), Some("ws_1"));
        assert!(state.session_id.is_none());
        assert!(state.active_provider.is_none());
        assert!(state.route_reason.is_none());
        assert!(state.pending_handoff_id.is_none());
        assert!(state.pending_handoff_session_id.is_none());
        assert!(state.input.is_empty());
        assert_eq!(state.activity_status, "idle");
        assert!(state.transcript_text().contains("new session"));
    }

    #[test]
    fn cancel_current_session_without_session_is_noop_message() {
        let mut state = TuiState::default();

        cancel_current_session(&mut state).expect("cancel");

        assert!(state.session_id.is_none());
        assert!(state
            .transcript_text()
            .contains("start a session before canceling"));
    }

    #[test]
    fn appends_cancel_response_and_updates_session_state() {
        let mut state = TuiState {
            session_id: Some("task_1".to_string()),
            active_provider: Some("codex".to_string()),
            route_reason: Some("old route".to_string()),
            ..TuiState::default()
        };
        let response = json!({
            "session": {
                "id": "task_1",
                "status": "Canceled",
                "active_provider_id": "gemini"
            }
        });

        append_cancel_response(&mut state, &response);

        assert_eq!(state.session_id.as_deref(), Some("task_1"));
        assert_eq!(state.active_provider.as_deref(), Some("gemini"));
        assert!(state.route_reason.is_none());
        assert!(state.transcript_text().contains("session task_1: Canceled"));
    }

    #[test]
    fn appends_recent_route_history() {
        let mut state = TuiState::default();
        let response = json!({
            "routes": [
                {
                    "selected_provider_id": "codex",
                    "previous_provider_id": null,
                    "reason": "Selected codex."
                },
                {
                    "selected_provider_id": "gemini",
                    "previous_provider_id": "codex",
                    "reason": "Accepted handoff."
                }
            ]
        });

        append_route_history(&mut state, &response);

        assert!(state.transcript_text().contains("routes:"));
        assert!(state
            .transcript_text()
            .contains("none -> codex: Selected codex."));
        assert!(state
            .transcript_text()
            .contains("codex -> gemini: Accepted handoff."));
    }

    #[test]
    fn appends_clean_session_diff() {
        let mut state = TuiState::default();
        let response = json!({
            "diff": {
                "dirty": false,
                "changed_files": []
            }
        });

        append_session_diff(&mut state, &response);

        assert!(state.transcript_text().contains("workspace clean"));
    }

    #[test]
    fn appends_dirty_session_diff_with_file_limit() {
        let mut state = TuiState::default();
        let response = json!({
            "diff": {
                "dirty": true,
                "changed_files": [
                    "a.rs",
                    "b.rs",
                    "c.rs",
                    "d.rs",
                    "e.rs",
                    "f.rs"
                ]
            }
        });

        append_session_diff(&mut state, &response);

        assert!(state
            .transcript_text()
            .contains("workspace dirty: a.rs, b.rs, c.rs, d.rs, e.rs (+1 more)"));
    }

    #[test]
    fn append_prompt_response_shows_rate_limit_hint() {
        let mut state = TuiState::default();
        let response = json!({
            "status": "failed",
            "provider_id": "codex",
            "error": "429 Too Many Requests",
            "limit_inference": {
                "kind": "RateLimit",
                "confidence": "Estimated",
                "source": "ErrorInference"
            }
        });

        append_prompt_response(&mut state, &response);

        assert!(state
            .transcript_text()
            .contains("prompt result: failed via codex"));
        assert!(state
            .transcript_text()
            .contains("provider hint: rate limit detected"));
    }

    #[test]
    fn append_recent_events_groups_tool_and_output_sections() {
        let mut state = TuiState::default();
        let response = json!({
            "events": [
                {
                    "event_type": "session.agent.output",
                    "payload": { "text": "reading files" }
                },
                {
                    "event_type": "session.agent.tool_call",
                    "payload": { "text": "cargo test" }
                },
                {
                    "event_type": "session.agent.failed",
                    "payload": { "stderr": "tests failed" }
                }
            ]
        });

        append_recent_events(&mut state, &response);

        let transcript = state.transcript_text();
        assert!(transcript.contains("command/tool events:"));
        assert!(transcript.contains("  session.agent.tool_call: cargo test"));
        assert!(transcript.contains("agent output:"));
        assert!(transcript.contains("  session.agent.output: reading files"));
        assert!(transcript.contains("  session.agent.failed: tests failed"));
    }

    #[test]
    fn append_recent_events_with_sections_shows_test_results() {
        let mut state = TuiState::default();
        let response = json!({
            "events": [
                {
                    "event_type": "session.agent.output",
                    "payload": { "text": "cargo test --all passed" }
                },
                {
                    "event_type": "session.agent.output",
                    "payload": { "text": "ordinary output" }
                }
            ]
        });

        append_recent_events_with_sections(&mut state, &response);

        let transcript = state.transcript_text();
        assert!(transcript.contains("agent output:"));
        assert!(transcript.contains("test results:"));
        assert!(transcript.contains("  cargo test --all passed"));
    }

    #[test]
    fn append_prompt_response_shows_auth_and_timeout_hints() {
        let mut auth_state = TuiState::default();
        append_prompt_response(
            &mut auth_state,
            &json!({
                "status": "failed",
                "provider_id": "gemini",
                "stderr": "not authenticated; please run login"
            }),
        );
        assert!(auth_state
            .transcript_text()
            .contains("provider hint: authentication/setup required"));

        let mut timeout_state = TuiState::default();
        append_prompt_response(
            &mut timeout_state,
            &json!({
                "status": "failed",
                "provider_id": "codex",
                "error": "codex timed out after 10 seconds"
            }),
        );
        assert!(timeout_state
            .transcript_text()
            .contains("provider hint: command timed out"));
    }

    #[test]
    fn daemon_start_command_uses_daemon_subcommand() {
        assert_eq!(daemon_start_args(), ["daemon"]);
    }

    #[test]
    fn daemon_connected_message_uses_local_endpoint() {
        assert_eq!(
            daemon_connected_message(),
            "daemon: connected at 127.0.0.1:7878"
        );
    }

    #[test]
    fn parses_provider_ids_from_daemon_response() {
        let response = json!({
            "providers": [
                { "id": "gemini", "enabled": true },
                { "id": "codex", "enabled": true },
                { "id": "disabled", "enabled": false }
            ]
        });

        let providers = parse_provider_ids(&response).expect("providers");

        assert_eq!(providers, vec!["gemini", "codex"]);
    }

    #[test]
    fn parses_session_views_from_daemon_response() {
        let response = json!({
            "sessions": [
                {
                    "id": "task_1",
                    "workspace_id": "ws_1",
                    "objective": "write docs",
                    "active_provider_id": "codex",
                    "status": "Running"
                },
                {
                    "id": { "0": "task_2" },
                    "workspace_id": { "0": "ws_2" },
                    "objective": "debug tests",
                    "active_provider_id": { "0": "gemini" },
                    "status": "Failed"
                }
            ]
        });

        let sessions = parse_session_views(&response).expect("sessions");

        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].id, "task_1");
        assert_eq!(sessions[0].workspace_id, "ws_1");
        assert_eq!(sessions[0].active_provider_id.as_deref(), Some("codex"));
        assert_eq!(sessions[1].id, "task_2");
        assert_eq!(sessions[1].workspace_id, "ws_2");
        assert_eq!(sessions[1].active_provider_id.as_deref(), Some("gemini"));
    }

    #[test]
    fn parses_provider_health_from_daemon_response() {
        let response = json!({
            "health": [
                {
                    "provider_id": "gemini",
                    "status": "Healthy",
                    "last_error": null
                },
                {
                    "provider_id": "codex",
                    "status": "Unavailable",
                    "last_error": "missing command"
                }
            ]
        });

        let health = parse_provider_health(&response).expect("health");

        assert_eq!(
            health,
            vec![
                ProviderHealthView {
                    provider_id: "gemini".to_string(),
                    status: "Healthy".to_string(),
                    last_error: None,
                },
                ProviderHealthView {
                    provider_id: "codex".to_string(),
                    status: "Unavailable".to_string(),
                    last_error: Some("missing command".to_string()),
                },
            ]
        );
    }

    #[test]
    fn parses_pending_permissions_from_daemon_response() {
        let response = json!({
            "permissions": [
                {
                    "id": "perm_1",
                    "session_id": "task_1",
                    "command": "cargo test",
                    "reason": "verify changes",
                    "status": "Pending",
                    "risk": { "level": "Low" }
                },
                {
                    "id": { "0": "perm_2" },
                    "session_id": { "0": "task_2" },
                    "command": "cargo fmt",
                    "reason": "format changes",
                    "status": "Pending"
                }
            ]
        });

        let permissions = parse_permissions(&response).expect("permissions");

        assert_eq!(
            permissions,
            vec![
                PermissionView {
                    id: "perm_1".to_string(),
                    session_id: Some("task_1".to_string()),
                    command: "cargo test".to_string(),
                    reason: "verify changes".to_string(),
                    status: "Pending".to_string(),
                    risk_level: Some("Low".to_string()),
                },
                PermissionView {
                    id: "perm_2".to_string(),
                    session_id: Some("task_2".to_string()),
                    command: "cargo fmt".to_string(),
                    reason: "format changes".to_string(),
                    status: "Pending".to_string(),
                    risk_level: None,
                },
            ]
        );
    }

    #[test]
    fn permission_status_shows_selected_pending_permission() {
        let state = TuiState {
            pending_permissions: vec![PermissionView {
                id: "perm_1".to_string(),
                session_id: Some("task_1".to_string()),
                command: "cargo test --all-features".to_string(),
                reason: "verify changes".to_string(),
                status: "Pending".to_string(),
                risk_level: Some("Low".to_string()),
            }],
            ..TuiState::default()
        };

        assert_eq!(
            state.permission_status(),
            "> [1/1] cargo test --all-features risk Low task_1"
        );
    }

    #[test]
    fn set_pending_permissions_clamps_selection() {
        let mut state = TuiState {
            selected_permission_index: 3,
            ..TuiState::default()
        };

        state.set_pending_permissions(vec![PermissionView {
            id: "perm_1".to_string(),
            session_id: None,
            command: "cargo test".to_string(),
            reason: "verify".to_string(),
            status: "Pending".to_string(),
            risk_level: None,
        }]);

        assert_eq!(state.selected_permission_index, 0);
        assert_eq!(
            state.selected_permission().expect("permission").id,
            "perm_1"
        );
    }

    #[test]
    fn cycles_pending_permission_selection() {
        let mut state = TuiState {
            pending_permissions: vec![
                PermissionView {
                    id: "perm_1".to_string(),
                    session_id: None,
                    command: "cargo test".to_string(),
                    reason: "verify".to_string(),
                    status: "Pending".to_string(),
                    risk_level: None,
                },
                PermissionView {
                    id: "perm_2".to_string(),
                    session_id: None,
                    command: "cargo fmt".to_string(),
                    reason: "format".to_string(),
                    status: "Pending".to_string(),
                    risk_level: None,
                },
            ],
            ..TuiState::default()
        };

        state.select_next_permission();
        assert_eq!(
            state.selected_permission().expect("permission").id,
            "perm_2"
        );

        state.select_next_permission();
        assert_eq!(
            state.selected_permission().expect("permission").id,
            "perm_1"
        );

        state.select_previous_permission();
        assert_eq!(
            state.selected_permission().expect("permission").id,
            "perm_2"
        );
    }

    #[test]
    fn appends_permission_resolution_message() {
        let mut state = TuiState::default();
        let response = json!({
            "permission": {
                "id": "perm_1",
                "command": "cargo test",
                "status": "Approved"
            }
        });

        append_permission_resolution(&mut state, &response);

        assert!(state
            .transcript_text()
            .contains("permission Approved: perm_1 - cargo test"));
    }

    #[test]
    fn provider_status_includes_health() {
        let state = TuiState {
            provider_health: vec![
                ProviderHealthView {
                    provider_id: "codex".to_string(),
                    status: "Healthy".to_string(),
                    last_error: None,
                },
                ProviderHealthView {
                    provider_id: "gemini".to_string(),
                    status: "Unavailable".to_string(),
                    last_error: Some("missing".to_string()),
                },
            ],
            ..TuiState::default()
        };

        assert!(state.provider_status().contains(">codex:ok"));
        assert!(state.provider_status().contains("gemini:down"));
    }

    #[test]
    fn provider_status_marks_selected_and_active_providers() {
        let state = TuiState {
            selected_provider_index: 1,
            active_provider: Some("codex".to_string()),
            provider_health: vec![
                ProviderHealthView {
                    provider_id: "codex".to_string(),
                    status: "Healthy".to_string(),
                    last_error: None,
                },
                ProviderHealthView {
                    provider_id: "gemini".to_string(),
                    status: "Healthy".to_string(),
                    last_error: None,
                },
            ],
            ..TuiState::default()
        };

        let status = state.provider_status();

        assert!(status.contains(" codex:ok*"));
        assert!(status.contains(">gemini:ok"));
    }

    #[test]
    fn summarizes_provider_health() {
        let health = vec![
            ProviderHealthView {
                provider_id: "codex".to_string(),
                status: "Healthy".to_string(),
                last_error: None,
            },
            ProviderHealthView {
                provider_id: "gemini".to_string(),
                status: "Unavailable".to_string(),
                last_error: Some("missing".to_string()),
            },
        ];

        assert_eq!(summarize_provider_health(&health), "codex:ok, gemini:down");
    }

    #[test]
    fn status_line_reflects_activity() {
        let backend = TestBackend::new(80, 18);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let state = TuiState {
            activity_status: "running codex".to_string(),
            ..TuiState::default()
        };

        terminal.draw(|frame| render(frame, &state)).expect("draw");
        let buffer = terminal.backend().buffer();
        let rendered = format!("{buffer:?}");

        assert!(rendered.contains("running codex"));
    }

    #[test]
    fn push_message_appends_to_transcript() {
        let mut state = TuiState::default();
        let initial_len = state.transcript.len();
        state.push_message("hello");
        assert_eq!(state.transcript.len(), initial_len + 1);
        assert_eq!(state.transcript.last().unwrap(), "hello");
        state.push_message("world");
        assert_eq!(state.transcript.len(), initial_len + 2);
    }

    #[test]
    fn transcript_never_truncates() {
        let mut state = TuiState::default();
        let initial_len = state.transcript.len();
        for i in 0..100 {
            state.push_message(format!("line {i}"));
        }
        assert_eq!(state.transcript.len(), initial_len + 100);
        assert_eq!(state.transcript[initial_len], "line 0");
        assert_eq!(state.transcript[initial_len + 99], "line 99");
    }

    #[test]
    fn push_message_resets_scroll_offset() {
        let mut state = TuiState {
            scroll_offset: 50,
            ..TuiState::default()
        };
        state.push_message("new message");
        assert_eq!(state.scroll_offset, 0);
    }

    #[test]
    fn scroll_up_increments_offset() {
        let mut state = TuiState::default();
        for i in 0..50 {
            state.push_message(format!("line {i}"));
        }
        assert_eq!(state.scroll_offset, 0);
        state.scroll_up(10);
        assert_eq!(state.scroll_offset, 10);
        state.scroll_up(10);
        assert_eq!(state.scroll_offset, 20);
    }

    #[test]
    fn scroll_down_decrements_offset() {
        let mut state = TuiState::default();
        for i in 0..50 {
            state.push_message(format!("line {i}"));
        }
        state.scroll_offset = 20;
        state.scroll_down(10);
        assert_eq!(state.scroll_offset, 10);
    }

    #[test]
    fn scroll_offset_clamped_to_transcript_length() {
        let mut state = TuiState::default();
        state.push_message("only one line");
        state.scroll_up(100);
        assert_eq!(state.scroll_offset, 4);
    }

    #[test]
    fn scroll_to_bottom_resets_offset() {
        let mut state = TuiState::default();
        for i in 0..50 {
            state.push_message(format!("line {i}"));
        }
        state.scroll_offset = 30;
        state.scroll_to_bottom();
        assert_eq!(state.scroll_offset, 0);
    }

    #[test]
    fn scroll_to_top_sets_max_offset() {
        let mut state = TuiState::default();
        for i in 0..50 {
            state.push_message(format!("line {i}"));
        }
        state.scroll_to_top();
        assert_eq!(state.scroll_offset, 53);
    }

    #[test]
    fn renders_scrolled_session_title() {
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut state = TuiState::default();
        for i in 0..50 {
            state.push_message(format!("line {i}"));
        }
        state.scroll_offset = 10;

        terminal.draw(|frame| render(frame, &state)).expect("draw");
        let buffer = terminal.backend().buffer();
        let rendered = format!("{buffer:?}");

        assert!(rendered.contains("Agent Stream ("));
    }

    #[test]
    fn transcript_text_joins_with_newlines() {
        let state = TuiState {
            transcript: vec!["a".to_string(), "b".to_string(), "c".to_string()],
            ..TuiState::default()
        };
        assert_eq!(state.transcript_text(), "a\nb\nc");
    }
}
