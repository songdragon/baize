# Baize

Baize is a persistent development session that can switch coding agents. The user keeps one conversation and workspace while Baize combines current workspace facts, relevant recent dialogue, and still-active constraints to let another agent continue the work.

An agent is its execution environment, including its built-in prompts, MCP servers, skills, computer use, and browser use where available. Baize aims to preserve those native capabilities through supported interfaces, including provider Desktop execution environments; CLI is the initial implementation, not the product boundary. Launching a Desktop app or calling a related CLI alone does not establish access to its capabilities.

Shared configuration reduces repeated setup and should reuse project instructions, MCP, and skills wherever possible. Session continuity is the core value; configuration, routing, quota observation, and handoff summaries support it. Switching does not require importing a provider's private session files or transferring its internal tool state.

See [the product vision](doc/vision.md), [requirements](doc/spec.md), and [technical design](doc/technical-spec.md). These describe target behavior; the current implementation is scoped below.

## Current implementation

The repository includes the persistent-session core and a terminal workbench for daily conversation, review and handoff:

- Rust workspace crates;
- core domain types;
- TOML config loader;
- SQLite append-only event log and MVP query tables;
- git workspace status inspection;
- ACP JSON-RPC message primitives;
- provider registry and health probing stubs;
- local daemon with HTTP + SSE workspace/session/handoff/permission APIs;
- Codex `exec --json <prompt>`, OpenCode `run --format json <prompt>`, and Antigravity `/Users/songdragon/.local/bin/agy --print <prompt> --output-format stream-json` execution paths;
- Ratatui workbench with workspace-scoped session search, chronological conversation recovery, a Unicode multiline editor, command palette and changes/context/handoff/permission/provider inspectors;
- `baize` CLI entrypoint.

The current implementation executes through CLI adapters and preserves Baize session identity and provider-native resume references. Provider Desktop execution, shared agent-configuration mapping, and end-to-end seamless switching are not yet implemented or verified. Baize's own future Desktop UI is separate from integrating a provider's Desktop execution environment.

CLI stdout/stderr and provider tool progress stream through the daemon into a folded process block. Final replies appear as normal prose with headings, emphasis and code. Press F9 to reveal the process, then Space to fold, arrows to select, Enter for full details and Esc to return to the composer. The client waits for its SSE subscription before submitting a prompt, reconnects without resubmitting work, and merges final snapshots without a local typing animation. See [streaming behavior and verification](doc/cli-streaming-output.md).

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

For TUI usage, provider setup, keyboard shortcuts, local API examples and test commands, see [doc/quickstart.md](doc/quickstart.md). The [workbench design](doc/tui-design.md) documents interaction and recovery behavior; [Rust UI research](doc/ui-research.md) records current framework choices and Desktop candidates.

The client reads the configured daemon address and leaves the daemon running when it exits. `Ctrl-X` cancels the Baize session but does not guarantee that the provider process stops. Unconfirmed runs must be reconciled with `F5` before continuing. Drafts are kept per session in client memory; exiting with unsent content requires confirmation.

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

The day-to-day adapter execution paths are Codex `exec --json <prompt>`, OpenCode `run --format json <prompt>`, and Antigravity `/Users/songdragon/.local/bin/agy --print <prompt> --output-format stream-json`. Gemini CLI is retained only as a legacy diagnostic profile because individual Code Assist accounts are now directed to Antigravity. Tests use fake executors and parser fixtures so they do not spend model quota.
