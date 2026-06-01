use axum::extract::State;
use axum::response::sse::{Event, Sse};
use std::convert::Infallible;
use tokio_stream::StreamExt;

use crate::state::AppState;

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
