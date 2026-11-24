use crate::{helpers::with_store, state::AppState};
use anyhow::{bail, Context, Result};
use baize_core::{
    BaizeEvent, EventId, ExecutionAttempt, SessionRequirement, TaskSession, TaskSessionId,
};

pub(crate) fn import_requirements(state: &AppState, session: &mut TaskSession) -> Result<()> {
    let handoffs = with_store(state, |store| store.list_handoffs_for_session(&session.id))?;
    for handoff in handoffs {
        if session.continuity.imported_handoffs.contains(&handoff.id.0) {
            continue;
        }
        for (index, text) in handoff.mechanical_facts.user_constraints.iter().enumerate() {
            if text.trim().is_empty() {
                continue;
            }
            session.continuity.revision += 1;
            session.continuity.requirements.insert(
                format!("{}-{index}", handoff.id.0),
                SessionRequirement {
                    text: text.clone(),
                    active: true,
                    revision: session.continuity.revision,
                    source_ref: format!("handoff:{}", handoff.id.0),
                },
            );
        }
        session.continuity.imported_handoffs.push(handoff.id.0);
    }
    Ok(())
}

/// Dropping an HTTP future after dispatch must leave a durable uncertainty marker.
pub(crate) struct TurnGuard {
    state: AppState,
    session_id: TaskSessionId,
    pub id: EventId,
    pub dispatched: bool,
}

impl TurnGuard {
    pub fn begin(state: &AppState, session: &mut TaskSession) -> Result<Self> {
        let id = EventId::new();
        with_store(state, |store| {
            let mut latest = store
                .get_task_session(&session.id)?
                .context("session not found")?;
            if latest.updated_at != session.updated_at {
                bail!("Session changed; refresh before submitting");
            }
            if let Some(turn) = &latest.continuity.active_turn {
                bail!("Turn {} is {}. Review workspace and /reconcile {} only after checking execution has stopped.",
                    turn.id.0, if turn.uncertain { "uncertain" } else { "still running" }, turn.id.0);
            }
            latest.continuity.active_turn = Some(ExecutionAttempt {
                id: id.clone(),
                provider_id: latest
                    .active_provider_id
                    .clone()
                    .context("missing provider")?,
                uncertain: false,
            });
            store.upsert_task_session(&latest)?;
            session.continuity.active_turn = latest.continuity.active_turn;
            Ok(())
        })?;
        Ok(Self {
            state: state.clone(),
            session_id: session.id.clone(),
            id,
            dispatched: false,
        })
    }
}

pub(crate) fn save_turn(
    state: &AppState,
    session: &TaskSession,
    event: &BaizeEvent,
    attempt_id: &EventId,
) -> Result<bool> {
    with_store(state, |store| {
        let latest = store
            .get_task_session(&session.id)?
            .context("session not found")?;
        if matches!(latest.status, baize_core::TaskSessionStatus::Canceled)
            || latest
                .continuity
                .active_turn
                .as_ref()
                .is_none_or(|turn| &turn.id != attempt_id)
        {
            return Ok(false);
        }
        store.save_session_event(session, event)?;
        Ok(true)
    })
}

