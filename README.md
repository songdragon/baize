# Baize

Baize is a workspace-native supervisor for local coding agents. It is designed to route work across agents such as Codex, Gemini CLI, GitHub Copilot CLI, and OpenCode while preserving workspace state and handoff context.

## Current MVP

This repository currently contains the first Rust MVP skeleton:

- Rust workspace crates;
- core domain types;
- TOML config loader;
- SQLite append-only event log;
- git workspace status inspection;
- ACP JSON-RPC message primitives;
- provider registry and health probing stubs;
- local daemon with HTTP + SSE basics;
- ratatui TUI shell;
- `baize` CLI entrypoint.

## Commands

Once Rust is installed:

```sh
cargo run -p baize-cli -- status .
cargo run -p baize-cli -- providers
cargo run -p baize-cli -- doctor
cargo run -p baize-cli -- daemon
cargo run -p baize-cli -- tui
```

For local sandboxed development, keep Baize data inside the repository:

```sh
BAIZE_DATA_DIR=.baize/data cargo run -p baize-cli -- daemon
```

## Notes

The first adapter validation path is Codex/Gemini. If their ACP or session-control capability is insufficient, the adapter layer will record the gap and fall back to native/server/CLI integration.
