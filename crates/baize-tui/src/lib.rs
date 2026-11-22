use anyhow::{anyhow, Context, Result};
use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind,
};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Padding, Paragraph, Wrap};
use ratatui::{Frame, Terminal};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::io::{stdout, BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, OnceLock};
use std::thread::{self, sleep};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

static DAEMON_ADDRESS: OnceLock<SocketAddr> = OnceLock::new();
const PROMPT_TIMEOUT_SECONDS: u64 = 120;
const TRANSCRIPT_DETAIL_MAX_CHARS: usize = 180;
const DAEMON_START_ATTEMPTS: usize = 20;
const DAEMON_START_POLL_MS: u64 = 100;
const DAEMON_CONNECT_TIMEOUT_MS: u64 = 500;
const DAEMON_REQUEST_TIMEOUT_SECONDS: u64 = 8;
const PROMPT_RESPONSE_GRACE_SECONDS: u64 = 5;
const UI_EVENT_POLL_MS: u64 = 100;

#[derive(Debug, Clone)]
pub struct TuiState {
    pub(crate) ui: UiState,
    pub workspace: String,
    pub transcript: Vec<String>,
    pub scroll_offset: usize,
    pub daemon_status: String,
    pub activity_status: String,
    pub background_status: Option<String>,
    pub detail_mode: bool,
    pub providers: Vec<String>,
    pub provider_health: Vec<ProviderHealthView>,
    pub provider_diagnostics: Vec<ProviderDiagnosticView>,
    pub recent_sessions: Vec<SessionView>,
    pub selected_session_index: usize,
    pub pending_permissions: Vec<PermissionView>,
    pub runtime_policy: Option<RuntimePolicyView>,
    pub selected_permission_index: usize,
    pub selected_provider_index: usize,
    pub active_provider: Option<String>,
    pub route_reason: Option<String>,
    pub input: String,
    pub image_paths: Vec<PathBuf>,
    pub workspace_id: Option<String>,
    pub session_id: Option<String>,
    pub pending_handoff_id: Option<String>,
    pub pending_handoff_session_id: Option<String>,
    pub seen_event_ids: HashSet<String>,
    pub streaming_assistant_open: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderHealthView {
    pub provider_id: String,
    pub status: String,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderDiagnosticView {
    pub provider_id: String,
    pub readiness: String,
    pub health: ProviderHealthView,
    pub issues: Vec<String>,
    pub suggested_actions: Vec<String>,
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
pub struct RuntimePolicyView {
    pub command_policy: String,
    pub execution_mode: String,
    pub checkpoint_policy: String,
    pub sticky_window_minutes: u16,
    pub quota_switch_threshold_percent: u8,
    pub failure_threshold_count: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingPromptSubmission {
    prompt: String,
    provider_id: String,
    image_paths: Vec<PathBuf>,
    workspace_id: Option<String>,
    session_id: Option<String>,
}

struct PromptTask {
    receiver: Receiver<PromptTaskMessage>,
    stop_stream: Arc<AtomicBool>,
}

impl Drop for PromptTask {
    fn drop(&mut self) {
        self.stop_stream.store(true, Ordering::SeqCst);
    }
}

struct UiTask {
    receiver: Receiver<Result<UiTaskCompletion, String>>,
}

struct DaemonLaunch {
    status: String,
    guard: Option<AutoStartedDaemon>,
}

struct AutoStartedDaemon {
    _child: Child,
}

#[derive(Debug, Clone)]
struct PromptTaskCompletion {
    workspace_id: Option<String>,
    workspace_display: Option<String>,
    session_id: String,
    active_provider: Option<String>,
    route_reason: Option<String>,
    prompt_response: Value,
    events_response: Value,
    routes_response: Value,
    diff_response: Value,
}

#[derive(Debug, Clone)]
struct PromptTaskStarted {
    workspace_id: Option<String>,
    workspace_display: Option<String>,
    session_id: String,
    active_provider: Option<String>,
    route_reason: Option<String>,
    created_session: bool,
}

#[derive(Debug, Clone)]
enum PromptTaskMessage {
    Started(PromptTaskStarted),
    StreamEvent(Value),
    StreamUnavailable(String),
    StreamRecovered,
    NotSubmitted(String),
    Completed(PromptTaskCompletion),
    Failed(String),
}

#[derive(Debug, Clone)]
struct SubmissionSession {
    session_id: String,
    route_reason: Option<String>,
    created: bool,
}

#[derive(Debug)]
struct InitialLoadResult {
    providers: Result<Vec<String>, String>,
    diagnostics: Result<Vec<ProviderDiagnosticView>, String>,
    permissions: Result<Vec<PermissionView>, String>,
    runtime_policy: Result<RuntimePolicyView, String>,
}

#[derive(Debug)]
struct SelectedSessionLoad {
    session: Value,
    routes_response: Value,
    diff_response: Value,
    events_response: Value,
    workspace_display: String,
}

#[derive(Debug)]
enum UiTaskCompletion {
    InitialLoad(InitialLoadResult),
    PermissionRefresh(Vec<PermissionView>),
    SelectedSessionLoad(SelectedSessionLoad),
    WorkspaceLoad {
        workspace_id: String,
        display: String,
        sessions: Vec<SessionView>,
    },
    Action(Box<ActionResult>),
}

impl Default for TuiState {
    fn default() -> Self {
        Self {
            ui: UiState::default(),
            workspace: std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| "Local workspace".to_string()),
            transcript: vec!["What are we working on?".to_string()],
            scroll_offset: 0,
            daemon_status: "daemon: not checked".to_string(),
            activity_status: "idle".to_string(),
            background_status: None,
            detail_mode: false,
            providers: vec![
                "codex".to_string(),
                "antigravity".to_string(),
                "opencode".to_string(),
                "copilot".to_string(),
            ],
            provider_health: Vec::new(),
            provider_diagnostics: Vec::new(),
            recent_sessions: Vec::new(),
            selected_session_index: 0,
            pending_permissions: Vec::new(),
            runtime_policy: None,
            selected_permission_index: 0,
            selected_provider_index: 0,
            active_provider: None,
            route_reason: None,
            input: String::new(),
            image_paths: Vec::new(),
            workspace_id: None,
            session_id: None,
            pending_handoff_id: None,
            pending_handoff_session_id: None,
            seen_event_ids: HashSet::new(),
            streaming_assistant_open: false,
        }
    }
}

mod prompt;
use prompt::*;
mod output;
use output::*;
mod stream;
use stream::*;
mod commands;
use commands::*;
mod transcript_view;
use transcript_view::*;
mod pagination;
use pagination::*;
mod inspector;
use inspector::*;
mod workbench;
use workbench::*;
mod editor;
use editor::*;
mod actions;
use actions::*;
mod runtime;
use runtime::*;
mod tasks;
use tasks::*;
mod view;
use view::*;
mod providers;
use providers::*;
mod attachments;
use attachments::*;
mod submission;
use submission::*;
mod history;
use history::*;
mod handoff;
use handoff::*;
mod session;
use session::*;
mod transcript;
use transcript::*;
mod state;
use state::*;
mod transport;
pub use runtime::run;
use transport::*;
pub use view::render;

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::{Backend, TestBackend};

    #[test]
    fn renders_conversation_workbench() {
        let backend = TestBackend::new(80, 18);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let state = TuiState::default();

        terminal.draw(|frame| render(frame, &state)).expect("draw");
        let buffer = terminal.backend().buffer();
        let rendered = format!("{buffer:?}");

        assert!(rendered.contains("BAIZE"));
        assert!(rendered.contains("New conversation"));
        assert!(rendered.contains("daemon"));
        assert!(rendered.contains("not checked"));
        assert!(rendered.contains("Conversation"));
        assert!(rendered.contains("none · CLI"));
        assert!(rendered.contains("What are we working on?"));
        assert!(rendered.contains("codex"));
        assert!(!rendered.contains("Session · F7 next"));
        assert!(!rendered.contains("SESSION"));
        assert!(!rendered.contains("Prompt"));
        assert!(rendered.contains("Ask Baize to work on"));
        assert!(rendered.contains("Enter send"));
        assert!(rendered.contains("F8 details"));
    }

    #[test]
    fn renders_control_plane_in_detail_mode() {
        let backend = TestBackend::new(110, 38);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let state = TuiState {
            detail_mode: true,
            ..TuiState::default()
        };

        terminal.draw(|frame| render(frame, &state)).expect("draw");
        let buffer = terminal.backend().buffer();
        let rendered = format!("{buffer:?}");

        assert!(rendered.contains("Session · F7 next"));
        assert!(rendered.contains("CURRENT CONVERSATION"));
        assert!(rendered.contains("Target:"));
        assert!(rendered.contains("codex"));
        assert!(rendered.contains("opencode"));
        assert!(rendered.contains("Ctrl-H"));
    }

    #[test]
    fn renders_control_plane_for_pending_permission_in_compact_mode() {
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let state = TuiState {
            pending_permissions: vec![PermissionView {
                id: "perm_1".to_string(),
                session_id: None,
                command: "cargo test".to_string(),
                reason: "verify".to_string(),
                status: "Pending".to_string(),
                risk_level: None,
            }],
            ..TuiState::default()
        };

        terminal.draw(|frame| render(frame, &state)).expect("draw");
        let buffer = terminal.backend().buffer();
        let rendered = format!("{buffer:?}");

        assert!(rendered.contains("Permissions · F7 next"));
        assert!(rendered.contains("PENDING PERMISSIONS"));
        assert!(rendered.contains("cargo test"));
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
    fn parse_http_json_response_allows_structured_prompt_cancel() {
        let response = "HTTP/1.1 409 Conflict\r\ncontent-type: application/json\r\n\r\n{\"status\":\"canceled\",\"turn_status\":\"canceled\",\"error\":\"session canceled\"}";
        let value = parse_http_json_response(response).expect("structured cancel");

        assert_eq!(value["status"], "canceled");
        assert_eq!(value["turn_status"], "canceled");
        assert_eq!(value["error"], "session canceled");
    }

    #[test]
    fn parse_http_json_response_includes_error_for_non_prompt_failure() {
        let response = "HTTP/1.1 404 Not Found\r\ncontent-type: application/json\r\n\r\n{\"error\":\"session not found\"}";
        let error = parse_http_json_response(response).expect_err("error");

        assert!(error.to_string().contains("HTTP/1.1 404 Not Found"));
        assert!(error.to_string().contains("session not found"));
    }

    #[test]
    fn parse_http_json_response_reports_empty_body_without_json_eof() {
        let response = "HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n";
        let error = parse_http_json_response(response).expect_err("error");

        assert!(error.to_string().contains("empty body"));
        assert!(!error.to_string().contains("EOF"));
    }

    #[test]
    fn runtime_policy_error_hint_suggests_daemon_restart_for_missing_endpoint() {
        let error = anyhow!("daemon returned HTTP/1.1 404 Not Found with empty body");

        assert_eq!(
            runtime_policy_error_hint(&error),
            "restart baize daemon to enable runtime status"
        );
    }

    #[test]
    fn default_runtime_policy_uses_safe_ask_mode() {
        let policy = default_runtime_policy();

        assert_eq!(policy.command_policy, "ask");
        assert_eq!(policy.execution_mode, "ask_before_commands");
        assert_eq!(policy.sticky_window_minutes, 30);
    }

    #[test]
    fn poll_prompt_task_applies_successful_completion() {
        let mut state = TuiState {
            activity_status: "running codex".to_string(),
            ..TuiState::default()
        };
        let (sender, receiver) = mpsc::channel();
        sender
            .send(PromptTaskMessage::Completed(PromptTaskCompletion {
                workspace_id: Some("ws_1".to_string()),
                workspace_display: Some("Workspace: baize\nws_1".to_string()),
                session_id: "task_1".to_string(),
                active_provider: Some("codex".to_string()),
                route_reason: Some("Selected codex".to_string()),
                prompt_response: json!({
                    "status": "running",
                    "turn_status": "completed",
                    "session_status": "Running",
                    "provider_id": "codex"
                }),
                events_response: json!({
                    "events": [
                        {
                            "event_type": "session.agent.output",
                            "payload": { "text": "done" }
                        }
                    ]
                }),
                routes_response: json!({
                    "routes": [
                        {
                            "selected_provider_id": "codex",
                            "previous_provider_id": null,
                            "reason": "Selected codex"
                        }
                    ]
                }),
                diff_response: json!({
                    "diff": {
                        "dirty": false,
                        "changed_files": []
                    }
                }),
            }))
            .expect("send completion");
        let mut task = Some(PromptTask {
            receiver,
            stop_stream: Arc::new(AtomicBool::new(false)),
        });

        poll_prompt_task(&mut state, &mut task);

        assert!(task.is_none());
        assert_eq!(state.activity_status, "idle");
        let transcript = state.transcript_text();
        assert!(!transcript.contains("turn: completed via codex"));
        assert!(!transcript.contains("session: task_1"));
        assert!(!transcript.contains("route:"));
        assert!(transcript.contains("╭─ assistant:"));
        assert!(transcript.contains("  done"));
        assert!(!transcript.contains("╭─ routes:"));
        assert!(transcript.contains("workspace clean"));
    }

    #[test]
    fn poll_prompt_task_reports_worker_error() {
        let mut state = TuiState {
            activity_status: "running gemini".to_string(),
            ..TuiState::default()
        };
        let (sender, receiver) = mpsc::channel();
        sender
            .send(PromptTaskMessage::Failed("provider failed".to_string()))
            .expect("send error");
        let mut task = Some(PromptTask {
            receiver,
            stop_stream: Arc::new(AtomicBool::new(false)),
        });

        poll_prompt_task(&mut state, &mut task);

        assert!(task.is_none());
        assert_eq!(state.activity_status, "failed");
        assert!(state.transcript_text().contains("error: provider failed"));
    }

    #[test]
    fn apply_prompt_submission_context_records_created_session_metadata() {
        let mut state = TuiState {
            detail_mode: true,
            ..TuiState::default()
        };
        let started = PromptTaskStarted {
            workspace_id: Some("ws_1".to_string()),
            workspace_display: Some("Workspace: baize\nws_1".to_string()),
            session_id: "task_1".to_string(),
            active_provider: Some("codex".to_string()),
            route_reason: Some("Selected codex for implementation".to_string()),
            created_session: true,
        };

        apply_prompt_started(&mut state, started);

        assert_eq!(state.workspace_id.as_deref(), Some("ws_1"));
        assert_eq!(state.session_id.as_deref(), Some("task_1"));
        assert_eq!(state.active_provider.as_deref(), Some("codex"));
        assert_eq!(
            state.route_reason.as_deref(),
            Some("Selected codex for implementation")
        );
        assert!(state.transcript_text().contains("session: task_1 (codex)"));
        assert!(state
            .transcript_text()
            .contains("route: Selected codex for implementation"));
    }

    #[test]
    fn compact_prompt_submission_context_keeps_metadata_out_of_transcript() {
        let mut state = TuiState::default();
        let started = PromptTaskStarted {
            workspace_id: Some("ws_1".to_string()),
            workspace_display: Some("Workspace: baize\nws_1".to_string()),
            session_id: "task_1".to_string(),
            active_provider: Some("codex".to_string()),
            route_reason: Some("Selected codex for implementation".to_string()),
            created_session: true,
        };

        apply_prompt_started(&mut state, started);

        assert_eq!(state.session_id.as_deref(), Some("task_1"));
        assert_eq!(state.active_provider.as_deref(), Some("codex"));
        assert!(!state.transcript_text().contains("session: task_1"));
        assert!(!state.transcript_text().contains("route:"));
    }

    #[test]
    fn apply_initial_load_result_updates_status_without_blocking_state() {
        let mut state = TuiState::default();
        apply_initial_load_result(
            &mut state,
            InitialLoadResult {
                providers: Ok(vec!["opencode".to_string(), "codex".to_string()]),
                diagnostics: Ok(vec![ProviderDiagnosticView {
                    provider_id: "opencode".to_string(),
                    readiness: "Ready".to_string(),
                    health: ProviderHealthView {
                        provider_id: "opencode".to_string(),
                        status: "Healthy".to_string(),
                        last_error: None,
                    },
                    issues: Vec::new(),
                    suggested_actions: Vec::new(),
                }]),
                permissions: Ok(vec![PermissionView {
                    id: "perm_1".to_string(),
                    session_id: Some("task_1".to_string()),
                    command: "cargo test".to_string(),
                    reason: "verify".to_string(),
                    status: "Pending".to_string(),
                    risk_level: Some("Low".to_string()),
                }]),
                runtime_policy: Ok(RuntimePolicyView {
                    command_policy: "deny".to_string(),
                    execution_mode: "read_only".to_string(),
                    checkpoint_policy: "before_handoff".to_string(),
                    sticky_window_minutes: 10,
                    quota_switch_threshold_percent: 20,
                    failure_threshold_count: 2,
                }),
            },
        );

        assert_eq!(state.providers, vec!["opencode", "codex"]);
        assert_eq!(state.pending_permissions.len(), 1);
        assert_eq!(state.command_policy_label(), "read-only");
        assert!(!state.transcript_text().contains("status load complete"));
    }

    #[test]
    fn poll_ui_task_applies_permission_refresh_and_clears_background_status() {
        let mut state = TuiState {
            background_status: Some("refreshing permissions".to_string()),
            ..TuiState::default()
        };
        let (sender, receiver) = mpsc::channel();
        sender
            .send(Ok(UiTaskCompletion::PermissionRefresh(vec![
                PermissionView {
                    id: "perm_1".to_string(),
                    session_id: None,
                    command: "cargo fmt".to_string(),
                    reason: "format".to_string(),
                    status: "Pending".to_string(),
                    risk_level: None,
                },
            ])))
            .expect("send refresh");
        let mut task = Some(UiTask { receiver });

        poll_ui_task(&mut state, &mut task);

        assert!(task.is_none());
        assert!(state.background_status.is_none());
        assert_eq!(state.pending_permissions.len(), 1);
        assert!(state
            .transcript_text()
            .contains("permissions refreshed: 1 pending"));
    }

    #[test]
    fn detach_prompt_task_after_cancel_drops_worker() {
        let mut state = TuiState::default();
        let (_sender, receiver) = mpsc::channel();
        let mut task = Some(PromptTask {
            receiver,
            stop_stream: Arc::new(AtomicBool::new(false)),
        });

        detach_prompt_task_after_cancel(&mut state, &mut task);

        assert!(task.is_none());
        assert!(state
            .transcript_text()
            .contains("prompt worker detached after cancel"));
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

        assert!(!rendered.contains("Ask Baize"));
        assert!(rendered.contains("hello baize"));
        assert!(rendered.contains("codex"));
        let cursor = terminal
            .backend_mut()
            .get_cursor_position()
            .expect("cursor");
        assert_eq!(cursor, Position::new(15, 15));
    }

    #[test]
    fn prompt_cursor_accounts_for_wide_input_characters() {
        let area = Rect::new(0, 15, 80, 4);
        let state = TuiState {
            input: "你好".to_string(),
            ..TuiState::default()
        };

        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal
            .draw(|frame| render_composer(frame, area, &state))
            .unwrap();
        assert_eq!(
            terminal.backend_mut().get_cursor_position().unwrap(),
            Position::new(8, 16)
        );
    }

    #[test]
    fn editor_wraps_without_losing_input() {
        let state = TuiState {
            input: "abcdefghijklmnopqrstuvwxyz".into(),
            ..TuiState::default()
        };
        let (rows, row, col) = editor_rows(&state, 8);
        assert_eq!(rows.join(""), "abcdefghijklmnopqrstuvwxyz");
        assert_eq!((row, col), (3, 5));
    }

    #[test]
    fn prompt_request_body_includes_target_provider() {
        let image_paths = vec![PathBuf::from("/tmp/screen.png")];
        let body = prompt_request_body("gemini", "continue task", &image_paths);

        assert_eq!(body["prompt"], "continue task");
        assert_eq!(body["provider_id"], "gemini");
        assert_eq!(body["image_paths"][0], "/tmp/screen.png");
        assert_eq!(body["timeout_seconds"], PROMPT_TIMEOUT_SECONDS);
    }

    #[test]
    fn image_paths_from_text_parses_shell_paths_and_file_urls() {
        let paths = image_paths_from_text(
            r#""/tmp/screen shot.png" file:///tmp/clip%20image.jpg /tmp/readme.txt"#,
        );

        assert_eq!(
            paths,
            vec![
                PathBuf::from("/tmp/screen shot.png"),
                PathBuf::from("/tmp/clip image.jpg")
            ]
        );
    }

    #[test]
    fn handle_paste_attaches_image_paths() {
        let image_path = std::env::temp_dir().join(format!(
            "baize-tui-paste-{}.png",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
        ));
        std::fs::write(&image_path, b"fake image").expect("image");
        let mut state = TuiState::default();

        handle_paste(&mut state, &image_path.to_string_lossy());

        assert_eq!(state.image_paths, vec![image_path.canonicalize().unwrap()]);
        assert!(state.input.is_empty());
        assert!(state.transcript_text().contains("image attached:"));
        std::fs::remove_file(image_path).ok();
    }

    #[test]
    fn handle_paste_appends_plain_text() {
        let mut state = TuiState::default();

        handle_paste(&mut state, "hello\nbaize");

        assert_eq!(state.input, "hello\nbaize");
        assert!(state.image_paths.is_empty());
    }

    #[test]
    fn parse_sse_data_line_reads_json_payload() {
        let event = parse_sse_data_line(
            r#"data: {"event_type":"session.agent.output","payload":{"text":"hello"}}"#,
        )
        .expect("event");

        assert_eq!(event["event_type"], "session.agent.output");
        assert_eq!(event["payload"]["text"], "hello");
    }

    #[test]
    fn apply_prompt_stream_event_appends_output_once() {
        let mut state = TuiState::default();
        let event = json!({
            "id": "evt_1",
            "event_type": "session.agent.output",
            "session_id": "task_1",
            "provider_id": "codex",
            "payload": { "text": "stream hello" }
        });

        apply_prompt_stream_event(&mut state, &event);
        apply_prompt_stream_event(&mut state, &event);

        let transcript = state.transcript_text();
        assert!(transcript.contains("assistant:"));
        assert_eq!(transcript.matches("stream hello").count(), 1);
    }

    #[test]
    fn apply_prompt_stream_event_shows_all_arrived_text_immediately() {
        let mut state = TuiState::default();
        let event = json!({
            "id": "evt_long",
            "event_type": "session.agent.output",
            "session_id": "task_1",
            "provider_id": "codex",
            "payload": {
                "text": "abcdefghijklmnopqrstuvwxyz0123456789TAIL"
            }
        });

        apply_prompt_stream_event(&mut state, &event);

        let partial = state.transcript_text();
        assert!(partial.contains("assistant:"));
        assert!(partial.contains("abcdefghijklmnop"));
        assert!(partial.contains("TAIL"));

        assert!(state
            .transcript_text()
            .contains("abcdefghijklmnopqrstuvwxyz0123456789TAIL"));
    }

    #[test]
    fn apply_prompt_stream_event_shows_raw_progress_without_text() {
        let mut state = TuiState::default();
        let event = json!({
            "id": "evt_raw",
            "event_type": "session.agent.output",
            "session_id": "task_1",
            "provider_id": "opencode",
            "payload": {
                "text": null,
                "raw": {
                    "type": "step_start",
                    "part": { "type": "step-start" }
                }
            }
        });

        apply_prompt_stream_event(&mut state, &event);

        let transcript = state.transcript_text();
        assert!(transcript.contains("status:"));
        assert!(transcript.contains("stream: step-start"));
    }

    #[test]
    fn parses_runtime_policy_from_daemon_response() {
        let policy = parse_runtime_policy(&json!({
            "runtime": {
                "command_policy": "allow_project",
                "execution_mode": "project_write_allowed",
                "checkpoint_policy": "before_handoff",
                "routing": {
                    "sticky_window_minutes": 45,
                    "quota_switch_threshold_percent": 15,
                    "failure_threshold_count": 2
                }
            }
        }))
        .expect("policy");

        assert_eq!(policy.command_policy, "allow_project");
        assert_eq!(policy.execution_mode, "project_write_allowed");
        assert_eq!(policy.checkpoint_policy, "before_handoff");
        assert_eq!(policy.sticky_window_minutes, 45);
        assert_eq!(policy.quota_switch_threshold_percent, 15);
        assert_eq!(policy.failure_threshold_count, 2);
    }

    #[test]
    fn parse_runtime_policy_rejects_missing_runtime_object() {
        let error = parse_runtime_policy(&json!({ "status": "ok" })).expect_err("error");

        assert!(error.to_string().contains("runtime"));
    }

    #[test]
    fn begin_prompt_submission_immediately_shows_running_state() {
        let mut state = TuiState {
            selected_provider_index: 1,
            input: "你能做什么".to_string(),
            runtime_policy: Some(RuntimePolicyView {
                command_policy: "deny".to_string(),
                execution_mode: "read_only".to_string(),
                checkpoint_policy: "before_handoff".to_string(),
                sticky_window_minutes: 30,
                quota_switch_threshold_percent: 10,
                failure_threshold_count: 3,
            }),
            ..TuiState::default()
        };

        let submission = begin_prompt_submission(&mut state).expect("submission");

        assert_eq!(submission.provider_id, "antigravity");
        assert_eq!(submission.prompt, "你能做什么");
        assert!(submission.image_paths.is_empty());
        assert!(state.input.is_empty());
        assert_eq!(state.activity_status, "running antigravity");
        assert!(!state.transcript_text().contains("╭─ task"));
        assert!(state.transcript_text().contains("> 你能做什么"));
        assert!(state.transcript_text().contains("antigravity thinking..."));
    }

    #[test]
    fn begin_prompt_submission_includes_attached_images() {
        let image_path = PathBuf::from("/tmp/screen.png");
        let mut state = TuiState {
            input: "describe this".to_string(),
            image_paths: vec![image_path.clone()],
            ..TuiState::default()
        };

        let submission = begin_prompt_submission(&mut state).expect("submission");

        assert_eq!(submission.image_paths, vec![image_path]);
        assert!(state.image_paths.is_empty());
        assert!(state.transcript_text().contains("attached 1 image(s)"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_clipboard_image_osascript_args_write_png_to_path() {
        let args = macos_clipboard_image_osascript_args(Path::new("/tmp/baize clip.png"));
        let script = args.join("\n");

        assert!(script.contains("set imageData to the clipboard as «class PNGf»"));
        assert!(script.contains("POSIX file \"/tmp/baize clip.png\""));
        assert!(script.contains("write imageData to outFile"));
    }

    #[test]
    fn transcript_section_labels_are_mapped_for_visual_rendering() {
        assert_eq!(transcript_section_label("assistant:"), Some("assistant"));
        assert_eq!(
            transcript_section_label("command/tool events:"),
            Some("tools")
        );
        assert_eq!(transcript_section_label("routes:"), Some("routes"));
        assert_eq!(transcript_section_label("plain line"), None);
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
    fn begin_prompt_submission_blocks_known_down_provider() {
        let mut state = TuiState {
            input: "summarize project".to_string(),
            provider_health: vec![ProviderHealthView {
                provider_id: "codex".to_string(),
                status: "Unavailable".to_string(),
                last_error: Some("failed to spawn codex".to_string()),
            }],
            ..TuiState::default()
        };

        let submission = begin_prompt_submission(&mut state);

        assert!(submission.is_none());
        assert_eq!(state.activity_status, "blocked");
        assert_eq!(state.input, "summarize project");
        assert!(state
            .transcript_text()
            .contains("provider codex is not ready: failed to spawn codex"));
        assert!(!state.transcript_text().contains("mesh: dispatching"));
    }

    #[test]
    fn begin_prompt_submission_blocks_unsupported_runtime_provider() {
        let mut state = TuiState {
            input: "summarize project".to_string(),
            selected_provider_index: 3,
            provider_diagnostics: vec![ProviderDiagnosticView {
                provider_id: "copilot".to_string(),
                readiness: "UnsupportedRuntime".to_string(),
                health: ProviderHealthView {
                    provider_id: "copilot".to_string(),
                    status: "Healthy".to_string(),
                    last_error: None,
                },
                issues: vec![
                    "Baize prompt runtime is not implemented for this provider yet".to_string(),
                ],
                suggested_actions: Vec::new(),
            }],
            provider_health: vec![ProviderHealthView {
                provider_id: "copilot".to_string(),
                status: "Healthy".to_string(),
                last_error: None,
            }],
            ..TuiState::default()
        };

        let submission = begin_prompt_submission(&mut state);

        assert!(submission.is_none());
        assert_eq!(state.input, "summarize project");
        assert!(state
            .transcript_text()
            .contains("provider copilot is not ready: Baize prompt runtime"));
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
                    provider_id: "antigravity".to_string(),
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
        assert!(!rendered.contains("antigr..."));
        assert!(!rendered.contains("Session · F7 next"));
    }

    #[test]
    fn renders_provider_mesh_as_individual_rows_in_detail_mode() {
        let backend = TestBackend::new(110, 38);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let state = TuiState {
            detail_mode: true,
            provider_health: vec![
                ProviderHealthView {
                    provider_id: "codex".to_string(),
                    status: "Unavailable".to_string(),
                    last_error: None,
                },
                ProviderHealthView {
                    provider_id: "antigravity".to_string(),
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

        assert!(rendered.contains("Session · F7 next"));
        assert!(rendered.contains("codex"));
        assert!(rendered.contains("antigravity"));
        assert!(rendered.contains("copilot"));
        assert!(rendered.contains("opencode"));
        assert!(!rendered.contains("ope..."));
    }

    #[test]
    fn renders_runtime_policy_in_control_plane() {
        let backend = TestBackend::new(120, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let state = TuiState {
            detail_mode: true,
            runtime_policy: Some(RuntimePolicyView {
                command_policy: "allow_project".to_string(),
                execution_mode: "project_write_allowed".to_string(),
                checkpoint_policy: "before_handoff".to_string(),
                sticky_window_minutes: 45,
                quota_switch_threshold_percent: 15,
                failure_threshold_count: 2,
            }),
            ..TuiState::default()
        };

        terminal.draw(|frame| render(frame, &state)).expect("draw");
        let buffer = terminal.backend().buffer();
        let rendered = format!("{buffer:?}");

        assert!(rendered.contains("project-write-allowed"));
        assert!(rendered.contains("project writes allowed"));
        assert!(rendered.contains("45m quota<15% fail>2"));
    }

    #[test]
    fn cycles_selected_provider_before_session_exists() {
        let mut state = TuiState::default();

        assert_eq!(state.selected_provider(), "codex");
        state.cycle_provider();

        assert_eq!(state.selected_provider(), "antigravity");
    }

    #[test]
    fn cycles_handoff_target_after_session_exists() {
        let mut state = TuiState {
            session_id: Some("task_1".to_string()),
            active_provider: Some("codex".to_string()),
            ..TuiState::default()
        };

        state.cycle_provider();

        assert_eq!(state.selected_provider(), "antigravity");
        let overview = inspector_lines(&state, Panel::Overview)
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(overview.contains("Active: codex"));
        assert!(overview.contains("Target: antigravity"));
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

        apply_action(
            &mut state,
            ActionResult {
                action: Action::PreviewHandoff,
                session_id: None,
                response,
                extra: Value::Null,
            },
        );
        state.pending_handoff_id = Some("handoff_1".to_string());
        state.pending_handoff_session_id = Some("task_1".to_string());

        assert_eq!(state.pending_handoff_id.as_deref(), Some("handoff_1"));
        assert_eq!(state.pending_handoff_session_id.as_deref(), Some("task_1"));
        let view = inspector_lines(&state, Panel::Handoff)
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(view.contains("From: codex") && view.contains("To: gemini"));
        assert!(view.contains("Objective: continue task") && view.contains("Ctrl-Y"));
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

        apply_action(
            &mut state,
            ActionResult {
                action: Action::PreviewHandoff,
                session_id: None,
                response,
                extra: Value::Null,
            },
        );

        let view = inspector_lines(&state, Panel::Handoff)
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        for text in [
            "Objective: continue task",
            "Note 6",
            "f.rs",
            "cargo fmt",
            "429 rate limit",
            "keep TUI simple",
        ] {
            assert!(view.contains(text), "{text}");
        }
        assert!(!view.contains("more summary lines"));
    }

    #[test]
    fn accept_pending_handoff_without_preview_is_noop_message() {
        let mut state = TuiState {
            session_id: Some("task_1".into()),
            ..TuiState::default()
        };
        request_action(&mut state, &mut None, Action::AcceptHandoff);

        assert!(state
            .ui
            .notice
            .as_deref()
            .unwrap()
            .contains("review a handoff"));
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
        assert_eq!(
            state.recent_sessions[state.selected_session_index].id,
            "task_2"
        );
    }

    #[test]
    fn recent_session_selection_cycles_without_loading_session() {
        let mut state = TuiState {
            session_id: Some("task_current".to_string()),
            ..TuiState::default()
        };
        state.set_recent_sessions(vec![
            SessionView {
                id: "task_1".to_string(),
                workspace_id: "ws_1".to_string(),
                objective: "first".to_string(),
                active_provider_id: Some("codex".to_string()),
                status: "Running".to_string(),
            },
            SessionView {
                id: "task_2".to_string(),
                workspace_id: "ws_1".to_string(),
                objective: "second".to_string(),
                active_provider_id: Some("gemini".to_string()),
                status: "Completed".to_string(),
            },
        ]);

        assert_eq!(state.selected_session_index, 1);
        assert!(state.select_next_recent_session());
        assert_eq!(state.selected_session_index, 0);
        assert_eq!(state.session_id.as_deref(), Some("task_current"));
        assert!(state.select_previous_recent_session());
        assert_eq!(state.selected_session_index, 1);
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
    fn append_complete_response_updates_session_and_clears_pending_handoff() {
        let mut state = TuiState {
            pending_handoff_id: Some("handoff_1".to_string()),
            pending_handoff_session_id: Some("task_1".to_string()),
            route_reason: Some("old route".to_string()),
            ..TuiState::default()
        };

        append_complete_response(
            &mut state,
            &json!({
                "session": {
                    "id": "task_1",
                    "status": "Completed",
                    "active_provider_id": "codex"
                }
            }),
        );

        assert_eq!(state.session_id.as_deref(), Some("task_1"));
        assert_eq!(state.active_provider.as_deref(), Some("codex"));
        assert!(state.pending_handoff_id.is_none());
        assert!(state.pending_handoff_session_id.is_none());
        assert!(state.route_reason.is_none());
        assert!(state
            .transcript_text()
            .contains("session task_1: Completed"));
    }

    #[test]
    fn cancel_current_session_without_session_is_noop_message() {
        let mut state = TuiState::default();

        request_action(&mut state, &mut None, Action::Cancel);

        assert!(state.session_id.is_none());
        assert!(state
            .ui
            .notice
            .as_deref()
            .unwrap()
            .contains("session first"));
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
        let mut state = TuiState {
            detail_mode: true,
            ..TuiState::default()
        };
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
        let mut state = TuiState {
            detail_mode: true,
            ..TuiState::default()
        };
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
    fn compact_session_diff_shows_count_not_file_list() {
        let mut state = TuiState::default();
        let response = json!({
            "diff": {
                "dirty": true,
                "changed_files": ["a.rs", "b.rs", "c.rs"]
            }
        });

        append_session_diff(&mut state, &response);

        let transcript = state.transcript_text();
        assert!(transcript.contains("changed files: 3"));
        assert!(transcript.contains("F8 details"));
        assert!(!transcript.contains("a.rs, b.rs"));
    }

    #[test]
    fn append_prompt_response_shows_rate_limit_hint() {
        let mut state = TuiState {
            detail_mode: true,
            ..TuiState::default()
        };
        let response = json!({
            "status": "failed",
            "turn_status": "failed",
            "session_status": "Failed",
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
            .contains("turn result: failed via codex (session Failed)"));
        assert!(state
            .transcript_text()
            .contains("provider hint: rate limit detected"));
    }

    #[test]
    fn append_prompt_response_shows_completed_turn_status() {
        let mut state = TuiState {
            detail_mode: true,
            ..TuiState::default()
        };
        let response = json!({
            "status": "running",
            "turn_status": "completed",
            "session_status": "Running",
            "provider_id": "antigravity"
        });

        append_prompt_response(&mut state, &response);

        assert!(state
            .transcript_text()
            .contains("turn result: completed via antigravity (session Running)"));
    }

    #[test]
    fn append_prompt_response_shows_context_status() {
        let mut state = TuiState {
            detail_mode: true,
            ..TuiState::default()
        };
        let response = json!({
            "status": "running",
            "turn_status": "completed",
            "session_status": "Running",
            "provider_id": "opencode",
            "context": {
                "strategy": "CrossProviderDelta",
                "estimated_tokens": 3210,
                "truncated_sections": ["diff_hunks"]
            }
        });

        append_prompt_response(&mut state, &response);

        let transcript = state.transcript_text();
        assert!(transcript.contains("context: cross-provider delta 3210 est tokens"));
        assert!(transcript.contains("truncated diff_hunks"));
    }

    #[test]
    fn session_events_path_filters_to_prompt_provider() {
        assert_eq!(
            session_events_path("task_1", "opencode"),
            "/sessions/task_1/events?provider_id=opencode"
        );
    }

    #[test]
    fn append_prompt_response_hides_success_stderr_noise() {
        let mut state = TuiState {
            detail_mode: true,
            ..TuiState::default()
        };
        let response = json!({
            "status": "running",
            "turn_status": "completed",
            "session_status": "Running",
            "provider_id": "codex",
            "stderr": "Reading additional input from stdin..."
        });

        append_prompt_response(&mut state, &response);

        let transcript = state.transcript_text();
        assert!(transcript.contains("turn result: completed via codex"));
        assert!(!transcript.contains("Reading additional input"));
    }

    #[test]
    fn append_prompt_response_keeps_legacy_status_display() {
        let mut state = TuiState {
            detail_mode: true,
            ..TuiState::default()
        };
        let response = json!({
            "status": "failed",
            "provider_id": "codex"
        });

        append_prompt_response(&mut state, &response);

        assert!(state
            .transcript_text()
            .contains("prompt result: failed via codex"));
    }

    #[test]
    fn prompt_activity_after_response_reflects_turn_failure() {
        assert_eq!(
            prompt_activity_after_response(&json!({ "turn_status": "failed" })),
            "failed"
        );
        assert_eq!(
            prompt_activity_after_response(&json!({ "turn_status": "completed" })),
            "idle"
        );
    }

    #[test]
    fn append_prompt_response_truncates_long_error_detail() {
        let mut state = TuiState::default();
        let long_error = "x".repeat(TRANSCRIPT_DETAIL_MAX_CHARS + 40);
        let response = json!({
            "status": "failed",
            "provider_id": "gemini",
            "stderr": long_error
        });

        append_prompt_response(&mut state, &response);

        let transcript = state.transcript_text();
        assert!(transcript.contains("stderr: "));
        assert!(transcript.contains("..."));
        assert!(!transcript.contains(&"x".repeat(TRANSCRIPT_DETAIL_MAX_CHARS + 20)));
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
                },
                {
                    "event_type": "session.agent.completed",
                    "payload": { "stderr": "Reading additional input from stdin..." }
                }
            ]
        });

        append_recent_events(&mut state, &response);

        let transcript = state.transcript_text();
        assert!(transcript.contains("tool │ cargo test"));
        assert!(transcript.contains("cargo test"));
        assert!(transcript.contains("╭─ assistant:"));
        assert!(transcript.contains("  reading files"));
        assert!(transcript.contains("error │"));
        assert!(transcript.contains("agent failed: tests failed"));
        assert!(!transcript.contains("Reading additional input"));
        assert!(!transcript.contains("session.agent.output: reading files"));
    }

    #[test]
    fn append_recent_events_preserves_long_assistant_output() {
        let mut state = TuiState::default();
        let long_output = format!("{} tail", "o".repeat(TRANSCRIPT_DETAIL_MAX_CHARS + 40));
        let response = json!({
            "events": [
                {
                    "event_type": "session.agent.output",
                    "payload": { "text": long_output }
                }
            ]
        });

        append_recent_events(&mut state, &response);

        let transcript = state.transcript_text();
        assert!(transcript.contains("assistant:"));
        assert!(transcript.contains("tail"));
        assert!(!transcript.contains("..."));
    }

    #[test]
    fn append_recent_events_preserves_all_assistant_lines() {
        let mut state = TuiState::default();
        let response = json!({
            "events": [
                { "event_type": "session.agent.output", "payload": { "text": "line 1" } },
                { "event_type": "session.agent.output", "payload": { "text": "line 2" } },
                { "event_type": "session.agent.output", "payload": { "text": "line 3" } },
                { "event_type": "session.agent.output", "payload": { "text": "line 4" } },
                { "event_type": "session.agent.output", "payload": { "text": "line 5" } },
                { "event_type": "session.agent.output", "payload": { "text": "line 6" } }
            ]
        });

        append_recent_events(&mut state, &response);

        let transcript = state.transcript_text();
        assert!(transcript.contains("  line 1"));
        assert!(transcript.contains("  line 5"));
        assert!(transcript.contains("  line 6"));
        assert!(!transcript.contains("  (+1 more)"));
    }

    #[test]
    fn append_recent_events_preserves_multiline_assistant_output() {
        let mut state = TuiState::default();
        let response = json!({
            "events": [
                {
                    "event_type": "session.agent.output",
                    "payload": {
                        "text": "```cpp\nint main() {\n    return 0;\n}\n```"
                    }
                }
            ]
        });

        append_recent_events(&mut state, &response);

        let transcript = state.transcript_text();
        assert!(transcript.contains("  ```cpp"));
        assert!(transcript.contains("  int main() {"));
        assert!(transcript.contains("      return 0;"));
        assert!(transcript.contains("  }"));
    }

    #[test]
    fn append_recent_events_shows_tool_result_without_reclassifying_it_as_reply() {
        let mut state = TuiState::default();
        let response = json!({
            "events": [
                {
                    "event_type": "session.agent.tool_call",
                    "payload": { "text": "cargo test --all passed" }
                },
                {
                    "event_type": "session.agent.output",
                    "payload": { "text": "ordinary output" }
                }
            ]
        });

        append_recent_events(&mut state, &response);

        let transcript = state.transcript_text();
        assert!(transcript.contains("assistant:"));
        assert!(transcript.contains("tool │ cargo test --all passed"));
        assert!(transcript.contains("cargo test --all passed"));
    }

    #[test]
    fn append_recent_events_does_not_treat_assistant_suggestions_as_test_results() {
        let mut state = TuiState::default();
        let response = json!({
            "events": [
                {
                    "event_type": "session.agent.output",
                    "payload": {
                        "text": "I can help run cargo test --workspace when you ask."
                    }
                }
            ]
        });

        append_recent_events(&mut state, &response);

        let transcript = state.transcript_text();
        assert!(transcript.contains("assistant:"));
        assert!(!transcript.contains("test results:"));
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
    fn daemon_auto_started_message_marks_owned_daemon() {
        assert_eq!(
            daemon_auto_started_message(),
            "daemon: connected at 127.0.0.1:7878 (auto-started)"
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
    fn parses_provider_diagnostics_from_daemon_response() {
        let response = json!({
            "diagnostics": [
                {
                    "provider_id": "codex",
                    "readiness": "SetupRequired",
                    "health": {
                        "provider_id": "codex",
                        "status": "Unavailable",
                        "last_error": "missing codex"
                    },
                    "issues": ["provider executable is not ready: missing codex"],
                    "suggested_actions": ["install `codex` or add it to PATH"]
                }
            ]
        });

        let diagnostics = parse_provider_diagnostics(&response).expect("diagnostics");

        assert_eq!(
            diagnostics,
            vec![ProviderDiagnosticView {
                provider_id: "codex".to_string(),
                readiness: "SetupRequired".to_string(),
                health: ProviderHealthView {
                    provider_id: "codex".to_string(),
                    status: "Unavailable".to_string(),
                    last_error: Some("missing codex".to_string()),
                },
                issues: vec!["provider executable is not ready: missing codex".to_string()],
                suggested_actions: vec!["install `codex` or add it to PATH".to_string()],
            }]
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
                    provider_id: "antigravity".to_string(),
                    status: "Unavailable".to_string(),
                    last_error: Some("missing".to_string()),
                },
            ],
            ..TuiState::default()
        };

        assert!(state.provider_status().contains(">codex:ok"));
        assert!(state.provider_status().contains("antigravity:down"));
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
                    provider_id: "antigravity".to_string(),
                    status: "Healthy".to_string(),
                    last_error: None,
                },
            ],
            ..TuiState::default()
        };

        let status = state.provider_status();

        assert!(status.contains(" codex:ok*"));
        assert!(status.contains(">antigravity:ok"));
    }

    #[test]
    fn set_provider_diagnostics_updates_health_and_selects_ready_provider() {
        let mut state = TuiState::default();

        let switched = state.set_provider_diagnostics(vec![
            ProviderDiagnosticView {
                provider_id: "codex".to_string(),
                readiness: "SetupRequired".to_string(),
                health: ProviderHealthView {
                    provider_id: "codex".to_string(),
                    status: "Unavailable".to_string(),
                    last_error: Some("missing".to_string()),
                },
                issues: vec!["missing".to_string()],
                suggested_actions: Vec::new(),
            },
            ProviderDiagnosticView {
                provider_id: "antigravity".to_string(),
                readiness: "Ready".to_string(),
                health: ProviderHealthView {
                    provider_id: "antigravity".to_string(),
                    status: "Healthy".to_string(),
                    last_error: None,
                },
                issues: Vec::new(),
                suggested_actions: Vec::new(),
            },
        ]);

        assert_eq!(
            switched,
            Some(("codex".to_string(), "antigravity".to_string()))
        );
        assert_eq!(state.selected_provider(), "antigravity");
        assert_eq!(state.provider_health[0].provider_id, "codex");
        assert_eq!(state.provider_diagnostics[1].readiness, "Ready");
    }

    #[test]
    fn set_provider_diagnostics_skips_unsupported_runtime_provider() {
        let mut state = TuiState::default();

        let switched = state.set_provider_diagnostics(vec![
            ProviderDiagnosticView {
                provider_id: "codex".to_string(),
                readiness: "SetupRequired".to_string(),
                health: ProviderHealthView {
                    provider_id: "codex".to_string(),
                    status: "Unavailable".to_string(),
                    last_error: Some("missing".to_string()),
                },
                issues: Vec::new(),
                suggested_actions: Vec::new(),
            },
            ProviderDiagnosticView {
                provider_id: "copilot".to_string(),
                readiness: "UnsupportedRuntime".to_string(),
                health: ProviderHealthView {
                    provider_id: "copilot".to_string(),
                    status: "Healthy".to_string(),
                    last_error: None,
                },
                issues: Vec::new(),
                suggested_actions: Vec::new(),
            },
            ProviderDiagnosticView {
                provider_id: "antigravity".to_string(),
                readiness: "Ready".to_string(),
                health: ProviderHealthView {
                    provider_id: "antigravity".to_string(),
                    status: "Healthy".to_string(),
                    last_error: None,
                },
                issues: Vec::new(),
                suggested_actions: Vec::new(),
            },
        ]);

        assert_eq!(
            switched,
            Some(("codex".to_string(), "antigravity".to_string()))
        );
        assert_eq!(state.selected_provider(), "antigravity");
    }

    #[test]
    fn append_provider_health_errors_shows_down_provider_setup_details() {
        let mut state = TuiState {
            detail_mode: true,
            provider_health: vec![
                ProviderHealthView {
                    provider_id: "codex".to_string(),
                    status: "Unavailable".to_string(),
                    last_error: Some("failed to spawn codex".to_string()),
                },
                ProviderHealthView {
                    provider_id: "gemini".to_string(),
                    status: "Healthy".to_string(),
                    last_error: None,
                },
            ],
            ..TuiState::default()
        };

        append_provider_health_errors(&mut state);

        let transcript = state.transcript_text();
        assert!(transcript.contains("provider setup issues:"));
        assert!(transcript.contains("codex: failed to spawn codex"));
    }

    #[test]
    fn append_provider_diagnostic_issues_shows_suggestions() {
        let mut state = TuiState {
            detail_mode: true,
            provider_diagnostics: vec![ProviderDiagnosticView {
                provider_id: "copilot".to_string(),
                readiness: "UnsupportedRuntime".to_string(),
                health: ProviderHealthView {
                    provider_id: "copilot".to_string(),
                    status: "Healthy".to_string(),
                    last_error: None,
                },
                issues: vec![
                    "Baize prompt runtime is not implemented for this provider yet".to_string(),
                ],
                suggested_actions: vec![
                    "use the ACP proof-of-life data to continue adapter implementation".to_string(),
                ],
            }],
            ..TuiState::default()
        };

        append_provider_diagnostic_issues(&mut state);

        let transcript = state.transcript_text();
        assert!(transcript.contains("provider diagnostics:"));
        assert!(transcript.contains("copilot: Baize prompt runtime"));
        assert!(transcript.contains("next: use the ACP proof-of-life"));
    }

    #[test]
    fn compact_provider_diagnostics_show_summary_only() {
        let mut state = TuiState {
            provider_diagnostics: vec![ProviderDiagnosticView {
                provider_id: "copilot".to_string(),
                readiness: "UnsupportedRuntime".to_string(),
                health: ProviderHealthView {
                    provider_id: "copilot".to_string(),
                    status: "Healthy".to_string(),
                    last_error: None,
                },
                issues: vec![
                    "Baize prompt runtime is not implemented for this provider yet".to_string(),
                ],
                suggested_actions: vec![
                    "use the ACP proof-of-life data to continue adapter implementation".to_string(),
                ],
            }],
            ..TuiState::default()
        };

        append_provider_diagnostic_issues(&mut state);

        let transcript = state.transcript_text();
        assert!(transcript.contains("provider diagnostics: 1 issue(s): copilot unsupported"));
        assert!(transcript.contains("F8 details"));
        assert!(!transcript.contains("Baize prompt runtime is not implemented"));
        assert!(!transcript.contains("next:"));
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
    fn push_message_preserves_scrollback_anchor() {
        let mut state = TuiState {
            scroll_offset: 50,
            ..TuiState::default()
        };
        state.push_message("new message");
        assert_eq!(state.scroll_offset, 51);
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
    fn scroll_offset_is_clamped_by_render_not_state() {
        let mut state = TuiState::default();
        state.push_message("only one line");
        state.scroll_up(100);
        assert_eq!(state.scroll_offset, 100);
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
        assert_eq!(state.scroll_offset, usize::MAX);
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

        assert!(rendered.contains("Conversation ("));
    }

    #[test]
    fn render_transcript_defaults_to_latest_lines() {
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut state = TuiState::default();
        for i in 0..40 {
            state.push_message(format!("older line {i}"));
        }
        state.push_message("latest assistant line");

        terminal.draw(|frame| render(frame, &state)).expect("draw");
        let buffer = terminal.backend().buffer();
        let rendered = format!("{buffer:?}");

        assert!(rendered.contains("latest assistant line"));
        assert!(!rendered.contains("older line 0"));
    }

    #[test]
    fn render_transcript_bottom_uses_wrapped_visual_lines() {
        let backend = TestBackend::new(60, 14);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let long_line = format!("{} TAIL_MARKER", "wrapped-output ".repeat(80));
        let state = TuiState {
            session_id: Some("task_wrapped".to_string()),
            transcript: vec![long_line],
            ..TuiState::default()
        };

        terminal.draw(|frame| render(frame, &state)).expect("draw");
        let buffer = terminal.backend().buffer();
        let rendered = format!("{buffer:?}");

        assert!(rendered.contains("TAIL_MARKER"));
        assert!(!rendered.contains(&"wrapped-output ".repeat(10)));
    }

    #[test]
    fn render_transcript_scroll_offset_shows_history() {
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut state = TuiState::default();
        for i in 0..40 {
            state.push_message(format!("older line {i}"));
        }
        state.push_message("latest assistant line");
        state.scroll_to_top();

        terminal.draw(|frame| render(frame, &state)).expect("draw");
        let buffer = terminal.backend().buffer();
        let rendered = format!("{buffer:?}");

        assert!(rendered.contains("What are we working on?"));
        assert!(!rendered.contains("latest assistant line"));
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
