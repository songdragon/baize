use super::*;

const PAGE_SIZE: usize = 500;

pub(super) fn load_pages(path: &str, field: &str) -> Result<Value> {
    load_pages_with(path, field, get_json)
}

fn load_pages_with(
    path: &str,
    field: &str,
    mut fetch: impl FnMut(&str) -> Result<Value>,
) -> Result<Value> {
    let mut all = Vec::new();
    let mut offset = 0;
    loop {
        let separator = if path.contains('?') { '&' } else { '?' };
        let response = fetch(&format!(
            "{path}{separator}limit={PAGE_SIZE}&offset={offset}"
        ))?;
        let page = response
            .get(field)
            .and_then(Value::as_array)
            .with_context(|| format!("missing {field} page"))?;
        let count = page.len();
        all.extend(page.iter().cloned());
        if count < PAGE_SIZE {
            break;
        }
        offset += count;
    }
    Ok(json!({field:all}))
}

pub(super) fn load_workspace_sessions(workspace_id: &str) -> Result<Vec<SessionView>> {
    // The daemon filters workspace AFTER SQL pagination. Fetch unfiltered pages first.
    let response = load_pages("/sessions", "sessions")?;
    Ok(parse_session_views(&response)?
        .into_iter()
        .filter(|s| s.workspace_id == workspace_id)
        .collect())
}

pub(super) fn load_session_events(session_id: &str) -> Result<Value> {
    load_pages(&format!("/sessions/{session_id}/events"), "events")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn loads_beyond_first_page_and_retains_order() {
        let mut requests = Vec::new();
        let result = load_pages_with("/sessions/task_1/events", "events", |path| {
            requests.push(path.to_string());
            if path.ends_with("offset=0") {
                Ok(json!({"events":(0..PAGE_SIZE).map(|i|json!({"id":i})).collect::<Vec<_>>()}))
            } else {
                Ok(json!({"events":[{"id":500}]}))
            }
        })
        .unwrap();
        assert_eq!(result["events"].as_array().unwrap().len(), 501);
        assert_eq!(result["events"][500]["id"], 500);
        assert_eq!(requests.len(), 2);
        assert!(requests[1].ends_with("offset=500"));
    }
    #[test]
    fn fails_whole_load_on_partial_or_malformed_response() {
        let result = load_pages_with("/events", "events", |_| Ok(json!({"error":"unavailable"})));
        assert!(result.is_err());
        let mut calls = 0;
        let result = load_pages_with("/events", "events", |_| {
            calls += 1;
            if calls == 1 {
                Ok(json!({"events":vec![json!({});PAGE_SIZE]}))
            } else {
                Err(anyhow!("disconnected"))
            }
        });
        assert!(result.unwrap_err().to_string().contains("disconnected"));
    }
}
