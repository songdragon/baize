use super::*;
const FIXTURE: &str = r#"
mode="$1"
while IFS= read -r line; do
 printf '%s\n' "$line" >> protocol.log
 case "$line" in
 *'"method":"initialize"'*) printf '%s\n' '{"id":"baize-init","result":{}}' ;;
 *'"method":"thread/start"'*|*'"method":"thread/resume"'*) printf '%s\n' '{"id":"baize-thread","result":{"thread":{"id":"native-fixture"}}}' ;;
 *'"method":"turn/start"'*)
  if [ "$mode" = earlycancel ]; then sleep 0.2; fi
  if [ "$mode" = lost ]; then exit 0; fi
  if [ "$mode" = malformed ]; then printf 'not-json\n'; exit 0; fi
  printf '%s\n' '{"id":"baize-turn","result":{"turn":{"id":"native-turn"}}}' '{"method":"turn/started","params":{"threadId":"native-fixture","turn":{"id":"native-turn","status":"inProgress","items":[]}}}'
  case "$mode" in
   approval|cancel|stale|silentcancel)
    scope=native-fixture; if [ "$mode" = stale ]; then scope=other-thread; fi
    printf '%s\n' "{\"id\":101,\"method\":\"item/commandExecution/requestApproval\",\"params\":{\"threadId\":\"$scope\",\"turnId\":\"native-turn\",\"itemId\":\"cmd-1\",\"command\":\"printf approved > approved\",\"cwd\":\"fixture\",\"reason\":\"single operation\"}}"
    ;;
   file|broadfile)
    if [ "$mode" = broadfile ]; then printf '%s\n' '{"id":101,"method":"item/fileChange/requestApproval","params":{"threadId":"native-fixture","turnId":"native-turn","itemId":"patch-1","grantRoot":"/outside-fixture"}}'; continue; fi
    printf '%s\n' '{"method":"item/started","params":{"threadId":"native-fixture","turnId":"native-turn","item":{"id":"patch-1","type":"fileChange","changes":[{"path":"file.txt","diff":"+ one change"}]}}}' '{"id":101,"method":"item/fileChange/requestApproval","params":{"threadId":"native-fixture","turnId":"native-turn","itemId":"patch-1","reason":"review file change"}}'
    ;;
   normal)
    printf '%s\n' '{"method":"item/agentMessage/delta","params":{"threadId":"native-fixture","turnId":"native-turn","itemId":"answer","delta":"hello "}}' '{"method":"item/completed","params":{"threadId":"native-fixture","turnId":"native-turn","item":{"id":"answer","type":"agentMessage","text":"hello world","phase":"final_answer"}}}' '{"method":"turn/completed","params":{"threadId":"native-fixture","turn":{"id":"native-turn","status":"completed","items":[]}}}'
    ;;
  esac
  ;;
 *'"method":"turn/interrupt"'*)
  if [ "$mode" = silentcancel ]; then printf '%s\n' '{"id":"baize-interrupt","result":{}}'; continue; fi
  printf '%s\n' '{"id":"baize-interrupt","result":{}}' '{"method":"turn/completed","params":{"threadId":"native-fixture","turn":{"id":"native-turn","status":"interrupted","items":[]}}}'
  ;;
 *'"id":101'*)
  case "$line" in
   *'"decision":"accept"'*) printf approved > approved ;;
   *'"decision":"cancel"'*) continue ;;
  esac
  printf '%s\n' '{"method":"turn/completed","params":{"threadId":"native-fixture","turn":{"id":"native-turn","status":"completed","items":[]}}}'
  ;;
 esac
done
"#;

