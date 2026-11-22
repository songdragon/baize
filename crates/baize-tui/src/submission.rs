use super::*;

pub(super) fn begin_prompt_submission(state: &mut TuiState) -> Option<PendingPromptSubmission> {
    let prompt = state.input.trim().to_string();
    if prompt.is_empty() {
        return None;
    }
    if handle_input_command(state, &prompt) {
        state.input.clear();
        return None;
    }

    let provider_id = state.selected_provider().to_string();
    if let Some(health) = selected_provider_unavailable(state) {
        let detail = provider_unavailable_detail(state, &provider_id, health);
        state.activity_status = "blocked".to_string();
        state.push_message(format!(
            "provider {provider_id} is not ready{detail}; press Tab to switch or Ctrl-R to refresh"
        ));
        return None;
    }

    state.activity_status = format!("running {provider_id}");
    push_transcript_gap(state);
    if state.detail_mode {
        state.push_message("╭─ task ─────────────────────────────".to_string());
    }
    state.push_message(format!("> {prompt}"));
    if state.detail_mode {
        state.push_message(format!("  target: {provider_id}"));
        if !state.image_paths.is_empty() {
            state.push_message(format!("  images: {} attached", state.image_paths.len()));
        }
        state.push_message(format!(
            "  policy: {} ({})",
            state.command_policy_label(),
            state.execution_mode()
        ));
    } else {
        if !state.image_paths.is_empty() {
            state.push_message(format!("  attached {} image(s)", state.image_paths.len()));
        }
    }
    begin_output_turn(state, &provider_id, None);
    state.scroll_to_bottom();
    state.input.clear();
    let image_paths = std::mem::take(&mut state.image_paths);

    Some(PendingPromptSubmission {
        prompt,
        provider_id,
        image_paths,
        workspace_id: state.workspace_id.clone(),
        session_id: state.session_id.clone(),
    })
}
