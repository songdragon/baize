# MVP Implementation Plan

Status: implemented and actively hardening

## Scope

This MVP implements the review-passed technical spec as a local Rust daemon plus TUI shell. Codex, Antigravity and OpenCode prompt execution paths are wired through CLI adapters; Gemini CLI is retained only as a legacy diagnostic profile. Tests use fake executors and parser fixtures so CI does not spend model quota.

The MVP target is a single-workspace local agent supervisor:

- inspect and register the current project;
- create and resume task sessions;
- route work to configured coding-agent providers;
- execute prompt requests through Codex/Antigravity/OpenCode CLI paths;
- record session events, route decisions, handoffs and permissions;
- let the user operate the workflow from a TUI;
- keep the kernel API reusable for a future desktop app.

## Implemented Work

### 1. Core Model

- Workspace and project identity;
- task session identity and lifecycle state;
- provider identity, priority, transport and capabilities;
- permission command risk assessment model;
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
- project lookup by canonical root for idempotent workspace registration;
- session event lookup;
- route decision lookup by session;
- permission insert, list and detail lookup;
- handoff insert/update and detail lookup.

### 3. Workspace

- local path inspection;
- git root detection;
- branch detection;
- dirty state and changed files;
- git diff hunk parsing for tracked file changes;
- clean git repository inspection;
- dirty git repository inspection with tracked and untracked files;
- fixed `git status --porcelain` parsing so changed file names do not lose the first character.

### 4. Provider Validation And Adapters

- default provider order: Codex, Antigravity, OpenCode, Copilot;
- provider transport registry;
- health probing via provider command `--version`;
- ACP transport metadata for Copilot and OpenCode;
- ACP initialize proof generation for ACP transports;
- structured validation for Codex/OpenCode/Copilot, Antigravity CLI validation, plus legacy Gemini diagnostics;
- detected capabilities and capability gap reporting;
- daemon endpoints for provider validation;
- Codex `exec --json` execution path;
- Antigravity `/Users/songdragon/.local/bin/agy --print <prompt>` execution path;
- OpenCode `run --format json` execution path;
- workspace command policy mapping for Codex/Antigravity/OpenCode execution arguments;
- Codex/Antigravity/OpenCode smoke validation command for auth, timeout and parser checks;
- stream-json/JSONL parser behavior;
- native provider session ID extraction from structured output;
- native provider session ID persistence and reuse for same-provider follow-up prompts;
- provider-specific resume argument generation for Codex, Antigravity and OpenCode follow-up prompts;
- structured provider error classification and daemon reporting;
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
- `baize doctor` readiness summary with `coding_ready`, ready prompt providers and runtime policy;
- `baize providers`;
- `baize validate [provider]`;
- `baize smoke <provider>` with gated real-prompt execution;
- `baize ask` for daemon-backed non-TUI prompt execution;
- refactored CLI command handling into testable output/action functions.

### 6. Documentation

- MVP quickstart for TUI usage with `BAIZE_DATA_DIR`;
- provider CLI setup notes for Codex, Antigravity and OpenCode;
- TUI keyboard shortcut reference;
- local HTTP API examples with curl;
- test, lint and coverage command reference.

### 7. Daemon API

- `GET /health`;
- `GET /runtime/status`;
- `GET /providers`;
- `GET /providers/:id/health`;
- `GET /providers/:id/validate`;
- `POST /providers/check`;
- `POST /providers/validate`;
- `GET /workspaces` with optional `name` and `primary_project_id` filters;
- `POST /workspaces`;
- `GET /workspaces/:id/projects` with optional `kind` and `vcs` filters;
- `GET /projects/:id`;
- `GET /workspaces/:id/status`;
- `GET /workspaces/status?path=...`;
- `GET /sessions`;
- `POST /sessions`;
- `GET /sessions/:id`;
- `POST /sessions/:id/prompt`;
- `POST /sessions/:id/cancel`;
- `POST /sessions/:id/complete`;
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
- `GET /events` (SSE stream);
- `GET /events/history`.
- Session status transitions: `Running` stays on prompt success, transitions to `Failed` on prompt failure or executor error, recovers from `Failed` on next successful prompt.
- Canceled sessions reject new prompt requests.
- Completed sessions can be reopened by a follow-up prompt.
- Late provider results are ignored when a session was canceled before completion.
- Unsupported prompt provider overrides are rejected before mutating session route state.
- `session.status.changed` event emission on status transitions.
- Startup recovery marks in-flight `Running` sessions as `Failed` and emits `session.recovered`.
- Session diff API includes changed files and tracked-file diff hunks.

### 8. Routing

