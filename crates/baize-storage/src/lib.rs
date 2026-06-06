use anyhow::{Context, Result};
use baize_core::{
    BaizeEvent, HandoffId, HandoffStatus, HandoffSummary, PermissionId, PermissionRequest,
    PermissionRiskLevel, Project, ProjectId, ProviderId, RouteDecision, RouteDecisionId,
    RoutingMode, TaskSession, TaskSessionId, TaskSessionStatus, TaskType, Workspace, WorkspaceId,
};
use rusqlite::{params, Connection};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

pub struct EventStore {
    conn: Connection,
    data_dir: PathBuf,
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
        let data_dir = path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let store = Self { conn, data_dir };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(
            "create table if not exists schema_version (version integer primary key);",
        )?;
        let current: i64 = self
            .conn
            .query_row(
                "select coalesce(max(version), 0) from schema_version",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        let migrations: &[(&str, &str)] = &[
            (
                "v1",
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
                    json text not null
                );
                "#,
            ),
            ("v2", "alter table permissions add column session_id text;"),
            (
                "v3",
                r#"
                create index if not exists events_session_timestamp_idx on events(session_id, timestamp);
                create index if not exists events_workspace_timestamp_idx on events(workspace_id, timestamp);
                create index if not exists events_provider_timestamp_idx on events(provider_id, timestamp);
                create index if not exists projects_workspace_idx on projects(workspace_id);
                create index if not exists task_sessions_workspace_idx on task_sessions(workspace_id);
                create index if not exists route_decisions_session_idx on route_decisions(session_id);
                create index if not exists handoffs_session_idx on handoffs(session_id);
                create index if not exists permissions_session_idx on permissions(session_id);
                "#,
            ),
            (
                "v4",
                r#"
                alter table permissions add column risk_level text;
                create index if not exists permissions_risk_level_idx on permissions(risk_level);
                "#,
            ),
            (
                "v5",
                r#"
                alter table task_sessions add column status text;
                alter table task_sessions add column active_provider_id text;
                create index if not exists task_sessions_status_idx on task_sessions(status);
                create index if not exists task_sessions_active_provider_idx on task_sessions(active_provider_id);
                "#,
            ),
            (
                "v6",
                r#"
                alter table handoffs add column status text;
                alter table handoffs add column to_provider_id text;
                create index if not exists handoffs_status_idx on handoffs(status);
                create index if not exists handoffs_to_provider_idx on handoffs(to_provider_id);
                "#,
            ),
            (
                "v7",
                r#"
                alter table route_decisions add column selected_provider_id text;
                alter table route_decisions add column task_type text;
                alter table route_decisions add column mode text;
                create index if not exists route_decisions_selected_provider_idx on route_decisions(selected_provider_id);
                create index if not exists route_decisions_task_type_idx on route_decisions(task_type);
                create index if not exists route_decisions_mode_idx on route_decisions(mode);
                "#,
            ),
            (
                "v8",
                r#"
                alter table projects add column root text;
                alter table projects add column kind text;
                alter table projects add column vcs text;
                create index if not exists projects_root_idx on projects(root);
                create index if not exists projects_kind_idx on projects(kind);
                create index if not exists projects_vcs_idx on projects(vcs);
                "#,
            ),
        ];
        for (index, (label, sql)) in migrations.iter().enumerate() {
            let version = (index + 1) as i64;
            if current < version {
                self.conn
                    .execute_batch(sql)
                    .with_context(|| format!("migration {label} failed"))?;
                self.conn
                    .execute(
                        "insert into schema_version (version) values (?1)",
                        params![version],
                    )
                    .with_context(|| format!("recording migration {label} failed"))?;
            }
        }
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

