use super::*;

pub(super) fn prepare_workspace_for_submission(
    submission: &PendingPromptSubmission,
) -> Result<(String, Option<String>)> {
    if let Some(id) = &submission.workspace_id {
        return Ok((id.clone(), None));
    }

    create_workspace_from_current_dir()
}

pub(super) fn create_workspace_from_current_dir() -> Result<(String, Option<String>)> {
    let cwd = std::env::current_dir().context("read current directory")?;
    let canonical = cwd.canonicalize().context("resolve workspace")?;
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
    Ok((id, Some(canonical.display().to_string())))
}

pub(super) fn prepare_session_for_submission(
    submission: &PendingPromptSubmission,
    workspace_id: &str,
) -> Result<SubmissionSession> {
    if let Some(id) = &submission.session_id {
        return Ok(SubmissionSession {
            session_id: id.clone(),
            route_reason: None,
            created: false,
        });
    }

    let response = post_json(
        "/sessions",
        json!({
            "workspace_id": workspace_id,
            "objective": submission.prompt,
            "provider_id": submission.provider_id,
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
    let route_reason = response
        .get("route_decision")
        .and_then(|decision| decision.get("reason"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);

    Ok(SubmissionSession {
        session_id: id,
        route_reason,
        created: true,
    })
}

pub(super) fn append_route_history(state: &mut TuiState, response: &Value) {
    if !state.detail_mode {
        return;
    }

    let Some(routes) = response.get("routes").and_then(Value::as_array) else {
        return;
    };
    if routes.is_empty() {
        return;
    }

    push_transcript_section(state, "routes:");
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

pub(super) fn append_session_diff(state: &mut TuiState, response: &Value) {
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
        push_transcript_gap(state);
        state.push_message("workspace clean");
        return;
    }

    if !state.detail_mode {
        push_transcript_gap(state);
        if changed_files.is_empty() {
            state.push_message("changed files: unknown · F8 details".to_string());
        } else {
            state.push_message(format!(
                "changed files: {} · F8 details",
                changed_files.len()
            ));
        }
        return;
    }

    let shown = changed_files.iter().take(5).cloned().collect::<Vec<_>>();
    let suffix = changed_files
        .len()
        .checked_sub(shown.len())
        .filter(|remaining| *remaining > 0)
        .map(|remaining| format!(" (+{remaining} more)"))
        .unwrap_or_default();
    push_transcript_gap(state);
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