impl Drop for TurnGuard {
    fn drop(&mut self) {
        let _ = with_store(&self.state, |store| {
            let Some(mut session) = store.get_task_session(&self.session_id)? else {
                return Ok(());
            };
            if let Some(attempt) = session
                .continuity
                .active_turn
                .as_mut()
                .filter(|turn| turn.id == self.id)
            {
                if self.dispatched {
                    attempt.uncertain = true;
                } else {
                    session.continuity.active_turn = None;
                }
                store.upsert_task_session(&session)?;
            }
            Ok(())
        });
    }
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub(crate) enum ContinuityCommand {
    Remember { key: String, text: String },
    Forget { key: String },
    Requirements,
    Reconcile { turn_id: String },
    ResetProvider { provider_id: String },
}

pub(crate) fn parse_command(prompt: &str) -> Option<Result<ContinuityCommand>> {
    let (command, rest) = prompt
        .trim()
        .split_once(char::is_whitespace)
        .unwrap_or((prompt.trim(), ""));
    let rest = rest.trim();
    Some(match command {
        "/remember" => rest.split_once(char::is_whitespace)
            .map(|(key, text)| ContinuityCommand::Remember { key: key.into(), text: text.trim().into() })
            .context("Usage: /remember <key> <requirement text>"),
        "/forget" if !rest.is_empty() => Ok(ContinuityCommand::Forget { key: rest.into() }),
        "/requirements" if rest.is_empty() => Ok(ContinuityCommand::Requirements),
        "/reconcile" if !rest.is_empty() => Ok(ContinuityCommand::Reconcile { turn_id: rest.into() }),
        "/reset-provider" if !rest.is_empty() => Ok(ContinuityCommand::ResetProvider { provider_id: rest.into() }),
        "/forget" | "/requirements" | "/reconcile" | "/reset-provider" => Err(anyhow::anyhow!("Usage: /forget <key>; /requirements; /reconcile <checked-turn-id>; /reset-provider <provider>")),
        _ => return None,
    })
}

pub(crate) fn apply_command(
    state: &AppState,
    id: &TaskSessionId,
    command: ContinuityCommand,
    prompt: &str,
) -> Result<serde_json::Value> {
    let mut session =
        with_store(state, |store| store.get_task_session(id))?.context("session not found")?;
    let baseline = session.clone();
    import_requirements(state, &mut session)?;
    let mut event = BaizeEvent::new(
        "session.requirement.changed",
        serde_json::json!({"prompt":prompt,"command":command}),
    );
    let message = match command {
        ContinuityCommand::Remember { key, text } => {
            if key.is_empty()
                || key.len() > 100
                || key.chars().any(char::is_whitespace)
                || text.trim().is_empty()
                || text.len() > 16_000
            {
                bail!("Requirement key must be 1..100 bytes without whitespace; text must be 1..16000 bytes");
            }
            session.continuity.revision += 1;
            session.continuity.requirements.insert(
                key.clone(),
                SessionRequirement {
                    text,
                    active: true,
                    revision: session.continuity.revision,
                    source_ref: event.id.0.clone(),
                },
            );
            format!("Saved requirement {key}. /remember with the same key replaces it; /forget {key} revokes it.")
        }
        ContinuityCommand::Forget { key } => {
            let requirement = session
                .continuity
                .requirements
                .get_mut(&key)
                .context("requirement key not found")?;
            session.continuity.revision += 1;
            requirement.active = false;
            requirement.revision = session.continuity.revision;
            requirement.source_ref = event.id.0.clone();
            format!("Revoked requirement {key}.")
        }
        ContinuityCommand::Requirements => session
            .continuity
            .requirements
            .iter()
            .map(|(key, value)| {
                format!(
                    "{key} [r{} {}] {} (source {})",
                    value.revision,
                    if value.active { "active" } else { "revoked" },
                    value.text,
                    value.source_ref
                )
            })
            .collect::<Vec<_>>()
            .join("\n"),
        ContinuityCommand::Reconcile { turn_id } => {
            let turn = session
                .continuity
                .active_turn
                .as_ref()
                .context("no uncertain turn to reconcile")?;
            if !turn.uncertain || turn.id.0 != turn_id {
                bail!("Only the exact uncertain turn can be reconciled; check execution has stopped first");
            }
            session.continuity.active_turn = None;
            event.event_type = "session.execution.reconciled".into();
            "Execution reviewed by user. No prompt was replayed; inspect changes before continuing."
                .into()
        }
        ContinuityCommand::ResetProvider { provider_id } => {
            if session
                .provider_native_session_ids
                .remove(&provider_id)
                .is_none()
            {
                bail!("No saved native session for {provider_id}");
            }
            session.continuity.checkpoints.remove(&provider_id);
            event.event_type = "session.provider.reset".into();
            format!("Cleared {provider_id} native reference. Next explicit prompt starts a new native session with Baize history; nothing was replayed.")
        }
    };
    if session.continuity.active_turn.is_some()
        && !matches!(event.event_type.as_str(), "session.execution.reconciled")
    {
        bail!("Execution is running or uncertain; reconcile it before changing requirements or native references");
    }
    event.workspace_id = Some(session.workspace_id.clone());
    event.session_id = Some(session.id.clone());
    event.payload["message"] = serde_json::json!(if message.is_empty() {
        "No saved requirements. /remember <key> <text> saves one."
    } else {
        &message
    });
    event.payload["continuity"] = serde_json::to_value(&session.continuity)?;
    session.updated_at = chrono::Utc::now();
    with_store(state, |store| {
        let latest = store.get_task_session(id)?.context("session not found")?;
        if latest.updated_at != baseline.updated_at
            || latest.continuity != baseline.continuity
            || latest.provider_native_session_ids != baseline.provider_native_session_ids
        {
            bail!("Session changed; refresh before retrying");
        }
        store.save_session_event(&session, &event)?;
        Ok(())
    })?;
    let _ = state.events.send(event.clone());
    Ok(
        serde_json::json!({"status":"completed", "turn_status":"completed", "session_status":session.status,
        "session_id":id.0,"session":session,"message":event.payload["message"],"local_command":true}),
    )
}
