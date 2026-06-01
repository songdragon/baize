# MVP Implementation Plan

Status: implemented and actively hardening

## Scope

This MVP implements the review-passed technical spec as a local Rust daemon plus TUI shell. Gemini and Codex prompt execution paths are wired through CLI adapters; tests use fake executors and parser fixtures so CI does not spend model quota.

The MVP target is a single-workspace local agent supervisor:

- inspect and register the current project;
- create and resume task sessions;
- route work to configured coding-agent providers;
- execute prompt requests through Codex/Gemini CLI paths;
- record session events, route decisions, handoffs and permissions;
- let the user operate the workflow from a TUI;
- keep the kernel API reusable for a future desktop app.

## Implemented Work

### 1. Core Model

- Workspace and project identity;
- task session identity and lifecycle state;
- provider identity, priority, transport and capabilities;
- route decision model;
- handoff summary and mechanical facts;
- permission request and resolution model;
- event model for append-only logging and SSE;
- generated ID constructors for all MVP entities.

### 2. Storage

- SQLite append-only `events` table;
- MVP query tables for workspaces, projects, sessions, route decisions, handoffs and permissions;
- JSON-backed records for fast iteration before final relational schema hardening;
- workspace/project/session persistence;
- session event lookup;
- route decision lookup by session;
- permission insert, list and detail lookup;
- handoff insert/update and detail lookup.

### 3. Workspace

- local path inspection;
- git root detection;
- branch detection;
- dirty state and changed files;
- clean git repository inspection;
- dirty git repository inspection with tracked and untracked files;
- fixed `git status --porcelain` parsing so changed file names do not lose the first character.

### 4. Provider Validation And Adapters

- default provider order: Codex, Gemini, Copilot, OpenCode;
- provider transport registry;
- health probing via provider command `--version`;
- ACP transport metadata for Copilot and OpenCode;
- structured validation for Codex/Gemini/Copilot/OpenCode;
- detected capabilities and capability gap reporting;
- daemon endpoints for provider validation;
- Gemini `--prompt --output-format stream-json` execution path;
- Codex `exec --json` execution path;
- stream-json/JSONL parser behavior;
- prompt execution timeout to prevent hanging on provider authentication or interactive prompts.

### 5. Config And CLI

- default TOML config model;
- config loading with default fallback;
- config validation;
- configurable routing policy thresholds;
- `baize config path`;
- `baize config show`;
- `baize config init --force`;
- `baize config validate`;
- `baize status`;
- `baize doctor`;
- `baize providers`;
- `baize validate [provider]`;
- refactored CLI command handling into testable output/action functions.

### 6. Daemon API

- `GET /health`;
- `GET /providers`;
- `GET /providers/:id/health`;
- `GET /providers/:id/validate`;
- `POST /providers/check`;
- `POST /providers/validate`;
- `GET /workspaces`;
- `POST /workspaces`;
- `GET /workspaces/:id/status`;
- `GET /workspaces/status?path=...`;
- `GET /sessions`;
- `POST /sessions`;
- `GET /sessions/:id`;
- `POST /sessions/:id/prompt`;
- `POST /sessions/:id/cancel`;
- `GET /sessions/:id/routes`;
- `POST /sessions/:id/handoff`;
- `POST /sessions/:id/handoff/:handoff_id/accept`;
- `GET /sessions/:id/events`;
- `GET /sessions/:id/diff`;
- `GET /sessions/:id/handoff/:handoff_id`;
- `GET /permissions`;
- `POST /permissions`;
- `GET /permissions/:id`;
- `POST /permissions/:id/approve`;
- `POST /permissions/:id/deny`;
- `GET /events`.
- Session status transitions: `Running` stays on prompt success, transitions to `Failed` on prompt failure or executor error, recovers from `Failed` on next successful prompt.
- Canceled sessions reject new prompt requests.
- `session.status.changed` event emission on status transitions.

### 7. Routing

- assisted-mode default route decision;
- configured provider priority selection;
- requested provider override;
- route decision persistence;
- route decision event emission;
- task-type hints on route decisions;
- configurable sticky routing window;
- route history API;
- TUI display of recent route history.

### 8. Handoff

- markdown handoff artifact generation;
- Baize mechanical facts attachment;
- changed files and user constraints capture;
- handoff persistence and event emission;
- handoff accept flow that updates active provider and emits route decision;
- TUI handoff preview before accept;
- TUI pending handoff status line;
- `Ctrl-H` to create preview;
- `Ctrl-Y` to accept pending handoff;
- pending handoff is cleared when loading, starting or canceling sessions.

### 9. Permission

- permission request creation;
- approve/deny resolution;
- permission persistence and event emission;
- list permissions;
- filter permissions by status;
- filter permissions by session ID;
- fetch permission detail by ID;
- TUI pending permission status line;
- `Ctrl-P` to refresh pending permissions;
- `Up`/`Down` to select pending permission;
- `Ctrl-A` to approve selected permission;
- `Ctrl-D` to deny selected permission.

### 10. TUI