fn request(dir: &std::path::Path) -> AgentPromptRequest {
    AgentPromptRequest {
        provider_id: ProviderId("codex".into()),
        prompt: "fixture request".into(),
        cwd: dir.into(),
        session_id: None,
        image_paths: vec![],
        timeout_seconds: Some(2),
        execution_policy: AgentExecutionPolicy::Ask,
    }
}
fn fixture(dir: &std::path::Path, mode: &str) -> Vec<String> {
    let script = dir.join("native.sh");
    std::fs::write(&script, FIXTURE).unwrap();
    vec![script.to_string_lossy().into_owned(), mode.into()]
}
#[test]
fn native_approval_is_scoped_once_only_and_preserves_sandbox_policy() {
    for mode in ["approval", "file"] {
        for approve in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let control = AgentControl::default();
            let mut approvals = 0;
            let mut sink = |event: AgentExecutionEvent| {
                if let Some(raw) = event.raw {
                    if let Some(approval) = raw.get("native_approval") {
                        approvals += 1;
                        assert!(approval["command"].as_str().unwrap().contains(
                            if mode == "file" {
                                "file.txt"
                            } else {
                                "printf approved"
                            }
                        ));
                        control
                            .decide(approval["request_id"].as_str().unwrap(), approve)
                            .unwrap();
                    }
                }
            };
            let result = run_with_command(
                request(dir.path()),
                Some(&mut sink),
                &control,
                "sh",
                &fixture(dir.path(), mode),
            )
            .unwrap();
            assert!(result.success);
            assert_eq!(approvals, 1);
            assert_eq!(dir.path().join("approved").exists(), approve);
            assert!(control.decide("101", true).is_err());
            let protocol = std::fs::read_to_string(dir.path().join("protocol.log")).unwrap();
            assert!(protocol.contains("\"approvalPolicy\":\"untrusted\""));
            assert!(protocol.contains("\"sandbox\":\"workspace-write\""));
            assert!(!protocol.contains("acceptForSession"));
        }
    }
}
#[test]
fn interrupt_waits_for_native_terminal_evidence_and_does_not_approve_the_waiting_tool() {
    let dir = tempfile::tempdir().unwrap();
    let control = AgentControl::default();
    let mut sink = |event: AgentExecutionEvent| {
        if event
            .raw
            .as_ref()
            .is_some_and(|raw| raw.get("native_approval").is_some())
        {
            control.cancel();
        }
    };
    let result = run_with_command(
        request(dir.path()),
        Some(&mut sink),
        &control,
        "sh",
        &fixture(dir.path(), "cancel"),
    )
    .unwrap();
    assert!(!result.success);
    assert_eq!(result.error.unwrap().kind, AgentErrorKind::Canceled);
    assert!(!dir.path().join("approved").exists());
    assert!(std::fs::read_to_string(dir.path().join("protocol.log"))
        .unwrap()
        .contains("turn/interrupt"));
}
#[test]
fn native_resume_streams_deltas_and_final_snapshots_without_replacing_user_instructions() {
    let dir = tempfile::tempdir().unwrap();
    let mut req = request(dir.path());
    req.session_id = Some("native-fixture".into());
    let result = run_with_command(
        req,
        None,
        &AgentControl::default(),
        "sh",
        &fixture(dir.path(), "normal"),
    )
    .unwrap();
    assert!(result.success);
    assert_eq!(result.native_session_id.as_deref(), Some("native-fixture"));
    assert_eq!(
        result
            .events
            .iter()
            .filter(|event| matches!(event.kind, AgentExecutionEventKind::Output))
            .count(),
        2
    );
    let protocol = std::fs::read_to_string(dir.path().join("protocol.log")).unwrap();
    assert!(protocol.contains("thread/resume") && protocol.contains("fixture request"));
    assert!(!protocol.contains("baseInstructions") && !protocol.contains("developerInstructions"));
}
#[test]
fn stale_callbacks_and_protocol_failure_cannot_be_approved_or_reported_as_success() {
    for mode in ["stale", "lost", "malformed"] {
        let dir = tempfile::tempdir().unwrap();
        let control = AgentControl::default();
        let mut approvals = 0;
        let mut sink = |event: AgentExecutionEvent| {
            if event
                .raw
                .as_ref()
                .is_some_and(|raw| raw.get("native_approval").is_some())
            {
                approvals += 1;
            }
        };
        let result = run_with_command(
            request(dir.path()),
            Some(&mut sink),
            &control,
            "sh",
            &fixture(dir.path(), mode),
        );
        if mode == "stale" {
            assert!(result.unwrap().success);
        } else {
            assert!(result.is_err());
        }
        assert_eq!(approvals, 0);
        assert!(control.decide("101", true).is_err());
        assert!(!dir.path().join("approved").exists());
    }
}
#[test]
fn cancellation_before_launch_cannot_send_a_native_turn() {
    let dir = tempfile::tempdir().unwrap();
    let control = AgentControl::default();
    control.cancel();
    let result = run_with_command(
        request(dir.path()),
        None,
        &control,
        "/missing-provider",
        &[],
    )
    .unwrap();
    assert_eq!(result.error.unwrap().kind, AgentErrorKind::Canceled);
    assert!(!dir.path().join("protocol.log").exists());
}

