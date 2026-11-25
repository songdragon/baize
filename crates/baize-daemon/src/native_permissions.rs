use crate::{helpers::with_store, state::AppState};
use anyhow::{Context, Result};
use baize_core::{
    BaizeEvent, EventId, NativePermission, PermissionId, PermissionRequest, PermissionStatus,
    ProviderId, TaskSessionId, TaskSessionStatus, WorkspaceId,
};
use chrono::Utc;
use serde_json::Value;

pub(crate) fn record(
    state: &AppState,
    workspace: &WorkspaceId,
    session: &TaskSessionId,
    provider: &ProviderId,
    attempt: &EventId,
    raw: &Value,
) -> Result<()> {
    let id = raw["request_id"]
        .as_str()
        .context("native approval missing request ID")?;
    anyhow::ensure!(id.len() <= 512, "native request ID is too long");
    let parsed: Value = serde_json::from_str(id).context("invalid native request ID")?;
    anyhow::ensure!(
        parsed.is_string() || parsed.is_number(),
        "native request ID must be scalar"
    );
    let command = raw["command"]
        .as_str()
        .filter(|text| !text.trim().is_empty())
        .context("native request missing operation details")?;
    let permission = PermissionRequest {
        id: PermissionId::new(),
        workspace_id: Some(workspace.clone()),
        session_id: Some(session.clone()),
        command: command.into(),
        reason: raw["reason"]
            .as_str()
            .unwrap_or("Review this native operation")
            .into(),
        risk: baize_core::assess_permission_command(command),
        status: PermissionStatus::Pending,
        created_at: Utc::now(),
        resolved_at: None,
        native: Some(NativePermission {
            attempt_id: attempt.clone(),
            provider_id: provider.clone(),
            request_id: id.into(),
            method: raw["method"].as_str().unwrap_or("native fixture").into(),
            params: raw["params"].clone(),
        }),
    };
    let mut event = BaizeEvent::new(
        "permission.requested",
        serde_json::json!({"permission":permission}),
    );
    event.workspace_id = Some(workspace.clone());
    event.session_id = Some(session.clone());
    event.provider_id = Some(provider.clone());
    with_store(state, |store| {
        let current = store
            .get_task_session(session)?
            .context("session not found")?;
        anyhow::ensure!(
            !matches!(current.status, TaskSessionStatus::Canceled)
                && current
                    .continuity
                    .last_stop
                    .as_ref()
                    .is_none_or(|stop| stop.attempt_id != *attempt)
                && current
                    .continuity
                    .active_turn
                    .as_ref()
                    .is_some_and(|turn| turn.id == *attempt
                        && turn.provider_id == *provider
                        && !turn.uncertain),
            "native approval belongs to an inactive execution"
        );
        anyhow::ensure!(
            state.executions.get(attempt)?.is_some(),
            "native execution is disconnected"
        );
        store.save_permission_event(&permission, &event)
    })?;
    let _ = state.events.send(event);
    Ok(())
}

pub(crate) fn resolve(
    state: &AppState,
    id: &PermissionId,
    status: PermissionStatus,
) -> Result<(PermissionRequest, BaizeEvent)> {
    with_store(state, |store| {
        let mut permission = store.get_permission(id)?.context("permission not found")?;
        let native = permission
            .native
            .clone()
            .context("permission is not a native callback")?;
        anyhow::ensure!(
            matches!(permission.status, PermissionStatus::Pending),
            "native approval is already resolved or expired"
        );
        let session = store
            .get_task_session(
                permission
                    .session_id
                    .as_ref()
                    .context("native approval missing session")?,
            )?
            .context("native session not found")?;
        anyhow::ensure!(
            !matches!(session.status, TaskSessionStatus::Canceled)
                && session
                    .continuity
                    .last_stop
                    .as_ref()
                    .is_none_or(|stop| stop.attempt_id != native.attempt_id)
                && session
                    .continuity
                    .active_turn
                    .as_ref()
                    .is_some_and(|turn| turn.id == native.attempt_id
                        && turn.provider_id == native.provider_id
                        && !turn.uncertain),
            "execution is stopping or no longer active; native approval expired"
        );
        let control = state
            .executions
            .get(&native.attempt_id)?
            .context("native execution connection lost; no approval sent")?;
        permission.status = status.clone();
        permission.resolved_at = Some(Utc::now());
        let mut event = BaizeEvent::new(
            "permission.resolved",
            serde_json::json!({"permission":permission,"delivery":"single native decision queued; operation outcome is separate"}),
        );
        event.workspace_id = permission.workspace_id.clone();
        event.session_id = permission.session_id.clone();
        event.provider_id = Some(native.provider_id.clone());
        control.decide_after(
            &native.request_id,
            matches!(status, PermissionStatus::Approved),
            || store.save_permission_event(&permission, &event),
        )?;
        Ok((permission, event))
    })
}

/// Keeps requests from appearing actionable after the worker panics or finishes.
pub(crate) struct PermissionScope {
    pub state: AppState,
    pub attempt: EventId,
}
impl Drop for PermissionScope {
    fn drop(&mut self) {
        let _ = expire(&self.state, Some(&self.attempt));
    }
}

pub(crate) fn expire(state: &AppState, attempt: Option<&EventId>) -> Result<()> {
    let events = with_store(state, |store| {
        let mut events = vec![];
        for mut permission in store.list_pending_native_permissions(attempt)? {
            let Some(native) = permission.native.as_ref() else {
                continue;
            };
            if !matches!(permission.status, PermissionStatus::Pending) {
                continue;
            }
            let expired = if let Some(id) = attempt {
                native.attempt_id == *id
            } else {
                state.executions.get(&native.attempt_id)?.is_none()
            };
            if !expired {
                continue;
            }
            let provider = native.provider_id.clone();
            permission.status = PermissionStatus::Expired;
            permission.resolved_at = Some(Utc::now());
            let mut event = BaizeEvent::new(
                "permission.resolved",
                serde_json::json!({"permission":permission,"reason":"Native execution connection ended; no decision was delivered."}),
            );
            event.workspace_id = permission.workspace_id.clone();
            event.session_id = permission.session_id.clone();
            event.provider_id = Some(provider);
            store.save_permission_event(&permission, &event)?;
            events.push(event);
        }
        Ok(events)
    })?;
    for event in events {
        let _ = state.events.send(event);
    }
    Ok(())
}
