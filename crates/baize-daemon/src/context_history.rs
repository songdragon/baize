use baize_core::{BaizeEvent, ContextSource};
use std::collections::HashMap;

pub(crate) struct Message {
    pub source: ContextSource,
    pub text: String,
}

/// Reconstruct stream items before selecting context; chunks are not messages.
pub(crate) fn messages(events: &[BaizeEvent]) -> Vec<Message> {
    let mut messages: Vec<Message> = vec![];
    let mut items = HashMap::<(String, String), usize>::new();
    for event in events {
        let role = match event.event_type.as_str() {
            "session.agent.started" => {
                items.clear();
                "user"
            }
            "session.agent.output" => "assistant",
            _ => continue,
        };
        let text = if role == "user" {
            event.payload["prompt"].as_str()
        } else {
            event.payload["text"].as_str()
        };
        let Some(text) = text else {
            continue;
        };
        let stream = &event.payload["stream"];
        let key = (
            event
                .provider_id
                .as_ref()
                .map(|id| id.0.clone())
                .unwrap_or_default(),
            stream["item_id"]
                .as_str()
                .unwrap_or(&event.id.0)
                .to_string(),
        );
        if role == "assistant" {
            if let Some(index) = items.get(&key) {
                if stream["mode"] == "delta" {
                    messages[*index].text.push_str(text);
                } else {
                    messages[*index].text = text.into();
                }
                continue;
            }
            items.insert(key, messages.len());
        }
        messages.push(Message {
            text: text.into(),
            source: ContextSource {
                event_id: event.id.clone(),
                role: role.into(),
                provider_id: event.provider_id.clone(),
                included: false,
                truncated: false,
            },
        });
    }
    messages
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reconstructs_deltas_and_preserves_role_source_without_promoting_suggestions() {
        let mut events = vec![BaizeEvent::new(
            "session.agent.started",
            json!({"prompt":"user correction"}),
        )];
        for (mode, text) in [
            ("delta", "agent "),
            ("delta", "suggestion"),
            ("snapshot", "final suggestion"),
        ] {
            events.push(BaizeEvent::new(
                "session.agent.output",
                json!({"text":text,"stream":{"mode":mode,"item_id":"answer"}}),
            ));
        }
        let result = messages(&events);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].source.role, "user");
        assert_eq!(result[0].source.event_id, events[0].id);
        assert_eq!(result[1].text, "final suggestion");
        assert_eq!(result[1].source.role, "assistant");
        assert!(messages(&[BaizeEvent::new("session.agent.output", json!({}))]).is_empty());
    }
}
