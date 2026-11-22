use super::*;

pub(super) fn apply_initial_load_result(state: &mut TuiState, result: InitialLoadResult) {
    if result.providers.is_ok() {
        state.daemon_status = daemon_connected_message();
    }
    let selected_provider = state.selected_provider().to_string();
    match result.providers {
        Ok(providers) if !providers.is_empty() => {
            state.selected_provider_index = providers
                .iter()
                .position(|provider| provider == &selected_provider)
                .unwrap_or(0);
            state.providers = providers;
        }
        Ok(_) => state.push_message("daemon returned no enabled providers; using defaults"),
        Err(error) => state.push_message(format!("provider load failed: {error}")),
    }
    match result.diagnostics {
        Ok(diagnostics) => {
            if let Some((from, to)) = state.set_provider_diagnostics(diagnostics) {
                state.push_message(format!("provider auto-selected: {from} -> {to}"));
            }
            if state.detail_mode {
                append_provider_diagnostic_issues(state);
            }
        }
        Err(error) => state.push_message(format!("provider diagnostics failed: {error}")),
    }
    match result.permissions {
        Ok(permissions) => state.set_pending_permissions(permissions),
        Err(error) => state.push_message(format!("permission load failed: {error}")),
    }
    match result.runtime_policy {
        Ok(policy) => state.runtime_policy = Some(policy),
        Err(error) => {
            state.runtime_policy = Some(default_runtime_policy());
            state.push_message(format!(
                "runtime policy unavailable; using safe default ask mode ({})",
                runtime_policy_error_hint(&anyhow!(error.clone()))
            ));
        }
    }
    if state.detail_mode {
        state.push_message("status load complete");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn result(providers: Result<Vec<String>, String>) -> InitialLoadResult {
        InitialLoadResult {
            providers,
            diagnostics: Ok(vec![]),
            permissions: Ok(vec![]),
            runtime_policy: Ok(default_runtime_policy()),
        }
    }
    #[test]
    fn refresh_preserves_the_latest_selected_provider_by_id_across_reordering() {
        let mut state = TuiState {
            selected_provider_index: 1,
            ..TuiState::default()
        };
        apply_initial_load_result(
            &mut state,
            result(Ok(vec![
                "opencode".into(),
                "codex".into(),
                "antigravity".into(),
            ])),
        );
        assert_eq!(state.selected_provider(), "antigravity");
        assert_eq!(state.selected_provider_index, 2);
        apply_initial_load_result(&mut state, result(Err("offline".into())));
        assert_eq!(state.selected_provider(), "antigravity");
        apply_initial_load_result(&mut state, result(Ok(vec!["opencode".into()])));
        assert_eq!(state.selected_provider(), "opencode");
    }
    #[test]
    fn refresh_still_falls_back_when_the_chosen_provider_is_unavailable() {
        let mut state = TuiState {
            selected_provider_index: 1,
            ..TuiState::default()
        };
        let mut refresh = result(Ok(vec!["codex".into(), "antigravity".into()]));
        refresh.diagnostics = Ok(vec![ProviderDiagnosticView {
            provider_id: "antigravity".into(),
            readiness: "SetupRequired".into(),
            health: ProviderHealthView {
                provider_id: "antigravity".into(),
                status: "Unavailable".into(),
                last_error: Some("login".into()),
            },
            issues: vec![],
            suggested_actions: vec![],
        }]);
        apply_initial_load_result(&mut state, refresh);
        assert_eq!(state.selected_provider(), "codex");
    }
}
