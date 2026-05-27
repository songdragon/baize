use anyhow::{Context, Result};
use baize_core::BaizeEvent;
use rusqlite::{params, Connection};
use std::fs;
use std::path::{Path, PathBuf};

pub struct EventStore {
    conn: Connection,
}

impl EventStore {
    pub fn open_default() -> Result<Self> {
        let path = default_data_dir().join("baize.db");
        Self::open(path)
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("failed to open sqlite database {}", path.display()))?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            create table if not exists events (
                id text primary key,
                event_type text not null,
                timestamp text not null,
                workspace_id text,
                session_id text,
                provider_id text,
                payload text not null
            );
            create index if not exists events_timestamp_idx on events(timestamp);
            create index if not exists events_type_idx on events(event_type);
            "#,
        )?;
        Ok(())
    }

    pub fn append_event(&self, event: &BaizeEvent) -> Result<()> {
        self.conn.execute(
            r#"
            insert into events (
                id, event_type, timestamp, workspace_id, session_id, provider_id, payload
            ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
            params![
                &event.id.0,
                &event.event_type,
                event.timestamp.to_rfc3339(),
                event.workspace_id.as_ref().map(|id| id.0.as_str()),
                event.session_id.as_ref().map(|id| id.0.as_str()),
                event.provider_id.as_ref().map(|id| id.0.as_str()),
                serde_json::to_string(&event.payload)?,
            ],
        )?;
        Ok(())
    }
}

pub fn default_data_dir() -> PathBuf {
    if let Ok(path) = std::env::var("BAIZE_DATA_DIR") {
        return PathBuf::from(path);
    }

    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("baize")
}