- ratatui shell;
- workspace/session/status panels;
- daemon auto-start;
- daemon connection status display;
- provider list loaded from daemon config;
- provider health display;
- `Ctrl-R` provider health refresh;
- prompt input and `Enter` submit;
- selected provider switching with `Tab`;
- latest session loading with `Ctrl-L`;
- new session reset with `Ctrl-N`;
- current session cancel with `Ctrl-X`;
- activity status line;
- provider status line;
- route status line;
- permission status line;
- handoff status line;
- explicit selected provider, permission and handoff markers;
- provider authentication, timeout and limit hints in prompt failures;
- workspace diff display after prompt/handoff;
- recent session event display;
- recent route history display;
- recent session list display;
- compact keyboard help line.

## Test Coverage

Current full test count: 109.

Implemented test coverage includes:

- core ID and event construction;
- ACP JSON-RPC request construction;
- config defaults, TOML parsing, initialization and validation;
- CLI action planning and output formatting;
- storage event append/count/session lookup;
- storage workspace/project/session persistence;
- storage route decision and permission lookup;
- workspace inspection for plain directories;
- workspace inspection for clean and dirty git repositories;
- provider priority and ACP transport metadata;
- provider validation behavior;
- Gemini/Codex command construction;
- stream-json/JSONL parser behavior;
- command timeout behavior;
- daemon workspace/session/prompt/events flow;
- daemon prompt failure error chain;
- daemon provider ordering and provider health ordering;
- daemon task-type inference for route decisions;
- daemon configurable sticky routing policy;
- daemon handoff creation and accept flow;
- daemon permission listing/filtering/detail lookup;
- daemon session status transitions (Running, Failed, Canceled, recovery);
- daemon canceled session prompt rejection;
- TUI dashboard rendering;
- TUI prompt input rendering;
- TUI provider, route, permission and handoff status formatting;
- TUI selected provider, permission and handoff markers;
- TUI provider error hints for authentication, timeout and inferred quota/rate limits;
- TUI latest session state loading;
- TUI recent session list parsing and display;
- TUI new session reset behavior;
- TUI session cancel state updates;
- TUI handoff preview and accept guard behavior;
- TUI permission selection and resolution display;
- TUI workspace diff and route history display.

Last measured coverage snapshot:

```text
TOTAL line coverage:     77.02%
TOTAL function coverage: 70.17%
TOTAL region coverage:   73.20%
```

Note: the coverage snapshot was measured before the latest small TUI/API additions. Re-run `cargo llvm-cov --workspace --summary-only` for the current exact number.

## Remaining MVP TODO

These are still in scope for a more usable MVP.

### 1. TUI Usability

- ~~Add a scrollback model for the session transcript instead of only keeping the last 30 lines;~~ (done)
- ~~add explicit visual selection for provider, permission and handoff states~~ (done);
- ~~add a session list view instead of only `Ctrl-L` latest-session loading~~ (done);
- add a handoff preview detail view with more than the first few markdown lines;
- add clearer command result/test result sections;
- ~~add better error presentation for provider authentication failures and timeouts~~ (done).

### 2. Daemon And API

- ~~Add structured `GET /sessions/:id/handoffs` list endpoint~~ (done);
- ~~add structured `GET /sessions/:id/permissions` convenience endpoint or document query usage~~ (done);
- ~~add session status transitions for completed/failed prompt runs~~ (done);
- ~~add explicit canceled-state behavior for future prompt requests~~ (done);
- ~~add pagination or limit parameters for events, sessions and permissions~~ (done);
- ~~add API-level status codes instead of returning every error as HTTP 200 JSON~~ (done);

### 3. Routing

- ~~Add sticky routing window~~ (done);
- ~~include provider health in route selection~~ (done);
- ~~include explicit user-selected provider override reason~~ (done);
- ~~add task-type hints for route decisions~~ (done);
- ~~add first quota/rate-limit inference from provider errors~~ (done);
- ~~add configurable routing policy thresholds~~ (done).

### 4. Adapter Runtime

- Validate real Codex CLI execution end to end with authentication, timeout and JSON parsing;
- validate real Gemini CLI execution end to end with authentication, timeout and stream-json parsing;
- preserve provider-native session/resume IDs when available;
- expose adapter stderr and provider errors in a more structured form;
- add Copilot/OpenCode ACP proof-of-life beyond metadata.

### 5. Persistence And Recovery

- ~~Add migration version tracking for SQLite schema~~ (done);
- add query tables or indexes for higher-volume event lookup;
- persist transcript/handoff artifacts as files when useful;
- add crash recovery semantics for in-flight agent runs;
- add checkpoint references for before-handoff policy.

### 6. Documentation

- Write a quickstart for running `baize tui` with local data directory;
- document required provider CLI setup for Codex and Gemini;
- document current keyboard shortcuts;
- document local API examples with curl;
- document test and coverage commands.

## Post-MVP / Out Of MVP

- ACP session lifecycle implementation beyond message primitives;
- exact quota extraction from providers where APIs allow it;
- multi-workspace TUI switching;
- desktop app shell;
- final relational schema hardening;
- hunk attribution;
- full command permission sandbox;
- multi-agent concurrent execution;
- cloud sync or team collaboration.
