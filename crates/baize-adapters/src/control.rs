use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

/// A fresh handle for one execution. Dropping a client does not request cancellation.
#[derive(Clone, Default)]
pub struct AgentControl(Arc<ControlState>);

#[derive(Default)]
struct ControlState {
    canceled: AtomicBool,
    approvals: Mutex<HashMap<String, Option<bool>>>,
}

impl AgentControl {
    /// Returns true only for the first cancellation request.
    pub fn cancel(&self) -> bool {
        !self.0.canceled.swap(true, Ordering::SeqCst)
    }

    pub fn is_canceled(&self) -> bool {
        self.0.canceled.load(Ordering::SeqCst)
    }

    /// A single decision for an existing native request; no session-wide approvals.
    #[cfg(test)]
    pub(crate) fn decide(&self, request_id: &str, approve: bool) -> Result<()> {
        self.decide_after(request_id, approve, || Ok(()))
    }

    /// Persist the human decision before making it visible to the native worker.
    pub fn decide_after(
        &self,
        request_id: &str,
        approve: bool,
        persist: impl FnOnce() -> Result<()>,
    ) -> Result<()> {
        let mut pending = self
            .0
            .approvals
            .lock()
            .map_err(|_| anyhow::anyhow!("approval lock poisoned"))?;
        if self.is_canceled() {
            bail!("execution is stopping");
        }
        let value = pending
            .get_mut(request_id)
            .context("native approval request is no longer pending")?;
        if value.is_some() {
            bail!("native approval already decided");
        }
        persist()?;
        *value = Some(approve);
        Ok(())
    }

    pub fn register_approval(&self, id: &str) -> Result<()> {
        let mut pending = self
            .0
            .approvals
            .lock()
            .map_err(|_| anyhow::anyhow!("approval lock poisoned"))?;
        if self.is_canceled() || pending.contains_key(id) {
            bail!("execution stopped or duplicate native request");
        }
        pending.insert(id.into(), None);
        Ok(())
    }

    pub fn take_decision(&self, id: &str) -> Result<Option<bool>> {
        let mut pending = self
            .0
            .approvals
            .lock()
            .map_err(|_| anyhow::anyhow!("approval lock poisoned"))?;
        if let Some(Some(decision)) = pending.get(id).copied() {
            pending.remove(id);
            return Ok(Some(decision));
        }
        Ok(None)
    }

    pub(crate) fn close_approval(&self, id: &str) {
        if let Ok(mut pending) = self.0.approvals.lock() {
            pending.remove(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancellation_is_shared_idempotent_and_does_not_affect_another_execution() {
        let control = AgentControl::default();
        assert!(!control.is_canceled());
        assert!(control.clone().cancel());
        assert!(control.is_canceled() && !control.cancel());
        assert!(!AgentControl::default().is_canceled());
    }
    #[test]
    fn native_decisions_are_once_only_and_reject_unknown_expired_or_canceled_requests() {
        let control = AgentControl::default();
        assert!(control.decide("unknown", true).is_err());
        for approve in [true, false] {
            control.register_approval("native").unwrap();
            assert!(control.register_approval("native").is_err());
            control.decide("native", approve).unwrap();
            assert!(control.decide("native", approve).is_err());
            assert_eq!(control.take_decision("native").unwrap(), Some(approve));
            assert!(control.decide("native", approve).is_err());
        }
        control.register_approval("expired").unwrap();
        control.close_approval("expired");
        assert!(control.decide("expired", true).is_err());
        control.register_approval("canceled").unwrap();
        control.cancel();
        assert!(control.decide("canceled", true).is_err());
        assert!(control.register_approval("later").is_err());
    }

    #[test]
    fn storage_failure_cannot_release_a_native_approval() {
        let control = AgentControl::default();
        control.register_approval("approval").unwrap();
        assert!(control
            .decide_after("approval", true, || anyhow::bail!("storage unavailable"))
            .is_err());
        assert_eq!(control.take_decision("approval").unwrap(), None);
        control.decide_after("approval", false, || Ok(())).unwrap();
        assert_eq!(control.take_decision("approval").unwrap(), Some(false));
    }
}
