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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_stop: Option<ExecutionStop>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionStop {
    pub attempt_id: EventId,
    pub provider_id: ProviderId,
    pub status: ExecutionStopStatus,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStopStatus {
    Stopping,
    Stopped,
    Unconfirmed,
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
            last_stop: None,
        };
        assert_eq!(
            state,
            serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap()
        );
        assert!(SessionContinuity::default().active_turn.is_none());
        assert!(serde_json::from_str::<SessionContinuity>("{\"revision\":-1}").is_err());
    }

    #[test]
    fn execution_stop_round_trip_and_legacy_default() {
        for status in [
            ExecutionStopStatus::Stopping,
            ExecutionStopStatus::Stopped,
            ExecutionStopStatus::Unconfirmed,
        ] {
            let stop = ExecutionStop {
                attempt_id: EventId::new(),
                provider_id: ProviderId("codex".into()),
                status,
                detail: "native stop evidence".into(),
            };
            assert_eq!(
                serde_json::from_value::<ExecutionStop>(serde_json::to_value(&stop).unwrap())
                    .unwrap(),
                stop
            );
        }
        assert!(
            serde_json::from_value::<ExecutionStop>(serde_json::json!({"status":"guess"})).is_err()
        );
        let old =
            serde_json::json!({"requirements":{},"revision":0,"checkpoints":{},"active_turn":null});
        assert!(serde_json::from_value::<SessionContinuity>(old)
            .unwrap()
            .last_stop
            .is_none());
    }

    #[test]
    fn native_permission_binding_round_trip_requires_exact_attempt_provider_and_request() {
        let binding = crate::NativePermission {
            attempt_id: EventId::new(),
            provider_id: ProviderId("codex".into()),
            request_id: "101".into(),
            method: "item/commandExecution/requestApproval".into(),
            params: serde_json::json!({"command":"one operation","cwd":"/fixture"}),
        };
        assert_eq!(
            serde_json::from_value::<crate::NativePermission>(
                serde_json::to_value(&binding).unwrap()
            )
            .unwrap(),
            binding
        );
        assert!(serde_json::from_value::<crate::NativePermission>(
            serde_json::json!({"request_id":101})
        )
        .is_err());
        assert!(matches!(
            serde_json::from_value::<crate::PermissionStatus>(serde_json::json!("Expired"))
                .unwrap(),
            crate::PermissionStatus::Expired
        ));
    }
}