    pub fn schema_version(&self) -> Result<i64> {
        let version = self
            .conn
            .query_row(
                "select coalesce(max(version), 0) from schema_version",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        Ok(version)
    }

    pub fn list_events_for_session(
        &self,
        session_id: &TaskSessionId,
        limit: Option<u64>,
        offset: Option<u64>,
    ) -> Result<Vec<BaizeEvent>> {
        let limit = limit.unwrap_or(100);
        let offset = offset.unwrap_or(0);
        let mut stmt = self.conn.prepare(
            "select id, event_type, timestamp, workspace_id, session_id, provider_id, payload from events where session_id = ?1 order by timestamp limit ?2 offset ?3",
        )?;
        let rows = stmt.query_map(params![&session_id.0, limit, offset], |row| {
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
            insert into projects (id, workspace_id, root, kind, vcs, json)
            values (?1, ?2, ?3, ?4, ?5, ?6)
            on conflict(id) do update set
                workspace_id = excluded.workspace_id,
                root = excluded.root,
                kind = excluded.kind,
                vcs = excluded.vcs,
                json = excluded.json
            "#,
            params![
                &project.id.0,
                &project.workspace_id.0,
                project.root.to_string_lossy().as_ref(),
                project_kind_label(&project.kind),
                vcs_kind_label(&project.vcs),
                serde_json::to_string(project)?,
            ],
        )?;
        Ok(())
    }

    pub fn get_project(&self, id: &ProjectId) -> Result<Option<Project>> {
        self.get_json("projects", &id.0)
    }

    pub fn list_projects_for_workspace(&self, workspace_id: &WorkspaceId) -> Result<Vec<Project>> {
        let mut stmt = self
            .conn
            .prepare("select json from projects where workspace_id = ?1 order by rowid")?;
        let rows = stmt.query_map(params![&workspace_id.0], |row| row.get::<_, String>(0))?;
        let mut projects = Vec::new();
        for row in rows {
            projects.push(serde_json::from_str(&row?)?);
        }
        Ok(projects)
    }

    pub fn list_projects_by_kind(&self, kind: &str) -> Result<Vec<Project>> {
        let mut stmt = self
            .conn
            .prepare("select json from projects where kind = ?1 order by rowid")?;
        let rows = stmt.query_map(params![kind], |row| row.get::<_, String>(0))?;
        let mut projects = Vec::new();
        for row in rows {
            projects.push(serde_json::from_str(&row?)?);
        }
        Ok(projects)
    }

    pub fn list_projects_by_vcs(&self, vcs: &str) -> Result<Vec<Project>> {
        let mut stmt = self
            .conn
            .prepare("select json from projects where vcs = ?1 order by rowid")?;
        let rows = stmt.query_map(params![vcs], |row| row.get::<_, String>(0))?;
        let mut projects = Vec::new();
        for row in rows {
            projects.push(serde_json::from_str(&row?)?);
        }
        Ok(projects)
    }

    pub fn get_primary_project(&self, workspace: &Workspace) -> Result<Option<Project>> {
        self.get_project(&workspace.primary_project_id)
    }

    pub fn upsert_task_session(&self, session: &TaskSession) -> Result<()> {
        self.conn.execute(
            r#"
            insert into task_sessions (id, workspace_id, status, active_provider_id, json)
            values (?1, ?2, ?3, ?4, ?5)
            on conflict(id) do update set
                workspace_id = excluded.workspace_id,
                status = excluded.status,
                active_provider_id = excluded.active_provider_id,
                json = excluded.json
            "#,
            params![
                &session.id.0,
                &session.workspace_id.0,
                task_session_status(&session.status),
                session.active_provider_id.as_ref().map(|id| id.0.as_str()),
                serde_json::to_string(session)?,
            ],
        )?;
        Ok(())
    }

    pub fn list_task_sessions(
        &self,
        limit: Option<u64>,
        offset: Option<u64>,
    ) -> Result<Vec<TaskSession>> {
        self.list_json_paginated("task_sessions", limit, offset)
    }

    pub fn list_task_sessions_by_status(
        &self,
        status: &str,
        limit: Option<u64>,
        offset: Option<u64>,
    ) -> Result<Vec<TaskSession>> {
        let limit = limit.unwrap_or(100);
        let offset = offset.unwrap_or(0);
        let mut stmt = self.conn.prepare(
            "select json from task_sessions where status = ?1 order by rowid limit ?2 offset ?3",
        )?;
        let rows = stmt.query_map(params![status, limit, offset], |row| {
            row.get::<_, String>(0)
        })?;
        let mut sessions = Vec::new();
        for row in rows {
            sessions.push(serde_json::from_str(&row?)?);
        }
        Ok(sessions)
    }

    pub fn list_task_sessions_by_active_provider(
        &self,
        provider_id: &ProviderId,
        limit: Option<u64>,
        offset: Option<u64>,
    ) -> Result<Vec<TaskSession>> {
        let limit = limit.unwrap_or(100);
        let offset = offset.unwrap_or(0);
        let mut stmt = self.conn.prepare(
            "select json from task_sessions where active_provider_id = ?1 order by rowid limit ?2 offset ?3",
        )?;
        let rows = stmt.query_map(params![&provider_id.0, limit, offset], |row| {
            row.get::<_, String>(0)
        })?;
        let mut sessions = Vec::new();
        for row in rows {
            sessions.push(serde_json::from_str(&row?)?);
        }
        Ok(sessions)
    }

    pub fn get_task_session(&self, id: &TaskSessionId) -> Result<Option<TaskSession>> {
        self.get_json("task_sessions", &id.0)
    }

    pub fn get_latest_session_for_workspace(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<Option<TaskSession>> {
        let mut stmt = self.conn.prepare(
            "select json from task_sessions where workspace_id = ?1 order by rowid desc limit 1",
        )?;
        let mut rows = stmt.query(params![&workspace_id.0])?;
        match rows.next()? {
            Some(row) => {
                let json_str: String = row.get(0)?;
                Ok(Some(serde_json::from_str(&json_str)?))
            }
            None => Ok(None),
        }
    }

    pub fn insert_route_decision(&self, decision: &RouteDecision) -> Result<()> {
        self.conn.execute(
            r#"
            insert into route_decisions (
                id, session_id, selected_provider_id, task_type, mode, json
            ) values (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
            params![
                &decision.id.0,
                &decision.session_id.0,
                &decision.selected_provider_id.0,
                decision.task_type.as_ref().map(task_type_label),
                routing_mode_label(&decision.mode),
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

    pub fn list_route_decisions_for_session_by_selected_provider(
        &self,
        session_id: &TaskSessionId,
        provider_id: &ProviderId,
    ) -> Result<Vec<RouteDecision>> {
        let mut stmt = self.conn.prepare(
            "select json from route_decisions where session_id = ?1 and selected_provider_id = ?2 order by rowid",
        )?;
        let rows = stmt.query_map(params![&session_id.0, &provider_id.0], |row| {
            row.get::<_, String>(0)
        })?;
        let mut decisions = Vec::new();
        for row in rows {
            decisions.push(serde_json::from_str(&row?)?);
        }
        Ok(decisions)
    }

    pub fn list_route_decisions_for_session_by_task_type(
        &self,
        session_id: &TaskSessionId,
        task_type: &str,
    ) -> Result<Vec<RouteDecision>> {
        let mut stmt = self.conn.prepare(
            "select json from route_decisions where session_id = ?1 and task_type = ?2 order by rowid",
        )?;
        let rows = stmt.query_map(params![&session_id.0, task_type], |row| {
            row.get::<_, String>(0)
        })?;
        let mut decisions = Vec::new();
        for row in rows {
            decisions.push(serde_json::from_str(&row?)?);
        }
        Ok(decisions)
    }

    pub fn list_route_decisions_for_session_by_mode(
        &self,
        session_id: &TaskSessionId,
        mode: &str,
    ) -> Result<Vec<RouteDecision>> {
        let mut stmt = self.conn.prepare(
            "select json from route_decisions where session_id = ?1 and mode = ?2 order by rowid",
        )?;
        let rows = stmt.query_map(params![&session_id.0, mode], |row| row.get::<_, String>(0))?;
        let mut decisions = Vec::new();
        for row in rows {
            decisions.push(serde_json::from_str(&row?)?);
        }
        Ok(decisions)
    }

    pub fn insert_handoff(&self, handoff: &HandoffSummary) -> Result<()> {
        self.conn.execute(
            r#"
            insert into handoffs (id, session_id, status, to_provider_id, json)
            values (?1, ?2, ?3, ?4, ?5)
            on conflict(id) do update set
                session_id = excluded.session_id,
                status = excluded.status,
                to_provider_id = excluded.to_provider_id,
                json = excluded.json
            "#,
            params![
                &handoff.id.0,
                &handoff.session_id.0,
                handoff_status(&handoff.status),
                &handoff.to_provider_id.0,
                serde_json::to_string(handoff)?,
            ],
        )?;
        Ok(())
    }

    pub fn write_handoff_artifact(&self, handoff: &HandoffSummary) -> Result<PathBuf> {
        let artifact_dir = self.data_dir.join("artifacts").join("handoffs");
        fs::create_dir_all(&artifact_dir)
            .with_context(|| format!("failed to create {}", artifact_dir.display()))?;
        let artifact_path = artifact_dir.join(format!("{}.md", handoff.id.0));
        fs::write(&artifact_path, &handoff.summary_markdown)
            .with_context(|| format!("failed to write {}", artifact_path.display()))?;
        Ok(artifact_path)
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

    pub fn list_handoffs_for_session_by_status(
        &self,
        session_id: &TaskSessionId,
        status: &str,
    ) -> Result<Vec<HandoffSummary>> {
        let mut stmt = self.conn.prepare(
            "select json from handoffs where session_id = ?1 and status = ?2 order by rowid",
        )?;
        let rows = stmt.query_map(params![&session_id.0, status], |row| {
            row.get::<_, String>(0)
        })?;
        let mut handoffs = Vec::new();
        for row in rows {
            handoffs.push(serde_json::from_str(&row?)?);
        }
        Ok(handoffs)
    }

    pub fn list_handoffs_for_session_by_to_provider(
        &self,
        session_id: &TaskSessionId,
        provider_id: &ProviderId,
    ) -> Result<Vec<HandoffSummary>> {
        let mut stmt = self.conn.prepare(
            "select json from handoffs where session_id = ?1 and to_provider_id = ?2 order by rowid",
        )?;
        let rows = stmt.query_map(params![&session_id.0, &provider_id.0], |row| {
            row.get::<_, String>(0)
        })?;
        let mut handoffs = Vec::new();
        for row in rows {
            handoffs.push(serde_json::from_str(&row?)?);
        }
        Ok(handoffs)
    }

    pub fn upsert_permission(&self, permission: &PermissionRequest) -> Result<()> {
        self.conn.execute(
            r#"
            insert into permissions (id, session_id, risk_level, json)
            values (?1, ?2, ?3, ?4)
            on conflict(id) do update set
                session_id = excluded.session_id,
                risk_level = excluded.risk_level,
                json = excluded.json
            "#,
            params![
                &permission.id.0,
                permission.session_id.as_ref().map(|id| id.0.as_str()),
                permission_risk_level(&permission.risk.level),
                serde_json::to_string(permission)?,
            ],
        )?;
        Ok(())
    }

    pub fn get_permission(&self, id: &PermissionId) -> Result<Option<PermissionRequest>> {
        self.get_json("permissions", &id.0)
    }

    pub fn list_permissions(
        &self,
        limit: Option<u64>,
        offset: Option<u64>,
    ) -> Result<Vec<PermissionRequest>> {
        self.list_json_paginated("permissions", limit, offset)
    }

    pub fn list_permissions_for_session(
        &self,
        session_id: &TaskSessionId,
        limit: Option<u64>,
        offset: Option<u64>,
    ) -> Result<Vec<PermissionRequest>> {
        let limit = limit.unwrap_or(100);
        let offset = offset.unwrap_or(0);
        let mut stmt = self.conn.prepare(
            "select json from permissions where session_id = ?1 order by rowid limit ?2 offset ?3",
        )?;
        let rows = stmt.query_map(params![&session_id.0, limit, offset], |row| {
            row.get::<_, String>(0)
        })?;
        let mut permissions = Vec::new();
        for row in rows {
            permissions.push(serde_json::from_str(&row?)?);
        }
        Ok(permissions)
    }

    pub fn list_permissions_by_risk_level(
        &self,
        risk_level: &str,
        limit: Option<u64>,
        offset: Option<u64>,
    ) -> Result<Vec<PermissionRequest>> {
        let limit = limit.unwrap_or(100);
        let offset = offset.unwrap_or(0);
        let mut stmt = self.conn.prepare(
            "select json from permissions where risk_level = ?1 order by rowid limit ?2 offset ?3",
        )?;
        let rows = stmt.query_map(params![risk_level, limit, offset], |row| {
            row.get::<_, String>(0)
        })?;
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

    fn list_json_paginated<T: DeserializeOwned>(
        &self,
        table: &str,
        limit: Option<u64>,
        offset: Option<u64>,
    ) -> Result<Vec<T>> {
        let limit = limit.unwrap_or(100);
        let offset = offset.unwrap_or(0);
        let sql = format!("select json from {table} order by rowid limit ?1 offset ?2");
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![limit, offset], |row| row.get::<_, String>(0))?;
        let mut values = Vec::new();
        for row in rows {
            values.push(serde_json::from_str(&row?)?);
        }
        Ok(values)
    }
}

fn permission_risk_level(level: &PermissionRiskLevel) -> &'static str {
    match level {
        PermissionRiskLevel::Low => "Low",
        PermissionRiskLevel::Medium => "Medium",
        PermissionRiskLevel::High => "High",
        PermissionRiskLevel::Blocked => "Blocked",
    }
}

fn project_kind_label(kind: &baize_core::ProjectKind) -> &'static str {
    match kind {
        baize_core::ProjectKind::GitRepo => "GitRepo",
        baize_core::ProjectKind::Directory => "Directory",
    }
}

fn vcs_kind_label(vcs: &baize_core::VcsKind) -> &'static str {
    match vcs {
        baize_core::VcsKind::Git => "Git",
        baize_core::VcsKind::None => "None",
    }
}

fn task_session_status(status: &TaskSessionStatus) -> &'static str {
    match status {
        TaskSessionStatus::Running => "Running",
        TaskSessionStatus::WaitingForPermission => "WaitingForPermission",
        TaskSessionStatus::Completed => "Completed",
        TaskSessionStatus::Failed => "Failed",
        TaskSessionStatus::Canceled => "Canceled",
    }
}

fn task_type_label(task_type: &TaskType) -> &'static str {
    match task_type {
        TaskType::Testing => "Testing",
        TaskType::Debugging => "Debugging",
        TaskType::Refactor => "Refactor",
        TaskType::Documentation => "Documentation",
        TaskType::Implementation => "Implementation",
    }
}

fn routing_mode_label(mode: &RoutingMode) -> &'static str {
    match mode {
        RoutingMode::Manual => "Manual",
        RoutingMode::Assisted => "Assisted",
        RoutingMode::Autopilot => "Autopilot",
    }
}

fn handoff_status(status: &HandoffStatus) -> &'static str {
    match status {
        HandoffStatus::Draft => "Draft",
        HandoffStatus::Accepted => "Accepted",
        HandoffStatus::Failed => "Failed",
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
        BaizeEvent, HandoffFacts, HandoffStatus, PermissionRiskAssessment, PermissionRiskLevel,
        PermissionStatus, ProjectKind, ProviderId, RoutingMode, TaskSessionStatus, TrustLevel,
        VcsKind, WorkspaceId,
    };
    use chrono::Utc;
    use serde_json::json;

    fn index_names(store: &EventStore) -> Vec<String> {
        let mut stmt = store
            .conn
            .prepare("select name from sqlite_master where type = 'index' order by name")
            .expect("index query");
        stmt.query_map([], |row| row.get::<_, String>(0))
            .expect("index rows")
            .map(|row| row.expect("index name"))
            .collect()
    }

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
            .list_events_for_session(&session_id, None, None)
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
            task_type: Some(TaskType::Testing),
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
            task_type: Some(TaskType::Debugging),
            confidence: 0.9,
            mode: RoutingMode::Manual,
            created_at: now,
        };
        let other = RouteDecision {
            id: RouteDecisionId::new(),
            session_id: other_session_id,
            selected_provider_id: ProviderId("opencode".to_string()),
            previous_provider_id: None,
            reason: "other".to_string(),
            task_type: None,
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

        let gemini = store
            .list_route_decisions_for_session_by_selected_provider(
                &session_id,
                &ProviderId("gemini".to_string()),
            )
            .expect("gemini decisions");
        assert_eq!(gemini.len(), 1);
        assert_eq!(gemini[0].reason, "handoff");

        let testing = store
            .list_route_decisions_for_session_by_task_type(&session_id, "Testing")
            .expect("testing decisions");
        assert_eq!(testing.len(), 1);
        assert_eq!(testing[0].selected_provider_id.0, "codex");

        let manual = store
            .list_route_decisions_for_session_by_mode(&session_id, "Manual")
            .expect("manual decisions");
        assert_eq!(manual.len(), 1);
        assert_eq!(manual[0].selected_provider_id.0, "gemini");
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
            risk: PermissionRiskAssessment::default(),
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
            risk: PermissionRiskAssessment::default(),
            status: PermissionStatus::Approved,
            created_at: Utc::now(),
            resolved_at: Some(Utc::now()),
        };

