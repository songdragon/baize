use super::*;

pub(super) fn ensure_daemon_running() -> Result<DaemonLaunch> {
    if daemon_healthy() {
        return Ok(DaemonLaunch {
            status: daemon_connected_message(),
            guard: None,
        });
    }

    let guard = AutoStartedDaemon {
        _child: start_daemon_process()?,
    };
    for _ in 0..DAEMON_START_ATTEMPTS {
        if daemon_healthy() {
            return Ok(DaemonLaunch {
                status: daemon_auto_started_message(),
                guard: Some(guard),
            });
        }
        sleep(Duration::from_millis(DAEMON_START_POLL_MS));
    }

    Err(anyhow!(
        "started daemon process, but health check did not pass at http://{}/health",
        daemon_address()
    ))
}

pub(super) fn daemon_healthy() -> bool {
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

pub(super) fn start_daemon_process() -> Result<Child> {
    let executable = std::env::current_exe().context("resolve current executable")?;
    Command::new(executable)
        .args(daemon_start_args())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("start baize daemon")
}

pub(super) fn daemon_start_args() -> [&'static str; 1] {
    ["daemon"]
}

pub(super) fn daemon_connected_message() -> String {
    format!("daemon: connected at {}", daemon_address())
}

pub(super) fn daemon_auto_started_message() -> String {
    format!("{} (auto-started)", daemon_connected_message())
}

pub(super) fn load_provider_ids() -> Result<Vec<String>> {
    let response = get_json("/providers")?;
    parse_provider_ids(&response)
}

pub(super) fn load_provider_health() -> Result<Vec<ProviderHealthView>> {
    let response = post_json("/providers/check", json!({}))?;
    parse_provider_health(&response)
}

pub(super) fn load_provider_diagnostics() -> Result<Vec<ProviderDiagnosticView>> {
    let response = post_json("/providers/diagnose", json!({}))?;
    parse_provider_diagnostics(&response)
}

pub(super) fn load_pending_permissions() -> Result<Vec<PermissionView>> {
    let response = load_pages("/permissions", "permissions")?;
    Ok(parse_permissions(&response)?
        .into_iter()
        .filter(|p| p.status.eq_ignore_ascii_case("pending"))
        .collect())
}

pub(super) fn load_runtime_policy() -> Result<RuntimePolicyView> {
    let response = get_json("/runtime/status")?;
    parse_runtime_policy(&response)
}

pub(super) fn default_runtime_policy() -> RuntimePolicyView {
    RuntimePolicyView {
        native_control: Value::Null,
        command_policy: "ask".to_string(),
        execution_mode: "ask_before_commands".to_string(),
        checkpoint_policy: "before_handoff".to_string(),
        sticky_window_minutes: 30,
        quota_switch_threshold_percent: 10,
        failure_threshold_count: 3,
    }
}

pub(super) fn runtime_policy_error_hint(error: &anyhow::Error) -> String {
    let detail = error.to_string();
    if detail.contains("404") || detail.contains("empty body") {
        "restart baize daemon to enable runtime status".to_string()
    } else {
        transcript_detail(&detail)
    }
}

pub(super) fn prompt_request_body(
    provider_id: &str,
    prompt: &str,
    image_paths: &[PathBuf],
) -> Value {
    json!({
        "prompt": prompt,
        "provider_id": provider_id,
        "image_paths": image_paths,
        "timeout_seconds": PROMPT_TIMEOUT_SECONDS,
    })
}

pub(super) fn parse_provider_ids(response: &Value) -> Result<Vec<String>> {
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

pub(super) fn parse_permissions(response: &Value) -> Result<Vec<PermissionView>> {
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
                reason: if let Some(native) =
                    permission.get("native").filter(|native| native.is_object())
                {
                    format!(
                        "Native {} · attempt {} · single request\n{reason}",
                        native["provider_id"].as_str().unwrap_or("unknown"),
                        native["attempt_id"].as_str().unwrap_or("unknown")
                    )
                } else {
                    reason.to_string()
                },
                status: status.to_string(),
                risk_level,
            })
        })
        .collect())
}

