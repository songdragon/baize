# Baize

Baize is a workspace-native supervisor for local coding agents. It is designed to route work across agents such as Codex, Antigravity, OpenCode, and GitHub Copilot CLI while preserving workspace state and handoff context.

## Current MVP

This repository currently contains the first Rust MVP skeleton:

- Rust workspace crates;
- core domain types;
- TOML config loader;
- SQLite append-only event log and MVP query tables;
- git workspace status inspection;
- ACP JSON-RPC message primitives;
- provider registry and health probing stubs;
- local daemon with HTTP + SSE workspace/session/handoff/permission APIs;
- Codex `exec --json`, OpenCode `run --format json`, and Antigravity `/Users/songdragon/.local/bin/agy --print <prompt>` execution paths;
- ratatui TUI shell;
- `baize` CLI entrypoint.

## Commands

Once Rust is installed:

```sh
cargo run -p baize-cli -- status .
cargo run -p baize-cli -- providers
cargo run -p baize-cli -- doctor
cargo run -p baize-cli -- validate
cargo run -p baize-cli -- validate antigravity
cargo run -p baize-cli -- daemon
cargo run -p baize-cli -- tui
cargo run -p baize-cli -- ask --provider codex "summarize this project"
```

For local sandboxed development, keep Baize data inside the repository:

```sh
BAIZE_DATA_DIR=.baize/data cargo run -p baize-cli -- daemon
```

For TUI usage, provider setup, keyboard shortcuts, local API examples and test commands, see [doc/quickstart.md](doc/quickstart.md).

Useful daemon endpoints:

```text
GET  /health
GET  /providers
GET  /providers/:id/validate
POST /providers/check
POST /providers/validate
GET  /workspaces
POST /workspaces
GET  /workspaces/:id/status
GET  /sessions
POST /sessions
GET  /sessions/:id
POST /sessions/:id/prompt
POST /sessions/:id/cancel
POST /sessions/:id/complete
POST /sessions/:id/handoff
GET  /sessions/:id/events
GET  /sessions/:id/diff
GET  /permissions
POST /permissions
GET  /permissions/:id
POST /permissions/:id/approve
POST /permissions/:id/deny
GET  /events
```

`POST /sessions/:id/prompt` accepts an optional `timeout_seconds` field. Use a short timeout for smoke tests when a provider might be waiting for authentication.

## Notes

The day-to-day adapter execution paths are Codex `exec --json`, OpenCode `run --format json`, and Antigravity `/Users/songdragon/.local/bin/agy --print <prompt>`. Gemini CLI is retained only as a legacy diagnostic profile because individual Code Assist accounts are now directed to Antigravity. Tests use fake executors and parser fixtures so they do not spend model quota.
