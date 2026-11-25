use crate::{EventId, ProviderId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// User-maintained requirements. Agent output is never promoted into this list.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionContinuity {
    pub requirements: BTreeMap<String, SessionRequirement>,
    pub revision: u64,
    pub checkpoints: BTreeMap<String, ProviderCheckpoint>,
    pub active_turn: Option<ExecutionAttempt>,
    #[serde(default)]
    pub imported_handoffs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRequirement {
    pub text: String,
    pub active: bool,
    pub revision: u64,
    pub source_ref: String,
}

/// A receipt for a confirmed successful native turn, never a preview or guess.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderCheckpoint {
    pub native_session_id: String,
    pub through_event_id: EventId,
    pub pending_event_ids: Vec<EventId>,
    pub requirement_revision: u64,
    pub execution_identity: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionAttempt {
    pub id: EventId,
    pub provider_id: ProviderId,
    pub uncertain: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continuity_models_construct_and_round_trip() {
        let event = EventId::new();
        let state = SessionContinuity {
            requirements: BTreeMap::from([(
                "language".into(),
                SessionRequirement {
                    text: "中文回答".into(),
                    active: true,
                    revision: 1,
                    source_ref: event.0.clone(),
                },
            )]),
            revision: 1,
            checkpoints: BTreeMap::from([(
                "codex".into(),
                ProviderCheckpoint {
                    native_session_id: "native-1".into(),
                    through_event_id: event.clone(),
                    pending_event_ids: vec![event.clone()],
                    requirement_revision: 1,
                    execution_identity: "codex/cli/workspace".into(),
                },
            )]),
            active_turn: Some(ExecutionAttempt {
                id: event,
                provider_id: ProviderId("codex".into()),
                uncertain: true,
            }),
            imported_handoffs: vec![],
        };
        assert_eq!(
            state,
            serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap()
        );
        assert!(SessionContinuity::default().active_turn.is_none());
        assert!(serde_json::from_str::<SessionContinuity>("{\"revision\":-1}").is_err());
    }
}