pub(super) fn parse_provider_health(response: &Value) -> Result<Vec<ProviderHealthView>> {
    let health = response
        .get("health")
        .and_then(Value::as_array)
        .ok_or_else(|| response_error("health", response))?;

    Ok(health
        .iter()
        .filter_map(parse_provider_health_entry)
        .collect())
}

pub(super) fn parse_runtime_policy(response: &Value) -> Result<RuntimePolicyView> {
    let runtime = response
        .get("runtime")
        .ok_or_else(|| response_error("runtime", response))?;
    let routing = runtime.get("routing").unwrap_or(&Value::Null);
    Ok(RuntimePolicyView {
        native_control: json!({"approvals":runtime["capabilities"]["native_approvals"],"entrypoints":runtime["execution_entrypoints"]}),
        command_policy: runtime
            .get("command_policy")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        execution_mode: runtime
            .get("execution_mode")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        checkpoint_policy: runtime
            .get("checkpoint_policy")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        sticky_window_minutes: value_as_u16(routing.get("sticky_window_minutes")).unwrap_or(0),
        quota_switch_threshold_percent: value_as_u8(routing.get("quota_switch_threshold_percent"))
            .unwrap_or(0),
        failure_threshold_count: value_as_u8(routing.get("failure_threshold_count")).unwrap_or(0),
    })
}

pub(super) fn value_as_u16(value: Option<&Value>) -> Option<u16> {
    value?
        .as_u64()
        .and_then(|number| u16::try_from(number).ok())
}

pub(super) fn value_as_u8(value: Option<&Value>) -> Option<u8> {
    value?.as_u64().and_then(|number| u8::try_from(number).ok())
}

pub(super) fn parse_provider_diagnostics(response: &Value) -> Result<Vec<ProviderDiagnosticView>> {
    let diagnostics = response
        .get("diagnostics")
        .and_then(Value::as_array)
        .ok_or_else(|| response_error("diagnostics", response))?;

    Ok(diagnostics
        .iter()
        .filter_map(|diagnostic| {
            let provider_id = diagnostic.get("provider_id").and_then(provider_id)?;
            let readiness = diagnostic
                .get("readiness")
                .and_then(Value::as_str)
                .unwrap_or("Unknown");
            let health = diagnostic
                .get("health")
                .and_then(parse_provider_health_entry)
                .unwrap_or_else(|| ProviderHealthView {
                    provider_id: provider_id.to_string(),
                    status: "Unknown".to_string(),
                    last_error: None,
                });
            let issues = string_array(diagnostic.get("issues"));
            let suggested_actions = string_array(diagnostic.get("suggested_actions"));
            Some(ProviderDiagnosticView {
                provider_id: provider_id.to_string(),
                readiness: readiness.to_string(),
                health,
                issues,
                suggested_actions,
            })
        })
        .collect())
}

pub(super) fn parse_provider_health_entry(entry: &Value) -> Option<ProviderHealthView> {
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
}

pub(super) fn string_array(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(ToOwned::to_owned)
        .collect()
}

pub(super) fn health_status_for<'a>(
    health: &'a [ProviderHealthView],
    provider_id: &str,
) -> Option<&'a str> {
    health
        .iter()
        .find(|entry| entry.provider_id == provider_id)
        .map(|entry| entry.status.as_str())
}

pub(super) fn selected_provider_unavailable(state: &TuiState) -> Option<&ProviderHealthView> {
    let provider_id = state.selected_provider();
    state.provider_health.iter().find(|entry| {
        entry.provider_id == provider_id
            && (provider_health_is_down(entry) || !state.provider_ready_for_prompt(provider_id))
    })
}

pub(super) fn provider_is_down(health: &[ProviderHealthView], provider_id: &str) -> bool {
    health
        .iter()
        .any(|entry| entry.provider_id == provider_id && provider_health_is_down(entry))
}

pub(super) fn provider_health_is_down(health: &ProviderHealthView) -> bool {
    short_health_status(&health.status) == "down"
}

