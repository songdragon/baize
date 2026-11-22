use super::*;

impl TuiState {
    pub(super) fn selected_provider(&self) -> &str {
        self.providers
            .get(self.selected_provider_index)
            .map(String::as_str)
            .unwrap_or("codex")
    }

    pub(super) fn cycle_provider(&mut self) {
        if self.providers.is_empty() {
            return;
        }
        self.selected_provider_index = (self.selected_provider_index + 1) % self.providers.len();
    }

    pub(super) fn set_provider_diagnostics(
        &mut self,
        diagnostics: Vec<ProviderDiagnosticView>,
    ) -> Option<(String, String)> {
        self.provider_health = diagnostics
            .iter()
            .map(|diagnostic| diagnostic.health.clone())
            .collect();
        self.provider_diagnostics = diagnostics;
        self.select_first_available_provider_if_current_is_down()
    }

    pub(super) fn select_first_available_provider_if_current_is_down(
        &mut self,
    ) -> Option<(String, String)> {
        let current = self.selected_provider().to_string();
        if self.provider_ready_for_prompt(&current) {
            return None;
        }

        let next_index = self
            .providers
            .iter()
            .position(|provider| self.provider_ready_for_prompt(provider))?;
        if next_index == self.selected_provider_index {
            return None;
        }

        self.selected_provider_index = next_index;
        Some((current, self.selected_provider().to_string()))
    }

    pub(super) fn provider_ready_for_prompt(&self, provider: &str) -> bool {
        if let Some(diagnostic) = self
            .provider_diagnostics
            .iter()
            .find(|diagnostic| diagnostic.provider_id == provider)
        {
            return diagnostic.readiness == "Ready";
        }

        !provider_is_down(&self.provider_health, provider)
    }

    pub(super) fn selected_permission(&self) -> Option<&PermissionView> {
        self.pending_permissions.get(self.selected_permission_index)
    }

    pub(super) fn select_next_permission(&mut self) {
        if self.pending_permissions.is_empty() {
            return;
        }
        self.selected_permission_index =
            (self.selected_permission_index + 1) % self.pending_permissions.len();
    }

    pub(super) fn select_previous_permission(&mut self) {
        if self.pending_permissions.is_empty() {
            return;
        }
        self.selected_permission_index = self
            .selected_permission_index
            .checked_sub(1)
            .unwrap_or(self.pending_permissions.len() - 1);
    }

    pub(super) fn set_pending_permissions(&mut self, permissions: Vec<PermissionView>) {
        self.pending_permissions = permissions;
        if self.pending_permissions.is_empty() {
            self.selected_permission_index = 0;
        } else if self.selected_permission_index >= self.pending_permissions.len() {
            self.selected_permission_index = self.pending_permissions.len() - 1;
        }
    }

    pub(super) fn set_recent_sessions(&mut self, sessions: Vec<SessionView>) {
        self.recent_sessions = sessions;
        if self.recent_sessions.is_empty() {
            self.selected_session_index = 0;
        } else {
            self.selected_session_index = self.recent_sessions.len() - 1;
        }
    }

    pub(super) fn select_recent_session(&mut self, session_id: &str) {
        if let Some(index) = self
            .recent_sessions
            .iter()
            .position(|session| session.id == session_id)
        {
            self.selected_session_index = index;
        }
    }

    pub(super) fn select_next_recent_session(&mut self) -> bool {
        if self.recent_sessions.is_empty() {
            return false;
        }
        self.selected_session_index =
            (self.selected_session_index + 1) % self.recent_sessions.len();
        true
    }

    pub(super) fn select_previous_recent_session(&mut self) -> bool {
        if self.recent_sessions.is_empty() {
            return false;
        }
        self.selected_session_index = self
            .selected_session_index
            .checked_sub(1)
            .unwrap_or(self.recent_sessions.len() - 1);
        true
    }

    #[cfg(test)]
    pub(super) fn provider_status(&self) -> String {
        let selected = self.selected_provider();
        self.providers
            .iter()
            .map(|provider| {
                let active = self.active_provider.as_deref() == Some(provider.as_str());
                let health = health_status_for(&self.provider_health, provider)
                    .map(short_health_status)
                    .unwrap_or("?");
                let selected_marker = if provider == selected { ">" } else { " " };
                let active_marker = if active { "*" } else { "" };
                format!("{selected_marker}{provider}:{health}{active_marker}")
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub(super) fn command_policy_label(&self) -> String {
        let Some(policy) = &self.runtime_policy else {
            return "unknown".to_string();
        };
        match policy.command_policy.as_str() {
            "ask" => "ask before commands".to_string(),
            "allow_project" => "project writes allowed".to_string(),
            "deny" => "read-only".to_string(),
            other => other.to_string(),
        }
    }

    pub(super) fn execution_mode(&self) -> String {
        self.runtime_policy
            .as_ref()
            .map(|policy| policy.execution_mode.replace('_', "-"))
            .unwrap_or_else(|| "unknown".to_string())
    }

    pub(super) fn routing_status(&self) -> String {
        let Some(policy) = &self.runtime_policy else {
            return "unknown".to_string();
        };
        format!(
            "{}m quota<{}% fail>{}",
            policy.sticky_window_minutes,
            policy.quota_switch_threshold_percent,
            policy.failure_threshold_count
        )
    }

    pub(super) fn push_message(&mut self, message: impl Into<String>) {
        let message = message.into();
        if self.scroll_offset > 0 {
            let added: usize = message
                .split('\n')
                .map(|line| wrap_transcript_raw_line(line, self.ui.wrap_width).len())
                .sum();
            self.scroll_offset = self.scroll_offset.saturating_add(added);
        }
        self.transcript.push(message);
    }

    pub(super) fn scroll_up(&mut self, amount: u16) {
        self.scroll_offset = self.scroll_offset.saturating_add(usize::from(amount));
    }

    pub(super) fn scroll_down(&mut self, amount: u16) {
        self.scroll_offset = self.scroll_offset.saturating_sub(usize::from(amount));
    }

    pub(super) fn scroll_to_bottom(&mut self) {
        self.scroll_offset = 0;
    }

    pub(super) fn scroll_to_top(&mut self) {
        self.scroll_offset = usize::MAX;
    }

    #[cfg(test)]
    pub(super) fn transcript_text(&self) -> String {
        self.transcript
            .iter()
            .enumerate()
            .map(|(index, line)| {
                if let Some(turn) = self
                    .ui
                    .output
                    .turns
                    .iter()
                    .find(|turn| turn.anchor == index)
                {
                    let items = turn
                        .items
                        .iter()
                        .map(|item| {
                            let prefix = match item.role {
                                OutputRole::Tool => "tool │ ",
                                OutputRole::Diagnostic => "stderr │ ",
                                OutputRole::Status => "status │ ",
                                _ => "  ",
                            };
                            safe_output_text(&item.text)
                                .split('\n')
                                .map(|line| format!("{prefix}{line}"))
                                .collect::<Vec<_>>()
                                .join("\n")
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    format!("{line}\n{items}")
                } else {
                    line.clone()
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

pub(super) fn short_health_status(status: &str) -> &str {
    match status {
        "Healthy" => "ok",
        "Degraded" => "warn",
        "Unavailable" => "down",
        "Unknown" => "?",
        _ => "?",
    }
}

pub(super) fn short_readiness_status(status: &str) -> &str {
    match status {
        "Ready" => "ready",
        "SetupRequired" => "setup",
        "UnsupportedRuntime" => "unsupported",
        "CapabilityMismatch" => "mismatch",
        _ => "?",
    }
}