#[test]
fn cancel_after_submission_waits_for_a_turn_id_and_native_interruption() {
    let dir = tempfile::tempdir().unwrap();
    let control = AgentControl::default();
    let worker = control.clone();
    let canceller = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(80));
        worker.cancel();
    });
    let result = run_with_command(
        request(dir.path()),
        None,
        &control,
        "sh",
        &fixture(dir.path(), "earlycancel"),
    )
    .unwrap();
    canceller.join().unwrap();
    assert_eq!(result.error.unwrap().kind, AgentErrorKind::Canceled);
    assert!(std::fs::read_to_string(dir.path().join("protocol.log"))
        .unwrap()
        .contains("turn/interrupt"));
}

#[test]
fn a_file_root_grant_cannot_be_presented_as_a_single_operation_approval() {
    let dir = tempfile::tempdir().unwrap();
    let control = AgentControl::default();
    let mut approvals = 0;
    let mut sink = |event: AgentExecutionEvent| {
        if let Some(raw) = event.raw {
            if let Some(approval) = raw.get("native_approval") {
                approvals += 1;
                control
                    .decide(approval["request_id"].as_str().unwrap(), true)
                    .unwrap();
            }
        }
    };
    let result = run_with_command(
        request(dir.path()),
        Some(&mut sink),
        &control,
        "sh",
        &fixture(dir.path(), "broadfile"),
    )
    .unwrap();
    assert!(result.success);
    assert_eq!(
        approvals, 0,
        "a grantRoot request must not be relabeled single-use"
    );
    assert!(!dir.path().join("approved").exists());
}

#[test]
fn stalled_native_input_is_bounded_instead_of_blocking_cancellation_forever() {
    let dir = tempfile::tempdir().unwrap();
    let started = Instant::now();
    let mut rpc = crate::codex_transport::RpcProcess::spawn(
        "sh",
        &["-c".into(), "sleep 30".into()],
        &dir.path().to_path_buf(),
    )
    .unwrap();
    assert!(rpc
        .send(serde_json::json!({"prompt":"x".repeat(200_000)}))
        .is_err());
    drop(rpc);
    assert!(started.elapsed() < Duration::from_secs(4));
}

#[test]
fn an_interrupt_ack_without_native_completion_is_not_stop_confirmation() {
    let dir = tempfile::tempdir().unwrap();
    let control = AgentControl::default();
    let mut sink = |event: AgentExecutionEvent| {
        if event
            .raw
            .as_ref()
            .is_some_and(|raw| raw.get("native_approval").is_some())
        {
            control.cancel();
        }
    };
    let started = Instant::now();
    let outcome = run_with_command(
        request(dir.path()),
        Some(&mut sink),
        &control,
        "sh",
        &fixture(dir.path(), "silentcancel"),
    );
    assert!(outcome.is_err());
    assert!(outcome
        .unwrap_err()
        .to_string()
        .contains("no terminal evidence"));
    assert!(started.elapsed() < Duration::from_secs(7));
    assert!(control.decide("101", true).is_err());
}
