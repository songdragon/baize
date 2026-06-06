use axum::extract::State;
use axum::http::StatusCode;
use axum::response::sse::{Event, Sse};
use axum::Json;
use baize_core::{ProviderId, TaskSessionId, WorkspaceId};
use std::convert::Infallible;
use tokio_stream::StreamExt;

use crate::helpers::{internal_error, ok_json, with_store};
use crate::state::AppState;
use crate::state::EventHistoryQuery;

pub async fn events(
    State(state): State<AppState>,
) -> Sse<impl futures::Stream<Item = Result<Event, Infallible>>> {
    let connected = futures::stream::once(async {
        Ok(Event::default()
            .event("daemon.connected")
            .data(serde_json::json!({ "status": "connected" }).to_string()))
    });
    let receiver = tokio_stream::wrappers::BroadcastStream::new(state.events.subscribe())
        .filter_map(|event| match event {
            Ok(event) => Some(Ok(Event::default()
                .event(event.event_type.clone())
                .data(serde_json::to_string(&event).unwrap_or_else(|_| "{}".to_string())))),
            Err(_) => None,
        });
    let receiver = receiver.map(|result| result);
    Sse::new(connected.chain(receiver))
}

pub async fn history(
    State(state): State<AppState>,
    axum::extract::Query(query): axum::extract::Query<EventHistoryQuery>,
) -> (StatusCode, Json<serde_json::Value>) {
    let workspace_id = query
        .workspace_id
        .as_ref()
        .map(|id| WorkspaceId(id.clone()));
    let session_id = query
        .session_id
        .as_ref()
        .map(|id| TaskSessionId(id.clone()));
    let provider_id = query.provider_id.as_ref().map(|id| ProviderId(id.clone()));
    let events = match with_store(&state, |store| {
        if let Some(session_id) = &session_id {
            store.list_events_for_session(session_id, query.limit, query.offset)
        } else if let Some(workspace_id) = &workspace_id {
            store.list_events_for_workspace(workspace_id, query.limit, query.offset)
        } else if let Some(provider_id) = &provider_id {
            store.list_events_for_provider(provider_id, query.limit, query.offset)
        } else if let Some(event_type) = &query.event_type {
            store.list_events_by_type(event_type, query.limit, query.offset)
        } else {
            store.list_events(query.limit, query.offset)
        }
    }) {
        Ok(events) => events,
        Err(error) => return internal_error(error.to_string()),
    };
    let events = events
        .into_iter()
        .filter(|event| {
            query
                .event_type
                .as_ref()
                .is_none_or(|event_type| event.event_type == *event_type)
        })
        .filter(|event| {
            workspace_id.as_ref().is_none_or(|workspace_id| {
                event
                    .workspace_id
                    .as_ref()
                    .is_some_and(|event_workspace| event_workspace.0 == workspace_id.0)
            })
        })
        .filter(|event| {
            session_id.as_ref().is_none_or(|session_id| {
                event
                    .session_id
                    .as_ref()
                    .is_some_and(|event_session| event_session.0 == session_id.0)
            })
        })
        .filter(|event| {
            provider_id.as_ref().is_none_or(|provider_id| {
                event
                    .provider_id
                    .as_ref()
                    .is_some_and(|event_provider| event_provider.0 == provider_id.0)
            })
        })
        .collect::<Vec<_>>();

    ok_json(serde_json::json!({ "events": events }))
}