pub(super) fn provider_unavailable_detail(
    state: &TuiState,
    provider_id: &str,
    health: &ProviderHealthView,
) -> String {
    if let Some(diagnostic) = state
        .provider_diagnostics
        .iter()
        .find(|diagnostic| diagnostic.provider_id == provider_id && diagnostic.readiness != "Ready")
    {
        let detail = diagnostic
            .issues
            .first()
            .map(String::as_str)
            .map(transcript_detail)
            .filter(|detail| !detail.is_empty())
            .unwrap_or_else(|| format!("readiness {}", diagnostic.readiness));
        return format!(": {detail}");
    }

    health
        .last_error
        .as_deref()
        .map(transcript_detail)
        .filter(|detail| !detail.is_empty())
        .map(|detail| format!(": {detail}"))
        .unwrap_or_default()
}

pub(super) fn diagnostic_from_health(health: ProviderHealthView) -> ProviderDiagnosticView {
    let readiness = if provider_health_is_down(&health) {
        "SetupRequired"
    } else {
        "Ready"
    };
    let mut issues = Vec::new();
    if let Some(error) = &health.last_error {
        if !error.trim().is_empty() {
            issues.push(format!("provider executable is not ready: {error}"));
        }
    }
    ProviderDiagnosticView {
        provider_id: health.provider_id.clone(),
        readiness: readiness.to_string(),
        health,
        issues,
        suggested_actions: Vec::new(),
    }
}

pub(super) fn append_provider_diagnostic_issues(state: &mut TuiState) {
    if state.provider_diagnostics.is_empty() {
        append_provider_health_errors(state);
        return;
    }

    if !state.detail_mode {
        append_provider_diagnostic_summary(state);
        return;
    }

    let lines = state
        .provider_diagnostics
        .iter()
        .filter(|entry| entry.readiness != "Ready")
        .flat_map(|entry| {
            entry
                .issues
                .iter()
                .take(2)
                .map(|issue| format!("{}: {}", entry.provider_id, transcript_detail(issue)))
        })
        .take(4)
        .collect::<Vec<_>>();
    if lines.is_empty() {
        return;
    }

    state.push_message("provider diagnostics:".to_string());
    for line in lines {
        state.push_message(format!("  {line}"));
    }

    let suggestions = state
        .provider_diagnostics
        .iter()
        .filter(|entry| entry.readiness != "Ready")
        .flat_map(|entry| entry.suggested_actions.iter())
        .map(|action| transcript_detail(action))
        .take(2)
        .collect::<Vec<_>>();
    for suggestion in suggestions {
        state.push_message(format!("  next: {suggestion}"));
    }
}

pub(super) fn append_provider_diagnostic_summary(state: &mut TuiState) {
    let issues = state
        .provider_diagnostics
        .iter()
        .filter(|entry| entry.readiness != "Ready")
        .map(|entry| {
            format!(
                "{} {}",
                entry.provider_id,
                short_readiness_status(&entry.readiness)
            )
        })
        .collect::<Vec<_>>();
    if issues.is_empty() {
        return;
    }

    state.push_message(format!(
        "provider diagnostics: {} issue(s): {} · F8 details",
        issues.len(),
        issues
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    ));
}

pub(super) fn append_provider_health_errors(state: &mut TuiState) {
    let lines = state
        .provider_health
        .iter()
        .filter(|entry| short_health_status(&entry.status) == "down")
        .filter_map(|entry| {
            let error = entry.last_error.as_deref()?;
            let error = transcript_detail(error);
            if error.is_empty() {
                None
            } else {
                Some(format!("{}: {error}", entry.provider_id))
            }
        })
        .take(3)
        .collect::<Vec<_>>();
    if lines.is_empty() {
        return;
    }

    if !state.detail_mode {
        let providers = lines
            .iter()
            .filter_map(|line| {
                line.split_once(':')
                    .map(|(provider, _)| provider.to_string())
            })
            .collect::<Vec<_>>();
        state.push_message(format!(
            "provider setup issues: {} down · F8 details",
            providers.join(", ")
        ));
        return;
    }

    state.push_message("provider setup issues:".to_string());
    for line in lines {
        state.push_message(format!("  {line}"));
    }
}
