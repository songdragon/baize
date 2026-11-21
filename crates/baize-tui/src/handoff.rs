use super::*;

pub(super) fn append_handoff_response(state: &mut TuiState, response: &Value) {
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

pub(super) fn append_permission_resolution(state: &mut TuiState, response: &Value) {
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