        store.upsert_permission(&first).expect("first permission");
        store.upsert_permission(&second).expect("second permission");

        let permissions = store.list_permissions(None, None).expect("permissions");

        assert_eq!(permissions.len(), 2);
        assert_eq!(permissions[0].command, "cargo test");
        assert!(matches!(permissions[1].status, PermissionStatus::Approved));
    }

    #[test]
    fn lists_permissions_by_risk_level() {
        let temp = tempfile::tempdir().expect("temp dir");
        let store = EventStore::open(temp.path().join("baize.db")).expect("store should open");
        let low = PermissionRequest {
            id: PermissionId::new(),
            workspace_id: None,
            session_id: None,
            command: "cargo test".to_string(),
            reason: "verify".to_string(),
            risk: PermissionRiskAssessment::default(),
            status: PermissionStatus::Pending,
            created_at: Utc::now(),
            resolved_at: None,
        };
        let high = PermissionRequest {
            id: PermissionId::new(),
            workspace_id: None,
            session_id: None,
            command: "sudo chmod 777 /tmp/file".to_string(),
            reason: "change permissions".to_string(),
            risk: PermissionRiskAssessment {
                level: PermissionRiskLevel::High,
                reasons: vec!["elevated privileges".to_string()],
                recommendation: "Review before approving.".to_string(),
            },
            status: PermissionStatus::Pending,
            created_at: Utc::now(),
            resolved_at: None,
        };

        store.upsert_permission(&low).expect("low permission");
        store.upsert_permission(&high).expect("high permission");

        let high_risk = store
            .list_permissions_by_risk_level("High", None, None)
            .expect("high risk permissions");

        assert_eq!(high_risk.len(), 1);
        assert_eq!(high_risk[0].command, "sudo chmod 777 /tmp/file");
        assert_eq!(high_risk[0].risk.level, PermissionRiskLevel::High);
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
    fn lists_handoffs_for_session_by_status_and_to_provider() {
        let temp = tempfile::tempdir().expect("temp dir");
        let store = EventStore::open(temp.path().join("baize.db")).expect("store should open");
        let session_id = TaskSessionId::new();
        let draft = HandoffSummary {
            id: HandoffId::new(),
            session_id: session_id.clone(),
            from_provider_id: ProviderId("codex".to_string()),
            to_provider_id: ProviderId("gemini".to_string()),
            summary_markdown: "# Draft".to_string(),
            mechanical_facts: HandoffFacts::default(),
            status: HandoffStatus::Draft,
            created_at: Utc::now(),
        };
        let accepted = HandoffSummary {
            id: HandoffId::new(),
            session_id: session_id.clone(),
            from_provider_id: ProviderId("gemini".to_string()),
            to_provider_id: ProviderId("codex".to_string()),
            summary_markdown: "# Accepted".to_string(),
            mechanical_facts: HandoffFacts::default(),
            status: HandoffStatus::Accepted,
            created_at: Utc::now(),
        };

        store.insert_handoff(&draft).expect("draft handoff");
        store.insert_handoff(&accepted).expect("accepted handoff");

        let drafts = store
            .list_handoffs_for_session_by_status(&session_id, "Draft")
            .expect("drafts");
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].to_provider_id.0, "gemini");

        let to_codex = store
            .list_handoffs_for_session_by_to_provider(&session_id, &ProviderId("codex".to_string()))
            .expect("codex handoffs");
        assert_eq!(to_codex.len(), 1);
        assert!(matches!(to_codex[0].status, HandoffStatus::Accepted));
    }

    #[test]
    fn writes_handoff_markdown_artifact() {
        let temp = tempfile::tempdir().expect("temp dir");
        let store = EventStore::open(temp.path().join("baize.db")).expect("store should open");
        let handoff = HandoffSummary {
            id: HandoffId::new(),
            session_id: TaskSessionId::new(),
            from_provider_id: ProviderId("codex".to_string()),
            to_provider_id: ProviderId("gemini".to_string()),
            summary_markdown: "# Handoff\n\nObjective: continue task\n".to_string(),
            mechanical_facts: HandoffFacts::default(),
            status: HandoffStatus::Draft,
            created_at: Utc::now(),
        };

        let artifact_path = store
            .write_handoff_artifact(&handoff)
            .expect("artifact write");

        assert!(artifact_path.starts_with(temp.path()));
        assert_eq!(
            std::fs::read_to_string(artifact_path).expect("artifact contents"),
            handoff.summary_markdown
        );
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
            risk: PermissionRiskAssessment::default(),
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
            risk: PermissionRiskAssessment::default(),
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
            risk: PermissionRiskAssessment::default(),
            status: PermissionStatus::Pending,
            created_at: Utc::now(),
            resolved_at: None,
        };

        store.upsert_permission(&first).expect("first permission");
        store.upsert_permission(&second).expect("second permission");
        store.upsert_permission(&other).expect("other permission");

        let permissions = store
            .list_permissions_for_session(&session_id, None, None)
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
        let workspace_projects = store
            .list_projects_for_workspace(&workspace.id)
            .expect("workspace projects");
        assert_eq!(workspace_projects.len(), 1);
        assert_eq!(workspace_projects[0].root, temp.path());

        let directories = store
            .list_projects_by_kind("Directory")
            .expect("directory projects");
        assert_eq!(directories.len(), 1);
        assert_eq!(directories[0].id.0, workspace.primary_project_id.0);

        let no_vcs = store
            .list_projects_by_vcs("None")
            .expect("non-vcs projects");
        assert_eq!(no_vcs.len(), 1);
        assert_eq!(no_vcs[0].id.0, workspace.primary_project_id.0);
        assert_eq!(
            store
                .get_task_session(&session.id)
                .expect("session lookup")
                .expect("session exists")
                .objective,
            "test objective"
        );
    }

    #[test]
    fn lists_task_sessions_by_status_and_active_provider() {
        let temp = tempfile::tempdir().expect("temp dir");
        let store = EventStore::open(temp.path().join("baize.db")).expect("store should open");
        let workspace_id = WorkspaceId::new();
        let now = Utc::now();
        let running = TaskSession {
            id: TaskSessionId::new(),
            workspace_id: workspace_id.clone(),
            objective: "running".to_string(),
            active_provider_id: Some(ProviderId("codex".to_string())),
            status: TaskSessionStatus::Running,
            created_at: now,
            updated_at: now,
        };
        let failed = TaskSession {
            id: TaskSessionId::new(),
            workspace_id,
            objective: "failed".to_string(),
            active_provider_id: Some(ProviderId("gemini".to_string())),
            status: TaskSessionStatus::Failed,
            created_at: now,
            updated_at: now,
        };

        store.upsert_task_session(&running).expect("running");
        store.upsert_task_session(&failed).expect("failed");

        let running_sessions = store
            .list_task_sessions_by_status("Running", None, None)
            .expect("running sessions");
        assert_eq!(running_sessions.len(), 1);
        assert_eq!(running_sessions[0].objective, "running");

        let gemini_sessions = store
            .list_task_sessions_by_active_provider(&ProviderId("gemini".to_string()), None, None)
            .expect("gemini sessions");
        assert_eq!(gemini_sessions.len(), 1);
        assert!(matches!(
            gemini_sessions[0].status,
            TaskSessionStatus::Failed
        ));
    }

    #[test]
    fn fresh_database_gets_latest_schema_version() {
        let temp = tempfile::tempdir().expect("temp dir");
        let store = EventStore::open(temp.path().join("baize.db")).expect("store should open");

        let version = store.schema_version().expect("schema version");
        assert!(version >= 8, "expected at least version 8, got {version}");
    }

    #[test]
    fn re_opening_database_does_not_re_run_migrations() {
        let temp = tempfile::tempdir().expect("temp dir");
        let db_path = temp.path().join("baize.db");

        let store = EventStore::open(&db_path).expect("store should open");
        let version_first = store.schema_version().expect("version");
        drop(store);

        let store = EventStore::open(&db_path).expect("store should re-open");
        let version_second = store.schema_version().expect("version");

        assert_eq!(version_first, version_second);
    }

    #[test]
    fn v2_permissions_session_id_column_exists() {
        let temp = tempfile::tempdir().expect("temp dir");
        let store = EventStore::open(temp.path().join("baize.db")).expect("store should open");

        let session_id = TaskSessionId::new();
        let permission = PermissionRequest {
            id: PermissionId::new(),
            workspace_id: None,
            session_id: Some(session_id.clone()),
            command: "ls".to_string(),
            reason: "list".to_string(),
            risk: PermissionRiskAssessment::default(),
            status: PermissionStatus::Pending,
            created_at: Utc::now(),
            resolved_at: None,
        };
        store.upsert_permission(&permission).expect("upsert");

        let by_session = store
            .list_permissions_for_session(&session_id, None, None)
            .expect("list by session");
        assert_eq!(by_session.len(), 1);
        assert_eq!(by_session[0].command, "ls");
    }

    #[test]
    fn v4_permissions_risk_level_column_exists() {
        let temp = tempfile::tempdir().expect("temp dir");
        let store = EventStore::open(temp.path().join("baize.db")).expect("store should open");
        let permission = PermissionRequest {
            id: PermissionId::new(),
            workspace_id: None,
            session_id: None,
            command: "sudo chmod 777 /tmp/file".to_string(),
            reason: "change permissions".to_string(),
            risk: PermissionRiskAssessment {
                level: PermissionRiskLevel::High,
                reasons: vec!["elevated privileges".to_string()],
                recommendation: "Review before approving.".to_string(),
            },
            status: PermissionStatus::Pending,
            created_at: Utc::now(),
            resolved_at: None,
        };
        store.upsert_permission(&permission).expect("upsert");

        let by_risk = store
            .list_permissions_by_risk_level("High", None, None)
            .expect("list by risk");
        assert_eq!(by_risk.len(), 1);
        assert_eq!(by_risk[0].command, "sudo chmod 777 /tmp/file");
    }

    #[test]
    fn query_indexes_exist() {
        let temp = tempfile::tempdir().expect("temp dir");
        let store = EventStore::open(temp.path().join("baize.db")).expect("store should open");
        let indexes = index_names(&store);

        for expected in [
            "events_session_timestamp_idx",
            "events_workspace_timestamp_idx",
            "events_provider_timestamp_idx",
            "projects_workspace_idx",
            "projects_root_idx",
            "projects_kind_idx",
            "projects_vcs_idx",
            "task_sessions_workspace_idx",
            "task_sessions_status_idx",
            "task_sessions_active_provider_idx",
            "route_decisions_session_idx",
            "route_decisions_selected_provider_idx",
            "route_decisions_task_type_idx",
            "route_decisions_mode_idx",
            "handoffs_session_idx",
            "handoffs_status_idx",
            "handoffs_to_provider_idx",
            "permissions_session_idx",
            "permissions_risk_level_idx",
        ] {
            assert!(
                indexes.iter().any(|index| index == expected),
                "missing index {expected}; found {indexes:?}"
            );
        }
    }

    #[test]
    fn v2_database_migrates_to_latest_indexes() {
        let temp = tempfile::tempdir().expect("temp dir");
        let db_path = temp.path().join("baize.db");
        {
            let conn = Connection::open(&db_path).expect("open raw db");
            conn.execute_batch(
                r#"
                create table schema_version (version integer primary key);
                insert into schema_version (version) values (1);
                insert into schema_version (version) values (2);
                create table events (
                    id text primary key,
                    event_type text not null,
                    timestamp text not null,
                    workspace_id text,
                    session_id text,
                    provider_id text,
                    payload text not null
                );
                create table workspaces (
                    id text primary key,
                    json text not null
                );
                create table projects (
                    id text primary key,
                    workspace_id text not null,
                    json text not null
                );
                create table task_sessions (
                    id text primary key,
                    workspace_id text not null,
                    json text not null
                );
                create table route_decisions (
                    id text primary key,
                    session_id text not null,
                    json text not null
                );
                create table handoffs (
                    id text primary key,
                    session_id text not null,
                    json text not null
                );
                create table permissions (
                    id text primary key,
                    json text not null,
                    session_id text
                );
                "#,
            )
            .expect("seed v2 schema");
        }

        let store = EventStore::open(&db_path).expect("store should migrate");

        assert_eq!(store.schema_version().expect("schema version"), 8);
        assert!(index_names(&store)
            .iter()
            .any(|index| index == "events_session_timestamp_idx"));
        assert!(index_names(&store)
            .iter()
            .any(|index| index == "permissions_risk_level_idx"));
        assert!(index_names(&store)
            .iter()
            .any(|index| index == "projects_root_idx"));
        assert!(index_names(&store)
            .iter()
            .any(|index| index == "projects_kind_idx"));
        assert!(index_names(&store)
            .iter()
            .any(|index| index == "projects_vcs_idx"));
        assert!(index_names(&store)
            .iter()
            .any(|index| index == "task_sessions_status_idx"));
        assert!(index_names(&store)
            .iter()
            .any(|index| index == "task_sessions_active_provider_idx"));
        assert!(index_names(&store)
            .iter()
            .any(|index| index == "route_decisions_selected_provider_idx"));
        assert!(index_names(&store)
            .iter()
            .any(|index| index == "route_decisions_task_type_idx"));
        assert!(index_names(&store)
            .iter()
            .any(|index| index == "route_decisions_mode_idx"));
        assert!(index_names(&store)
            .iter()
            .any(|index| index == "handoffs_status_idx"));
        assert!(index_names(&store)
            .iter()
            .any(|index| index == "handoffs_to_provider_idx"));
    }

    #[test]
    fn pagination_limits_and_offsets_results() {
        let temp = tempfile::tempdir().expect("temp dir");
        let store = EventStore::open(temp.path().join("baize.db")).expect("store should open");
        let session_id = TaskSessionId::new();
        for i in 0..5 {
            let mut event = BaizeEvent::new("test.event", json!({ "i": i }));
            event.session_id = Some(session_id.clone());
            store.append_event(&event).expect("append");
        }

        let all = store
            .list_events_for_session(&session_id, None, None)
            .expect("all");
        assert_eq!(all.len(), 5);

        let limited = store
            .list_events_for_session(&session_id, Some(2), None)
            .expect("limited");
        assert_eq!(limited.len(), 2);

        let offset = store
            .list_events_for_session(&session_id, None, Some(3))
            .expect("offset");
        assert_eq!(offset.len(), 2);

        let page = store
            .list_events_for_session(&session_id, Some(2), Some(2))
            .expect("page");
        assert_eq!(page.len(), 2);
        assert_eq!(page[0].payload["i"], 2);
        assert_eq!(page[1].payload["i"], 3);
    }

    #[test]
    fn get_latest_session_for_workspace_returns_most_recent() {
        let temp = tempfile::tempdir().expect("temp dir");
        let store = EventStore::open(temp.path().join("baize.db")).expect("store should open");
        let ws_a = WorkspaceId::new();
        let ws_b = WorkspaceId::new();
        let now = Utc::now();

        let session_a1 = TaskSession {
            id: TaskSessionId::new(),
            workspace_id: ws_a.clone(),
            objective: "first in A".to_string(),
            active_provider_id: Some(ProviderId("codex".to_string())),
            status: TaskSessionStatus::Running,
            created_at: now,
            updated_at: now,
        };
        let session_a2 = TaskSession {
            id: TaskSessionId::new(),
            workspace_id: ws_a.clone(),
            objective: "second in A".to_string(),
            active_provider_id: Some(ProviderId("gemini".to_string())),
            status: TaskSessionStatus::Running,
            created_at: now,
            updated_at: now,
        };
        let session_b1 = TaskSession {
            id: TaskSessionId::new(),
            workspace_id: ws_b.clone(),
            objective: "first in B".to_string(),
            active_provider_id: Some(ProviderId("opencode".to_string())),
            status: TaskSessionStatus::Running,
            created_at: now,
            updated_at: now,
        };

        store.upsert_task_session(&session_a1).expect("a1");
        store.upsert_task_session(&session_a2).expect("a2");
        store.upsert_task_session(&session_b1).expect("b1");

        let latest_a = store
            .get_latest_session_for_workspace(&ws_a)
            .expect("latest a")
            .expect("some");
        assert_eq!(latest_a.objective, "second in A");
        assert_eq!(latest_a.active_provider_id.unwrap().0, "gemini");

        let latest_b = store
            .get_latest_session_for_workspace(&ws_b)
            .expect("latest b")
            .expect("some");
        assert_eq!(latest_b.objective, "first in B");

        let latest_empty = store
            .get_latest_session_for_workspace(&WorkspaceId::new())
            .expect("query");
        assert!(latest_empty.is_none());
    }
}
