use anyhow::Result;
use baize_adapters::AgentControl;
use baize_core::EventId;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

#[derive(Clone, Default)]
pub(crate) struct ExecutionRegistry(Arc<Mutex<HashMap<EventId, AgentControl>>>);

pub(crate) struct ExecutionHandle {
    registry: ExecutionRegistry,
    id: EventId,
    pub control: AgentControl,
}

impl ExecutionRegistry {
    pub fn start(&self, id: &EventId) -> Result<ExecutionHandle> {
        let mut map = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("execution registry poisoned"))?;
        anyhow::ensure!(!map.contains_key(id), "duplicate execution attempt");
        let control = AgentControl::default();
        map.insert(id.clone(), control.clone());
        Ok(ExecutionHandle {
            registry: self.clone(),
            id: id.clone(),
            control,
        })
    }
    pub fn get(&self, id: &EventId) -> Result<Option<AgentControl>> {
        Ok(self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("execution registry poisoned"))?
            .get(id)
            .cloned())
    }
}

impl Drop for ExecutionHandle {
    fn drop(&mut self) {
        if let Ok(mut map) = self.registry.0.lock() {
            map.remove(&self.id);
        }
    }
}

/// Native terminal evidence survives a disconnected HTTP client without advancing receipts.
pub(crate) fn confirm_stop(
    state: &crate::state::AppState,
    session_id: &baize_core::TaskSessionId,
    attempt: &EventId,
    result: &baize_adapters::AgentRunResult,
) -> Result<()> {
    use baize_core::{BaizeEvent, ExecutionStop, ExecutionStopStatus, TaskSessionStatus};
    let event = crate::helpers::with_store(state, |store| {
        let Some(mut session) = store.get_task_session(session_id)? else {
            return Ok(None);
        };
        if !session
            .continuity
            .active_turn
            .as_ref()
            .is_some_and(|turn| turn.id == *attempt)
        {
            return Ok(None);
        }
        session.continuity.active_turn = None;
        if !matches!(session.status, TaskSessionStatus::Canceled) {
            session.status = TaskSessionStatus::Running;
        }
        session.updated_at = chrono::Utc::now();
        session.continuity.last_stop = Some(ExecutionStop {
            attempt_id: attempt.clone(),
            provider_id: result.provider_id.clone(),
            status: ExecutionStopStatus::Stopped,
            detail: "Native turn completion confirmed interruption; partial changes retained."
                .into(),
        });
        let mut event = BaizeEvent::new(
            "session.agent.ignored",
            serde_json::json!({"reason":"Native interruption confirmed","active_turn":null,"native_session_id":result.native_session_id,"provider_error":result.error,"last_stop":session.continuity.last_stop,"continuity":session.continuity}),
        );
        event.session_id = Some(session.id.clone());
        event.workspace_id = Some(session.workspace_id.clone());
        event.provider_id = Some(result.provider_id.clone());
        store.save_session_event(&session, &event)?;
        Ok(Some(event))
    })?;
    if let Some(event) = event {
        let _ = state.events.send(event);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn registry_controls_only_the_exact_attempt_and_expires_on_worker_exit() {
        let registry = ExecutionRegistry::default();
        let a = EventId::new();
        let b = EventId::new();
        let first = registry.start(&a).unwrap();
        let second = registry.start(&b).unwrap();
        assert!(registry.start(&a).is_err());
        registry.get(&a).unwrap().unwrap().cancel();
        assert!(first.control.is_canceled() && !second.control.is_canceled());
        drop(first);
        assert!(registry.get(&a).unwrap().is_none());
    }
}
