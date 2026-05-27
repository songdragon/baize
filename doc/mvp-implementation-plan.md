# MVP Implementation Plan

Status: implemented

## Scope

This MVP implements the review-passed technical spec as a local Rust daemon plus TUI shell. Real agent execution is intentionally not enabled yet; prompt handling records events and returns an accepted MVP response while adapter validation focuses on provider discovery and health.

## Implemented Work

### 1. Core Model

- Workspace and project identity;
- task session identity and lifecycle state;
- provider identity, priority, transport and capabilities;
- route decision model;
- handoff summary and mechanical facts;
- permission request and resolution model;
- event model for append-only logging and SSE.

### 2. Storage

- SQLite append-only `events` table;
- MVP query tables for workspaces, projects, sessions, route decisions, handoffs and permissions;
- JSON-backed records for fast iteration before final relational schema hardening;
- session event lookup.

### 3. Workspace

- local path inspection;
- git root detection;
- branch detection;
- dirty state and changed files.

### 4. Provider Validation

- default provider order: Codex, Gemini, Copilot, OpenCode;
- provider transport registry;
- health probing via provider command `--version`;
- ACP transport metadata for Copilot and OpenCode.

### 5. Daemon API

- `GET /health`;
- `GET /providers`;
- `GET /providers/:id/health`;
- `POST /providers/check`;
- `GET /workspaces`;
- `POST /workspaces`;
- `GET /workspaces/:id/status`;
- `GET /workspaces/status?path=...`;
- `GET /sessions`;
- `POST /sessions`;
- `GET /sessions/:id`;
- `POST /sessions/:id/prompt`;
- `POST /sessions/:id/cancel`;
- `POST /sessions/:id/handoff`;
- `GET /sessions/:id/events`;
- `GET /sessions/:id/diff`;
- `GET /sessions/:id/handoff/:handoff_id`;
- `POST /permissions`;
- `POST /permissions/:id/approve`;
- `POST /permissions/:id/deny`;
- `GET /events`.

### 6. Routing

- assisted-mode default route decision;
- configured provider priority selection;
- requested provider override;
- route decision persistence;
- route decision event emission.

### 7. Handoff

- markdown handoff artifact generation;
- Baize mechanical facts attachment;
- changed files and user constraints capture;
- handoff persistence and event emission.

### 8. Permission

- permission request creation;
- approve/deny resolution;
- permission persistence and event emission.

### 9. TUI

- ratatui shell;
- workspace/session/status panels;
- provider status text;
- render function covered by unit test.

## Test Coverage

- core ID and event construction;
- ACP JSON-RPC request construction;
- config defaults and TOML parsing;
- storage event append/count/session lookup;
- storage workspace/project/session persistence;
- workspace inspection for non-git directories;
- adapter provider priority and ACP transport metadata;
- daemon workspace/session/prompt/events flow;
- daemon handoff creation;
- TUI dashboard rendering.

## Out Of MVP

- real provider prompt execution;
- ACP session lifecycle implementation beyond message primitives;
- quota extraction from providers;
- multi-workspace TUI switching;
- desktop app;
- final relational schema hardening;
- hunk attribution.