- assisted-mode default route decision;
- configured provider priority selection;
- requested provider override;
- route decision persistence;
- route decision event emission;
- task-type hints on route decisions;
- configurable sticky routing window;
- provider/runtime failure threshold skip for sticky and priority routing;
- route history API;
- TUI display of recent route history.

### 9. Handoff

- markdown handoff artifact generation;
- Baize mechanical facts attachment;
- changed files and user constraints capture;
- commands, test signals, route history and provider errors capture from session events;
- handoff persistence and event emission;
- handoff markdown artifact persistence;
- before-handoff checkpoint references in handoff facts;
- handoff accept flow that updates active provider and emits route decision;
- TUI handoff preview before accept;
- TUI pending handoff status line;
- `Ctrl-H` to create preview;
- `Ctrl-Y` to accept pending handoff;
- pending handoff is cleared when loading, starting or canceling sessions.

### 10. Permission

- permission request creation;
- command risk assessment for permission requests;
- approve/deny resolution;
- permission persistence and event emission;
- list permissions;
- filter permissions by status;
- filter permissions by session ID;
- fetch permission detail by ID;
- TUI pending permission status line;
- TUI permission risk display when daemon provides risk data;
- `Ctrl-P` to refresh pending permissions;
- `Up`/`Down` to select pending permission;
- `Ctrl-A` to approve selected permission;
- `Ctrl-D` to deny selected permission.

### 11. TUI

- ratatui shell;
- workspace/session/status panels;
- daemon auto-start;
- daemon connection status display;
- provider list loaded from daemon config;
- provider health display;
- `Ctrl-R` provider health refresh;
- prompt input and `Enter` submit;
- non-blocking prompt worker so the TUI remains responsive while agent prompts run;
- cancel flow detaches any pending prompt worker so late results do not update the TUI;
- selected provider switching with `Tab`;
- latest session loading with `Ctrl-L`;
- new session reset with `Ctrl-N`;
- current session cancel with `Ctrl-X`;
- current session complete with `Ctrl-E`;
- recent session selection with `F2`/`F3` and selected-session loading with `F4`;
- activity status line;
- provider status line;
- route status line;
- permission status line;
- handoff status line;
- explicit selected provider, permission and handoff markers;
- handoff preview detail with markdown summary and mechanical facts;
- provider authentication, timeout and limit hints in prompt failures;
- command/tool output and test result sections in the transcript;
- workspace diff display after prompt/handoff;
- recent session event display;
- recent route history display;
- recent session list display;
- compact keyboard help line.
- runtime policy display;
- full assistant output preservation with code indentation;
- default latest-output transcript viewport with PageUp/PageDown/Home/End and mouse-wheel scroll.

## Test Coverage

Current full test count: 234.

Implemented test coverage includes:

