//! Current-run renderer captures for the interaction review; never calls a provider.
use super::*;
use ratatui::backend::TestBackend;

fn capture(directory: &Path, name: &str, state: &TuiState) {
    capture_size(directory, name, state, 112, 36);
}

fn capture_size(directory: &Path, name: &str, state: &TuiState, width: u16, height: u16) {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("audit terminal");
    let frame = terminal
        .draw(|frame| render(frame, state))
        .expect("audit render");
    let cells: Vec<_> = frame
        .buffer
        .content
        .iter()
        .map(|cell| {
            json!({
                "symbol":cell.symbol(),"fg":format!("{:?}",cell.fg),"bg":format!("{:?}",cell.bg),
                "bold":cell.modifier.contains(Modifier::BOLD)
            })
        })
        .collect();
    std::fs::write(
        directory.join(format!("{name}.json")),
        serde_json::to_vec(&json!({"width":width,"height":height,"cells":cells}))
            .expect("serialize capture"),
    )
    .expect("write capture");
}

fn session_load(id: &str, events: Value) -> SelectedSessionLoad {
    SelectedSessionLoad {
        workspace_display: "/audit/baize".into(),
        session: json!({"id":id,"workspace_id":"ws_audit","status":"Running","active_provider_id":"antigravity","objective":id}),
        events_response: events,
        routes_response: json!({"routes":[]}),
        diff_response: json!({"diff":{"branch":"main","dirty":false,"changed_files":[]}}),
    }
}

fn editor_state() -> TuiState {
    TuiState {
        workspace: "/audit/baize".into(),
        session_id: Some("task_original".into()),
        workspace_id: Some("ws_audit".into()),
        active_provider: Some("antigravity".into()),
        selected_provider_index: 1,
        daemon_status: "daemon: offline audit fixture".into(),
        input: "Draft A · 中文与 emoji 👩‍💻".into(),
        transcript: vec![
            "> Original question".into(),
            "Original conversation must stay with task_original.".into(),
        ],
        ..TuiState::default()
    }
}

#[test]
fn capture_current_interaction_review() {
    let Ok(path) = std::env::var("BAIZE_TUI_AUDIT_DIR") else {
        return;
    };
    let directory = PathBuf::from(path);
    std::fs::create_dir_all(&directory).expect("create audit directory");
    let mut observations = json!({});
    let (mut prompt, mut task) = (None, None);
    let original = editor_state();
    capture(&directory, "01-editor", &original);

    let mut state = original.clone();
    handle_key(
        &mut state,
        KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT),
        &mut prompt,
        &mut task,
    )
    .expect("backtab");
    observations["shift_tab_focus"] = json!(format!("{:?}", state.ui.focus));
    capture(&directory, "02-shift-tab", &state);

    let mut state = original.clone();
    save_draft(&mut state);
    apply_selected_session_load(
        &mut state,
        session_load("task_broken", json!({"error":"history unavailable"})),
    );
    observations["failed_load"] =
        json!({"session":state.session_id,"input":state.input,"notice":state.ui.notice});
    capture(&directory, "03-failed-session-load", &state);

    let mut state = original.clone();
    save_draft(&mut state); // Request starts; editing remains available while loading.
    state.input.push_str(" · EDITED WHILE LOADING");
    apply_selected_session_load(&mut state, session_load("task_other", json!({"events":[]})));
    apply_selected_session_load(
        &mut state,
        session_load("task_original", json!({"events":[]})),
    );
    observations["draft_after_switching_back"] = json!(state.input);
    capture(&directory, "04-draft-after-switch", &state);

    let mut state = original.clone();
    apply_initial_load_result(
        &mut state,
        InitialLoadResult {
            providers: Ok(vec![
                "codex".into(),
                "antigravity".into(),
                "opencode".into(),
            ]),
            diagnostics: Ok(vec![]),
            permissions: Ok(vec![]),
            runtime_policy: Ok(default_runtime_policy()),
        },
    );
    observations["target_after_refresh"] = json!(state.selected_provider());
    capture(&directory, "05-provider-refresh", &state);

    let mut state = original.clone();
    run_command(&mut state, CommandId::Deny, &mut prompt, &mut task)
        .expect("deny without a permission");
    observations["deny_review_focus"] = json!(format!("{:?}", state.ui.focus));
    capture(&directory, "06-permission-review", &state);

    let mut state = original.clone();
    handle_key(
        &mut state,
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        &mut prompt,
        &mut task,
    )
    .expect("quit confirmation");
    capture(&directory, "07-quit-confirmation", &state);

    let mut state = original;
    run_command(&mut state, CommandId::Cancel, &mut prompt, &mut task)
        .expect("cancel confirmation");
    capture(&directory, "08-cancel-confirmation", &state);
    let temp = tempfile::tempdir().expect("isolated audit drafts");
    state.ui.overlay = None;
    state.session_id = None;
    let path = temp.path().join("drafts.json");
    let mut store = DraftStore::open(
        path.clone(),
        state.workspace.clone(),
        &mut TuiState::default(),
    )
    .expect("draft store");
    store.checkpoint(&mut state);
    let mut recovered = TuiState {
        workspace: state.workspace.clone(),
        daemon_status: state.daemon_status.clone(),
        ..TuiState::default()
    };
    DraftStore::open(path, recovered.workspace.clone(), &mut recovered)
        .expect("recover saved draft");
    observations["recovered_draft"] = json!(recovered.input);
    capture(&directory, "09-recovered-draft", &recovered);
    open_overlay(&mut recovered, Overlay::Quit);
    capture(&directory, "10-saved-draft-quit", &recovered);
    capture_size(&directory, "11-small-terminal-quit", &recovered, 35, 9);
    assert!(prompt.is_none() && task.is_none());
    std::fs::write(
        directory.join("observations.json"),
        serde_json::to_vec_pretty(&observations).expect("serialize observations"),
    )
    .expect("write observations");
}
