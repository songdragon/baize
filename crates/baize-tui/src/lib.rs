use anyhow::{anyhow, Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::widgets::{Block, Borders, Paragraph};
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
    pub session: String,
    pub daemon_status: String,
    pub activity_status: String,
    pub providers: Vec<String>,
    pub provider_health: Vec<ProviderHealthView>,
    pub pending_permissions: Vec<PermissionView>,
    pub selected_permission_index: usize,
    pub selected_provider_index: usize,
    pub active_provider: Option<String>,
    pub route_reason: Option<String>,
    pub input: String,
    pub workspace_id: Option<String>,
    pub session_id: Option<String>,
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
}

impl Default for TuiState {
    fn default() -> Self {
        Self {
            workspace: "Baize MVP TUI".to_string(),
            session: "Type a prompt and press Enter.\nEsc or Ctrl-C quits.\n\nBaize starts the local daemon automatically when possible."
                .to_string(),
            daemon_status: "daemon: not checked".to_string(),
            activity_status: "idle".to_string(),
            providers: vec![
                "codex".to_string(),
                "gemini".to_string(),
                "copilot".to_string(),
                "opencode".to_string(),
            ],
            provider_health: Vec::new(),
            pending_permissions: Vec::new(),
            selected_permission_index: 0,
            selected_provider_index: 0,
            active_provider: None,
            route_reason: None,
            input: String::new(),
            workspace_id: None,
            session_id: None,
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
    let mut state = TuiState::default();
    state.daemon_status = daemon_status;
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
                    KeyCode::Down => state.select_next_permission(),
                    KeyCode::Up => state.select_previous_permission(),
                    KeyCode::Tab => state.cycle_provider(),
                    KeyCode::Char(ch) => state.input.push(ch),
                    KeyCode::Backspace => {
                        state.input.pop();
                    }
                    KeyCode::Enter => {
                        if let Err(error) = submit_prompt(&mut state) {
                            state.activity_status = "failed".to_string();
                            state.push_message(format!("error: {error:#}"));
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
            Constraint::Min(5),
            Constraint::Length(9),
        ])
        .split(frame.area());

    frame.render_widget(
        Paragraph::new(state.workspace.as_str())
            .block(Block::default().title("Workspace").borders(Borders::ALL)),
        chunks[0],
    );
    frame.render_widget(
        Paragraph::new(state.session.as_str())
            .block(Block::default().title("Session").borders(Borders::ALL)),
        chunks[1],
    );
    frame.render_widget(
        Paragraph::new(format!(
            "{}\nStatus: {}\nProviders: {}\nRoute: {}\nPerms: {}\nHelp: {}\n> {}",
            state.daemon_status,
            state.activity_status,
            state.provider_status(),
            state.route_status(),
            state.permission_status(),
            state.help_text(),
            state.input
        ))
        .block(Block::default().title("Status").borders(Borders::ALL)),
        chunks[2],
    );
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
            Some(PermissionView {
                id: id.to_string(),
                session_id: permission
                    .get("session_id")
                    .and_then(task_session_id)
                    .map(ToOwned::to_owned),
                command: command.to_string(),
                reason: reason.to_string(),
                status: status.to_string(),
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

fn submit_prompt(state: &mut TuiState) -> Result<()> {
    let prompt = state.input.trim().to_string();
    if prompt.is_empty() {
        return Ok(());
    }

    state.activity_status = format!("running {}", state.selected_provider());
    state.push_message(format!("> {prompt}"));
    let session_id = match state.session_id.clone() {
        Some(session_id) => session_id,
        None => {
            let workspace_id = ensure_workspace(state)?;
            ensure_session(state, &workspace_id, &prompt)?
        }
    };

    let response = post_json(
        &format!("/sessions/{session_id}/prompt"),
        json!({
            "prompt": prompt,
            "timeout_seconds": PROMPT_TIMEOUT_SECONDS,
        }),
    )?;
    append_prompt_response(state, &response);

    let events = get_json(&format!("/sessions/{session_id}/events"))?;
    append_recent_events(state, &events);
    refresh_route_history(state)?;
    refresh_session_diff(state)?;
    state.input.clear();
    state.activity_status = "idle".to_string();
    Ok(())
}

fn load_latest_session(state: &mut TuiState) -> Result<()> {
    state.activity_status = "loading latest session".to_string();
    let response = get_json("/sessions")?;
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
    state.input.clear();
    state.activity_status = "idle".to_string();
    state.push_message("new session: next prompt will create a fresh task");
}

fn latest_session(response: &Value) -> Option<&Value> {
    response.get("sessions")?.as_array()?.last()
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
    state.workspace = format!("Workspace: loaded\n{workspace_id}");
    state.push_message(format!("loaded session: {id} ({provider})"));
    state.push_message(format!("objective: {}", one_line(objective)));
    Ok(())
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
    let accepted = post_json(
        &format!("/sessions/{session_id}/handoff/{handoff_id}/accept"),
        json!({}),
    )?;
    append_handoff_response(state, &accepted);
    refresh_route_history(state)?;
    refresh_session_diff(state)?;
    state.activity_status = "idle".to_string();
    Ok(())
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
    state.push_message(format!("result: {status} via {provider}"));

    if let Some(error) = response.get("error").and_then(Value::as_str) {
        state.push_message(format!("error: {error}"));
    }
    if let Some(stderr) = response.get("stderr").and_then(Value::as_str) {
        if !stderr.trim().is_empty() {
            state.push_message(format!("stderr: {}", one_line(stderr)));
        }
    }
}

fn append_recent_events(state: &mut TuiState, response: &Value) {
    let Some(events) = response.get("events").and_then(Value::as_array) else {
        return;
    };

    for event in events
        .iter()
        .rev()
        .take(5)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
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

        match detail {
            Some(detail) if !detail.is_empty() => {
                state.push_message(format!("{event_type}: {detail}"))
            }
            _ => state.push_message(event_type.to_string()),
        }
    }
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

    fn provider_status(&self) -> String {
        let selected = self.selected_provider();
        self.providers
            .iter()
            .map(|provider| {
                let active = self.active_provider.as_deref() == Some(provider.as_str());
                let health = health_status_for(&self.provider_health, provider)
                    .map(short_health_status)
                    .unwrap_or("?");
                let label = format!("{provider}:{health}");
                match (provider == selected, active) {
                    (true, true) => format!("[{label}*]"),
                    (true, false) => format!("[{label}]"),
                    (false, true) => format!("{label}*"),
                    (false, false) => label,
                }
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

    fn permission_status(&self) -> String {
        let Some(permission) = self.selected_permission() else {
            return "none pending".to_string();
        };
        let session = permission
            .session_id
            .as_deref()
            .map(|session_id| format!(" {session_id}"))
            .unwrap_or_default();
        format!(
            "[{}/{}] {}{}",
            self.selected_permission_index + 1,
            self.pending_permissions.len(),
            short_text(&permission.command, 36),
            session
        )
    }

    fn help_text(&self) -> &'static str {
        "Enter|Tab|Up/Dn perm|^N new|^L load|^H hand|^P pull|^A ok|^D no"
    }

    fn push_message(&mut self, message: impl Into<String>) {
        let mut lines = self
            .session
            .lines()
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        lines.push(message.into());
        let keep_from = lines.len().saturating_sub(30);
        self.session = lines.split_off(keep_from).join("\n");
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
    if !status_line.contains(" 200 ") {
        return Err(anyhow!("daemon returned {status_line}"));
    }
    let value: Value = serde_json::from_str(body.trim()).context("parse daemon JSON response")?;
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

fn task_session_id(value: &Value) -> Option<&str> {
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
        let backend = TestBackend::new(80, 17);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let state = TuiState::default();

        terminal.draw(|frame| render(frame, &state)).expect("draw");
        let buffer = terminal.backend().buffer();
        let rendered = format!("{buffer:?}");

        assert!(rendered.contains("Baize MVP TUI"));
        assert!(rendered.contains("daemon: not checked"));
        assert!(rendered.contains("Status: idle"));
        assert!(rendered.contains("Providers: [codex:?], gemini:?, copilot:?, opencode:?"));
        assert!(rendered.contains("Perms: none pending"));
        assert!(rendered
            .contains("Help: Enter|Tab|Up/Dn perm|^N new|^L load|^H hand|^P pull|^A ok|^D no"));
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
    fn renders_prompt_input() {
        let backend = TestBackend::new(80, 18);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let state = TuiState {
            input: "hello baize".to_string(),
            ..TuiState::default()
        };

        terminal.draw(|frame| render(frame, &state)).expect("draw");
        let buffer = terminal.backend().buffer();
        let rendered = format!("{buffer:?}");

        assert!(rendered.contains("> hello baize"));
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
        assert!(state.session.contains("handoff Accepted: codex -> gemini"));
        assert!(state.session.contains("Accepted handoff handoff_1"));
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
        let mut state = TuiState::default();
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
        assert!(state.session.contains("loaded session: task_1"));
        assert!(state.session.contains("objective: continue this task"));
    }

    #[test]
    fn start_new_session_clears_session_binding_but_keeps_workspace() {
        let mut state = TuiState {
            session_id: Some("task_1".to_string()),
            workspace_id: Some("ws_1".to_string()),
            active_provider: Some("codex".to_string()),
            route_reason: Some("old route".to_string()),
            input: "draft prompt".to_string(),
            activity_status: "running codex".to_string(),
            ..TuiState::default()
        };

        start_new_session(&mut state);

        assert_eq!(state.workspace_id.as_deref(), Some("ws_1"));
        assert!(state.session_id.is_none());
        assert!(state.active_provider.is_none());
        assert!(state.route_reason.is_none());
        assert!(state.input.is_empty());
        assert_eq!(state.activity_status, "idle");
        assert!(state.session.contains("new session"));
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

        assert!(state.session.contains("routes:"));
        assert!(state.session.contains("none -> codex: Selected codex."));
        assert!(state.session.contains("codex -> gemini: Accepted handoff."));
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

        assert!(state.session.contains("workspace clean"));
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
            .session
            .contains("workspace dirty: a.rs, b.rs, c.rs, d.rs, e.rs (+1 more)"));
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
                    "status": "Pending"
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
                },
                PermissionView {
                    id: "perm_2".to_string(),
                    session_id: Some("task_2".to_string()),
                    command: "cargo fmt".to_string(),
                    reason: "format changes".to_string(),
                    status: "Pending".to_string(),
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
            }],
            ..TuiState::default()
        };

        assert_eq!(
            state.permission_status(),
            "[1/1] cargo test --all-features task_1"
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
                },
                PermissionView {
                    id: "perm_2".to_string(),
                    session_id: None,
                    command: "cargo fmt".to_string(),
                    reason: "format".to_string(),
                    status: "Pending".to_string(),
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
            .session
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

        assert!(state.provider_status().contains("[codex:ok]"));
        assert!(state.provider_status().contains("gemini:down"));
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
        let backend = TestBackend::new(80, 17);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let state = TuiState {
            activity_status: "running codex".to_string(),
            ..TuiState::default()
        };

        terminal.draw(|frame| render(frame, &state)).expect("draw");
        let buffer = terminal.backend().buffer();
        let rendered = format!("{buffer:?}");

        assert!(rendered.contains("Status: running codex"));
    }
}