- core ID and event construction;
- core permission command risk assessment;
- ACP JSON-RPC request construction;
- config defaults, TOML parsing, initialization and validation;
- CLI action planning and output formatting;
- CLI smoke command output formatting;
- CLI ask command mapping and output summary formatting;
- storage event append/count/session lookup;
- storage workspace/project/session persistence;
- storage project lookup by root for idempotent workspace registration;
- storage query indexes for high-volume session/workspace lookups;
- storage workspace name/primary-project query columns and indexes;
- storage project root/kind/vcs query columns and indexes;
- storage route decision provider/task/mode query columns and indexes;
- storage task session status/provider query columns and indexes;
- storage handoff status/provider query columns and indexes;
- storage permission risk-level query column and index;
- storage route decision and permission lookup;
- storage handoff artifact file writing;
- workspace inspection for plain directories;
- workspace inspection for clean and dirty git repositories;
- workspace diff hunk parsing and git extraction;
- provider priority and ACP transport metadata;
- provider ACP initialize proof generation;
- provider validation behavior;
- Codex/Antigravity/OpenCode command construction;
- Codex/Antigravity/OpenCode execution policy argument mapping;
- Codex/Antigravity/OpenCode resume argument generation where supported;
- Codex/Antigravity/OpenCode smoke validation without real prompt execution;
- stream-json/JSONL parser behavior;
- adapter native provider session ID extraction;
- adapter provider error classification;
- command timeout behavior;
- adapter terminal success detection for stream-json output that completes before CLI process exit;
- adapter timeout handling that treats an already-emitted terminal success result as a successful prompt turn;
- daemon workspace/session/prompt/events flow;
- daemon propagation of workspace command policy into adapter prompt requests;
- daemon idempotent workspace registration by project root;
- daemon workspace project listing;
- daemon event history filtering;
- daemon session diff hunk reporting;
- daemon prompt native provider session ID reporting;
- daemon native provider session ID persistence and resume request propagation;
- daemon prompt failure error chain;
- daemon prompt failure structured provider error reporting;
- daemon provider ordering and provider health ordering;
- daemon task-type inference for route decisions;
- daemon route decision provider/task/mode filtering;
- daemon session status/provider/workspace filtering;
- daemon configurable sticky routing policy;
- daemon provider/runtime failure threshold routing skip;
- daemon handoff creation and accept flow;
- daemon handoff fact extraction from session events and routes;
- daemon handoff status/provider filtering;
- daemon handoff artifact path response and event payload;
- daemon checkpoint policy handling for handoff facts;
- daemon permission listing/filtering/detail lookup;
- daemon permission command risk reporting;
- daemon permission risk-level filtering;
- daemon session status transitions (Running, Failed, Canceled, recovery);
- daemon explicit Completed status and prompt reopening behavior;
- daemon startup recovery for in-flight sessions;
- daemon canceled session prompt rejection;
- daemon ignored-result guard for prompts that complete after session cancellation;
- daemon guard that prevents unsupported prompt provider overrides from changing session state;
- TUI dashboard rendering;
- TUI prompt input rendering;
- TUI provider, route, permission and handoff status formatting;
- TUI selected provider, permission and handoff markers;
- TUI provider error hints for authentication, timeout and inferred quota/rate limits;
- TUI latest session state loading;
- TUI selected recent-session cycling and loading;
- TUI recent session list parsing and display;
- TUI new session reset behavior;
- TUI session cancel state updates;
- TUI session complete state updates;
- TUI handoff preview and accept guard behavior;
- TUI handoff preview detail rendering;
- TUI permission selection and resolution display;
- TUI permission risk parsing and status display;
- TUI command/tool event and test result sections;
- TUI workspace diff and route history display.
- TUI prompt worker completion/error polling.
- TUI prompt worker detachment after cancellation.
- TUI runtime policy rendering and fallback behavior.
- TUI Codex assistant-message parsing, full assistant output preservation and transcript scrolling.
- TUI transcript section rendering and immediate per-turn target/policy/thinking feedback.
- TUI auto-started daemon ownership marker for shutdown lifecycle.

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
- ~~add a handoff preview detail view with more than the first few markdown lines~~ (done);
- ~~add clearer command result/test result sections~~ (done);
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

- ~~Validate real Codex CLI execution end to end with authentication, timeout and JSON parsing~~ (done as `baize smoke codex`, with real prompt gated by `--run-prompt`);
- ~~add Antigravity provider profile and headless CLI command construction~~ (done as `baize smoke antigravity`; real prompt is gated by `--run-prompt`);
- ~~disable Gemini CLI as a default prompt runtime after the Antigravity migration requirement~~ (done; legacy validation remains available);
- ~~add OpenCode CLI prompt runtime beyond ACP metadata~~ (done with `opencode run --format json`; real prompt is gated by `baize smoke opencode --run-prompt`);
- ~~preserve provider-native session/resume IDs when available~~ (done for structured output capture);
- ~~expose adapter stderr and provider errors in a more structured form~~ (done);
- ~~add Copilot/OpenCode ACP proof-of-life beyond metadata~~ (done with initialize proof generation).

### 5. Persistence And Recovery

- ~~Add migration version tracking for SQLite schema~~ (done);
- ~~add query tables or indexes for higher-volume event lookup~~ (done);
- ~~persist transcript/handoff artifacts as files when useful~~ (done for handoff artifacts);
- ~~add crash recovery semantics for in-flight agent runs~~ (done);
- ~~add checkpoint references for before-handoff policy~~ (done).

### 6. Documentation

- ~~Write a quickstart for running `baize tui` with local data directory~~ (done);
- ~~document required provider CLI setup for Codex, Antigravity and OpenCode~~ (done);
- ~~document current keyboard shortcuts~~ (done);
- ~~document local API examples with curl~~ (done);
- ~~document test and coverage commands~~ (done).

## Post-MVP / Out Of MVP

- ACP session lifecycle implementation beyond message primitives;
- exact quota extraction from providers where APIs allow it;
- multi-workspace TUI switching;
- desktop app shell;
- final relational schema hardening;
- ~~workspace name/primary-project query columns/indexes~~ (done as one schema hardening step);
- ~~project root/kind/vcs query columns/indexes~~ (done as one schema hardening step);
- ~~route decision provider/task/mode query columns/indexes~~ (done as one schema hardening step);
- ~~handoff status/provider query columns/indexes~~ (done as one schema hardening step);
- ~~task session status/provider query columns/indexes~~ (done as one schema hardening step);
- ~~permission risk-level query column/index~~ (done as one schema hardening step);
- ~~hunk attribution~~ (done for tracked-file diff hunk extraction);
- ~~full command permission sandbox~~ (done as first-pass command risk assessment and surfacing);
- multi-agent concurrent execution;
- cloud sync or team collaboration.
