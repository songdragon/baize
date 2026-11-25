use super::*;

pub(super) fn start_prompt_task(submission: PendingPromptSubmission) -> Result<PromptTask> {
    let (sender, receiver) = mpsc::channel();
    let stop_stream = Arc::new(AtomicBool::new(false));
    let worker_stop_stream = stop_stream.clone();
    thread::spawn(move || {
        if let Err(error) = run_prompt_submission(&submission, sender.clone(), worker_stop_stream) {
            let _ = sender.send(PromptTaskMessage::Failed(format!("{error:#}")));
        }
    });
    Ok(PromptTask {
        receiver,
        stop_stream,
    })
}

pub(super) fn run_prompt_submission(
    submission: &PendingPromptSubmission,
    sender: Sender<PromptTaskMessage>,
    stop_stream: Arc<AtomicBool>,
) -> Result<()> {
    if let Err(error) =
        get_json("/runtime/status").and_then(|runtime| require_continuity_runtime(&runtime))
    {
        let _ = sender.send(PromptTaskMessage::NotSubmitted(error.to_string()));
        return Ok(());
    }
    let (workspace_id, workspace_display) = prepare_workspace_for_submission(submission)?;
    let session = prepare_session_for_submission(submission, &workspace_id)?;
    let started = PromptTaskStarted {
        workspace_id: Some(workspace_id.clone()),
        workspace_display: workspace_display.clone(),
        session_id: session.session_id.clone(),
        active_provider: Some(submission.provider_id.clone()),
        route_reason: session.route_reason.clone(),
        created_session: session.created,
    };
    let _ = sender.send(PromptTaskMessage::Started(started));

    let stream = match start_prompt_sse_reader(
        sender.clone(),
        session.session_id.clone(),
        submission.provider_id.clone(),
        stop_stream.clone(),
    ) {
        Ok(stream) => stream,
        Err(error) => {
            let _ = sender.send(PromptTaskMessage::NotSubmitted(format!("{error:#}")));
            return Ok(());
        }
    };
    let prompt_response = match post_json_with_timeout(
        &format!("/sessions/{}/prompt", session.session_id),
        prompt_request_body(
            &submission.provider_id,
            &submission.prompt,
            &submission.image_paths,
        ),
        Duration::from_secs(PROMPT_TIMEOUT_SECONDS + PROMPT_RESPONSE_GRACE_SECONDS),
    ) {
        Ok(response) => response,
        Err(error) => {
            drop(stream);
            let _ = send_recorded_events(&sender, &session.session_id, &submission.provider_id);
            return Err(error);
        }
    };
    drop(stream);
    if prompt_response["not_submitted"] == true {
        let session_response = get_json(&format!("/sessions/{}", session.session_id))?;
        let _ = sender.send(PromptTaskMessage::Rejected {
            response: prompt_response,
            session: session_response["session"].clone(),
        });
        return Ok(());
    }
    let events_response = load_session_events(&session.session_id)?;
    let routes_response = get_json(&format!("/sessions/{}/routes", session.session_id))?;
    let diff_response = get_json(&format!("/sessions/{}/diff", session.session_id))?;

    let completion = PromptTaskCompletion {
        workspace_id: Some(workspace_id),
        workspace_display,
        session_id: session.session_id,
        active_provider: Some(submission.provider_id.clone()),
        route_reason: session.route_reason,
        prompt_response,
        events_response,
        routes_response,
        diff_response,
    };
    let _ = sender.send(PromptTaskMessage::Completed(completion));
    Ok(())
}

fn require_continuity_runtime(value: &Value) -> Result<()> {
    anyhow::ensure!(value["runtime"]["capabilities"]["session_continuity"].as_u64().unwrap_or(0)>=2,
        "Daemon requires continuity protocol 2. Restart it with the current Baize binary; no prompt was sent");
    Ok(())
}

#[cfg(test)]
pub(super) fn session_events_path(session_id: &str, provider_id: &str) -> String {
    format!("/sessions/{session_id}/events?provider_id={provider_id}")
}

#[cfg(test)]
mod compatibility_tests {
    use super::*;
    #[test]
    fn old_daemon_cannot_receive_local_commands_as_model_prompts() {
        assert!(require_continuity_runtime(
            &json!({"runtime":{"capabilities":{"session_continuity":1}}})
        )
        .is_err());
        assert!(require_continuity_runtime(&json!({"runtime":{"command_policy":"ask"}})).is_err());
        assert!(require_continuity_runtime(
            &json!({"runtime":{"capabilities":{"session_continuity":0}}})
        )
        .is_err());
        assert!(require_continuity_runtime(
            &json!({"runtime":{"capabilities":{"session_continuity":2}}})
        )
        .is_ok());
    }
}
