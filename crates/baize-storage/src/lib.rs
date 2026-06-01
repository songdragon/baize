use anyhow::{Context, Result};
use baize_core::{
    BaizeEvent, HandoffId, HandoffSummary, PermissionId, PermissionRequest, Project, ProjectId,
    RouteDecision, RouteDecisionId, TaskSession, TaskSessionId, Workspace, WorkspaceId,
};
use rusqlite::{params, Connection};
use serde::de::DeserializeOwned;
use serde::Serialize;
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

            create table if not exists workspaces (
                id text primary key,
                json text not null
            );
            create table if not exists projects (
                id text primary key,
                workspace_id text not null,
                json text not null
            );
            create table if not exists task_sessions (
                id text primary key,
                workspace_id text not null,
                json text not null
            );
            create table if not exists route_decisions (
                id text primary key,
                session_id text not null,
                json text not null
            );
            create table if not exists handoffs (
                id text primary key,
                session_id text not null,
                json text not null
            );
            create table if not exists permissions (
                id text primary key,
                session_id text,
                json text not null
            );
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

    pub fn event_count(&self) -> Result<u64> {
        let count = self
            .conn
            .query_row("select count(*) from events", [], |row| {
                row.get::<_, u64>(0)
            })?;
        Ok(count)
    }

    pub fn list_events_for_session(&self, session_id: &TaskSessionId) -> Result<Vec<BaizeEvent>> {
        let mut stmt = self
            .conn
            .prepare("select id, event_type, timestamp, workspace_id, session_id, provider_id, payload from events where session_id = ?1 order by timestamp")?;
        let rows = stmt.query_map(params![&session_id.0], |row| {
            let timestamp: String = row.get(2)?;
            let payload: String = row.get(6)?;
            Ok(BaizeEvent {
                id: baize_core::EventId(row.get(0)?),
                event_type: row.get(1)?,
                timestamp: chrono::DateTime::parse_from_rfc3339(&timestamp)
                    .map(|ts| ts.with_timezone(&chrono::Utc))
                    .map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            2,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })?,
                workspace_id: row.get::<_, Option<String>>(3)?.map(WorkspaceId),
                session_id: row.get::<_, Option<String>>(4)?.map(TaskSessionId),
                provider_id: row.get::<_, Option<String>>(5)?.map(baize_core::ProviderId),
                payload: serde_json::from_str(&payload).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        6,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?,
            })
        })?;
        let mut events = Vec::new();
        for row in rows {
            events.push(row?);
        }
        Ok(events)
    }

    pub fn upsert_workspace(&self, workspace: &Workspace) -> Result<()> {
        self.upsert_json("workspaces", &workspace.id.0, workspace)
    }

    pub fn list_workspaces(&self) -> Result<Vec<Workspace>> {
        self.list_json("workspaces")
    }

    pub fn get_workspace(&self, id: &WorkspaceId) -> Result<Option<Workspace>> {
        self.get_json("workspaces", &id.0)
    }

    pub fn upsert_project(&self, project: &Project) -> Result<()> {
        self.conn.execute(
            r#"
            insert into projects (id, workspace_id, json)
            values (?1, ?2, ?3)
            on conflict(id) do update set workspace_id = excluded.workspace_id, json = excluded.json
            "#,
            params![
                &project.id.0,
                &project.workspace_id.0,
                serde_json::to_string(project)?,
            ],
        )?;
        Ok(())
    }

    pub fn get_project(&self, id: &ProjectId) -> Result<Option<Project>> {
        self.get_json("projects", &id.0)
    }

    pub fn get_primary_project(&self, workspace: &Workspace) -> Result<Option<Project>> {
        self.get_project(&workspace.primary_project_id)
    }

    pub fn upsert_task_session(&self, session: &TaskSession) -> Result<()> {
        self.conn.execute(
            r#"
            insert into task_sessions (id, workspace_id, json)
            values (?1, ?2, ?3)
            on conflict(id) do update set workspace_id = excluded.workspace_id, json = excluded.json
            "#,
            params![
                &session.id.0,
                &session.workspace_id.0,
                serde_json::to_string(session)?,
            ],
        )?;
        Ok(())
    }

    pub fn list_task_sessions(&self) -> Result<Vec<TaskSession>> {
        self.list_json("task_sessions")
    }

    pub fn get_task_session(&self, id: &TaskSessionId) -> Result<Option<TaskSession>> {
        self.get_json("task_sessions", &id.0)
    }

    pub fn insert_route_decision(&self, decision: &RouteDecision) -> Result<()> {
        self.conn.execute(
            "insert into route_decisions (id, session_id, json) values (?1, ?2, ?3)",
            params![
                &decision.id.0,
                &decision.session_id.0,
                serde_json::to_string(decision)?,
            ],
        )?;
        Ok(())
    }

    pub fn get_route_decision(&self, id: &RouteDecisionId) -> Result<Option<RouteDecision>> {
        self.get_json("route_decisions", &id.0)
    }

    pub fn list_route_decisions_for_session(
        &self,
        session_id: &TaskSessionId,
    ) -> Result<Vec<RouteDecision>> {
        let mut stmt = self
            .conn
            .prepare("select json from route_decisions where session_id = ?1 order by rowid")?;
        let rows = stmt.query_map(params![&session_id.0], |row| row.get::<_, String>(0))?;
        let mut decisions = Vec::new();
        for row in rows {
            decisions.push(serde_json::from_str(&row?)?);
        }
        Ok(decisions)
    }

    pub fn insert_handoff(&self, handoff: &HandoffSummary) -> Result<()> {
        self.conn.execute(
            r#"
            insert into handoffs (id, session_id, json)
            values (?1, ?2, ?3)
            on conflict(id) do update set session_id = excluded.session_id, json = excluded.json
            "#,
            params![
                &handoff.id.0,
                &handoff.session_id.0,
                serde_json::to_string(handoff)?,
            ],
        )?;
        Ok(())
    }

    pub fn get_handoff(&self, id: &HandoffId) -> Result<Option<HandoffSummary>> {
        self.get_json("handoffs", &id.0)
    }

    pub fn list_handoffs_for_session(
        &self,
        session_id: &TaskSessionId,
    ) -> Result<Vec<HandoffSummary>> {
        let mut stmt = self
            .conn
            .prepare("select json from handoffs where session_id = ?1 order by rowid")?;
        let rows = stmt.query_map(params![&session_id.0], |row| row.get::<_, String>(0))?;
        let mut handoffs = Vec::new();
        for row in rows {
            handoffs.push(serde_json::from_str(&row?)?);
        }
        Ok(handoffs)
    }

    pub fn upsert_permission(&self, permission: &PermissionRequest) -> Result<()> {
        self.conn.execute(
            r#"
            insert into permissions (id, session_id, json)
            values (?1, ?2, ?3)
            on conflict(id) do update set session_id = excluded.session_id, json = excluded.json
            "#,
            params![
                &permission.id.0,
                permission.session_id.as_ref().map(|id| id.0.as_str()),
                serde_json::to_string(permission)?,
            ],
        )?;
        Ok(())
    }

    pub fn get_permission(&self, id: &PermissionId) -> Result<Option<PermissionRequest>> {
        self.get_json("permissions", &id.0)
    }

    pub fn list_permissions(&self) -> Result<Vec<PermissionRequest>> {
        self.list_json("permissions")
    }

    pub fn list_permissions_for_session(
        &self,
        session_id: &TaskSessionId,
    ) -> Result<Vec<PermissionRequest>> {
        let mut stmt = self
            .conn
            .prepare("select json from permissions where session_id = ?1 order by rowid")?;
        let rows = stmt.query_map(params![&session_id.0], |row| row.get::<_, String>(0))?;
        let mut permissions = Vec::new();
        for row in rows {
            permissions.push(serde_json::from_str(&row?)?);
        }
        Ok(permissions)
    }

    fn upsert_json<T: Serialize>(&self, table: &str, id: &str, value: &T) -> Result<()> {
        let sql = format!(
            "insert into {table} (id, json) values (?1, ?2) on conflict(id) do update set json = excluded.json"
        );
        self.conn
            .execute(&sql, params![id, serde_json::to_string(value)?])?;
        Ok(())
    }

    fn get_json<T: DeserializeOwned>(&self, table: &str, id: &str) -> Result<Option<T>> {
        let sql = format!("select json from {table} where id = ?1");
        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query(params![id])?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };
        let raw: String = row.get(0)?;
        Ok(Some(serde_json::from_str(&raw)?))
    }

    fn list_json<T: DeserializeOwned>(&self, table: &str) -> Result<Vec<T>> {
        let sql = format!("select json from {table} order by rowid");
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut values = Vec::new();
        for row in rows {
            values.push(serde_json::from_str(&row?)?);
        }
        Ok(values)
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

#[cfg(test)]
mod tests {
    use super::*;
    use baize_core::{
        BaizeEvent, PermissionStatus, ProjectKind, ProviderId, RoutingMode, TaskSessionStatus,
        TrustLevel, VcsKind, WorkspaceId,
    };
    use chrono::Utc;
    use serde_json::json;

    #[test]
    fn creates_database_and_appends_events() {
        let temp = tempfile::tempdir().expect("temp dir");
        let db_path = temp.path().join("baize.db");
        let store = EventStore::open(&db_path).expect("store should open");

        assert_eq!(store.event_count().expect("count"), 0);

        let event = BaizeEvent::new("test.event", json!({ "ok": true }));
        store.append_event(&event).expect("append should work");

        assert_eq!(store.event_count().expect("count"), 1);
        assert!(db_path.exists());
    }

    #[test]
    fn lists_events_for_session() {
        let temp = tempfile::tempdir().expect("temp dir");
        let store = EventStore::open(temp.path().join("baize.db")).expect("store should open");
        let session_id = TaskSessionId::new();
        let mut event = BaizeEvent::new("session.agent.completed", json!({ "status": "ok" }));
        event.session_id = Some(session_id.clone());

        store.append_event(&event).expect("append should work");
        let events = store
            .list_events_for_session(&session_id)
            .expect("events should load");

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "session.agent.completed");
        assert_eq!(events[0].payload["status"], "ok");
    }

    #[test]
    fn lists_route_decisions_for_session() {
        let temp = tempfile::tempdir().expect("temp dir");
        let store = EventStore::open(temp.path().join("baize.db")).expect("store should open");
        let session_id = TaskSessionId::new();
        let other_session_id = TaskSessionId::new();
        let now = Utc::now();
        let first = RouteDecision {
            id: RouteDecisionId::new(),
            session_id: session_id.clone(),
            selected_provider_id: ProviderId("codex".to_string()),
            previous_provider_id: None,
            reason: "initial".to_string(),
            confidence: 0.75,
            mode: RoutingMode::Assisted,
            created_at: now,
        };
        let second = RouteDecision {
            id: RouteDecisionId::new(),
            session_id: session_id.clone(),
            selected_provider_id: ProviderId("gemini".to_string()),
            previous_provider_id: Some(ProviderId("codex".to_string())),
            reason: "handoff".to_string(),
            confidence: 0.9,
            mode: RoutingMode::Assisted,
            created_at: now,
        };
        let other = RouteDecision {
            id: RouteDecisionId::new(),
            session_id: other_session_id,
            selected_provider_id: ProviderId("opencode".to_string()),
            previous_provider_id: None,
            reason: "other".to_string(),
            confidence: 0.5,
            mode: RoutingMode::Assisted,
            created_at: now,
        };

        store.insert_route_decision(&first).expect("first");
        store.insert_route_decision(&second).expect("second");
        store.insert_route_decision(&other).expect("other");

        let decisions = store
            .list_route_decisions_for_session(&session_id)
            .expect("decisions");

        assert_eq!(decisions.len(), 2);
        assert_eq!(decisions[0].selected_provider_id.0, "codex");
        assert_eq!(decisions[1].selected_provider_id.0, "gemini");
    }

    #[test]
    fn lists_permission_requests() {
        let temp = tempfile::tempdir().expect("temp dir");
        let store = EventStore::open(temp.path().join("baize.db")).expect("store should open");
        let session_id = TaskSessionId::new();
        let first = PermissionRequest {
            id: PermissionId::new(),
            workspace_id: Some(WorkspaceId::new()),
            session_id: Some(session_id),
            command: "cargo test".to_string(),
            reason: "verify changes".to_string(),
            status: PermissionStatus::Pending,
            created_at: Utc::now(),
            resolved_at: None,
        };
        let second = PermissionRequest {
            id: PermissionId::new(),
            workspace_id: None,
            session_id: None,
            command: "cargo fmt".to_string(),
            reason: "format changes".to_string(),
            status: PermissionStatus::Approved,
            created_at: Utc::now(),
            resolved_at: Some(Utc::now()),
        };

        store.upsert_permission(&first).expect("first permission");
        store.upsert_permission(&second).expect("second permission");

        let permissions = store.list_permissions().expect("permissions");

        assert_eq!(permissions.len(), 2);
        assert_eq!(permissions[0].command, "cargo test");
        assert!(matches!(permissions[1].status, PermissionStatus::Approved));
    }

    #[test]
    fn lists_handoffs_for_session() {
        let temp = tempfile::tempdir().expect("temp dir");
        let store = EventStore::open(temp.path().join("baize.db")).expect("store should open");
        let session_id = TaskSessionId::new();
        let other_session_id = TaskSessionId::new();
        let now = Utc::now();
        let first = HandoffSummary {
            id: HandoffId::new(),
            session_id: session_id.clone(),
            from_provider_id: ProviderId("codex".to_string()),
            to_provider_id: ProviderId("gemini".to_string()),
            summary_markdown: "# Handoff 1".to_string(),
            mechanical_facts: baize_core::HandoffFacts::default(),
            status: baize_core::HandoffStatus::Accepted,
            created_at: now,
        };
        let second = HandoffSummary {
            id: HandoffId::new(),
            session_id: session_id.clone(),
            from_provider_id: ProviderId("gemini".to_string()),
            to_provider_id: ProviderId("codex".to_string()),
            summary_markdown: "# Handoff 2".to_string(),
            mechanical_facts: baize_core::HandoffFacts::default(),
            status: baize_core::HandoffStatus::Draft,
            created_at: now,
        };
        let other = HandoffSummary {
            id: HandoffId::new(),
            session_id: other_session_id,
            from_provider_id: ProviderId("codex".to_string()),
            to_provider_id: ProviderId("opencode".to_string()),
            summary_markdown: "# Other".to_string(),
            mechanical_facts: baize_core::HandoffFacts::default(),
            status: baize_core::HandoffStatus::Draft,
            created_at: now,
        };

        store.insert_handoff(&first).expect("first handoff");
        store.insert_handoff(&second).expect("second handoff");
        store.insert_handoff(&other).expect("other handoff");

        let handoffs = store
            .list_handoffs_for_session(&session_id)
            .expect("handoffs");

        assert_eq!(handoffs.len(), 2);
        assert_eq!(handoffs[0].to_provider_id.0, "gemini");
        assert_eq!(handoffs[1].to_provider_id.0, "codex");
    }

    #[test]
    fn lists_permissions_for_session() {
        let temp = tempfile::tempdir().expect("temp dir");
        let store = EventStore::open(temp.path().join("baize.db")).expect("store should open");
        let session_id = TaskSessionId::new();
        let other_session_id = TaskSessionId::new();
        let first = PermissionRequest {
            id: PermissionId::new(),
            workspace_id: Some(WorkspaceId::new()),
            session_id: Some(session_id.clone()),
            command: "cargo test".to_string(),
            reason: "verify".to_string(),
            status: PermissionStatus::Pending,
            created_at: Utc::now(),
            resolved_at: None,
        };
        let second = PermissionRequest {
            id: PermissionId::new(),
            workspace_id: None,
            session_id: Some(session_id.clone()),
            command: "cargo fmt".to_string(),
            reason: "format".to_string(),
            status: PermissionStatus::Approved,
            created_at: Utc::now(),
            resolved_at: Some(Utc::now()),
        };
        let other = PermissionRequest {
            id: PermissionId::new(),
            workspace_id: None,
            session_id: Some(other_session_id),
            command: "rm -rf".to_string(),
            reason: "cleanup".to_string(),
            status: PermissionStatus::Pending,
            created_at: Utc::now(),
            resolved_at: None,
        };

        store.upsert_permission(&first).expect("first permission");
        store.upsert_permission(&second).expect("second permission");
        store.upsert_permission(&other).expect("other permission");

        let permissions = store
            .list_permissions_for_session(&session_id)
            .expect("permissions");

        assert_eq!(permissions.len(), 2);
        assert_eq!(permissions[0].command, "cargo test");
        assert_eq!(permissions[1].command, "cargo fmt");
    }

    #[test]
    fn stores_workspace_project_and_session_records() {
        let temp = tempfile::tempdir().expect("temp dir");
        let store = EventStore::open(temp.path().join("baize.db")).expect("store should open");
        let workspace_id = WorkspaceId::new();
        let project_id = ProjectId::new();
        let now = Utc::now();
        let workspace = Workspace {
            id: workspace_id.clone(),
            name: "test".to_string(),
            primary_project_id: project_id.clone(),
            created_at: now,
            updated_at: now,
        };
        let project = Project {
            id: project_id,
            workspace_id: workspace_id.clone(),
            root: temp.path().to_path_buf(),
            kind: ProjectKind::Directory,
            vcs: VcsKind::None,
            active_branch: None,
            trust_level: TrustLevel::Trusted,
            created_at: now,
            updated_at: now,
        };
        let session = TaskSession {
            id: TaskSessionId::new(),
            workspace_id,
            objective: "test objective".to_string(),
            active_provider_id: None,
            status: TaskSessionStatus::Running,
            created_at: now,
            updated_at: now,
        };

        store.upsert_workspace(&workspace).expect("workspace");
        store.upsert_project(&project).expect("project");
        store.upsert_task_session(&session).expect("session");

        assert_eq!(store.list_workspaces().expect("workspaces").len(), 1);
        assert_eq!(
            store
                .get_primary_project(&workspace)
                .expect("project lookup")
                .expect("project exists")
                .root,
            temp.path()
        );
        assert_eq!(
            store
                .get_task_session(&session.id)
                .expect("session lookup")
                .expect("session exists")
                .objective,
            "test objective"
        );
    }
}
