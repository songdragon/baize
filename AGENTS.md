# AGENTS.md

Guide for AI agents working on the Baize codebase.

## Project Overview

Baize is a workspace-native supervisor for local coding agents. It routes work across providers (Codex, Antigravity, OpenCode, GitHub Copilot CLI, with Gemini CLI retained only as a legacy diagnostic profile) while preserving workspace state and handoff context. The architecture is a headless daemon with HTTP+SSE APIs plus a thin TUI client.

- **Language**: Rust (stable, edition 2021)
- **Workspace**: 9 crates under `crates/`, resolver = "2"
- **Version**: 0.1.0 (MVP, actively hardening)

## Build & Run Commands

```sh
cargo build --workspace                          # build all crates
cargo build -p baize-cli                         # build binary only
cargo run -p baize-cli -- <subcommand>           # run CLI
BAIZE_DATA_DIR=.baize/data cargo run -p baize-cli -- daemon  # local sandbox daemon
```

## Test Commands

```sh
cargo test --workspace                           # run all 72 tests
cargo test -p <crate>                            # test single crate
cargo llvm-cov --workspace --summary-only        # coverage report
```

## Lint & Format

```sh
cargo fmt --all -- --check                       # check formatting
cargo clippy --workspace --all-targets           # lint
```

**Always run both `cargo fmt` and `cargo clippy` before considering work done.**

## Crate Map

| Crate | Purpose | Internal Deps |
|---|---|---|
| `baize-core` | Domain types, IDs, enums, event model | (leaf) |
| `baize-config` | TOML config loading, validation, defaults | (leaf) |
| `baize-acp` | ACP JSON-RPC message primitives | (leaf) |
| `baize-workspace` | Git status inspection (root, branch, dirty, changed files) | (leaf) |
| `baize-storage` | SQLite append-only event log, query tables | baize-core |
| `baize-adapters` | Provider profiles, health probing, prompt execution, stream parsing | baize-core |
| `baize-daemon` | HTTP+SSE API, session orchestration, routing, handoff, permissions | baize-core, baize-config, baize-storage, baize-adapters, baize-workspace |
| `baize-tui` | ratatui terminal UI, daemon client | (none at lib level) |
| `baize-cli` | CLI entrypoint (`baize` binary), command dispatch | baize-config, baize-daemon, baize-tui, baize-adapters, baize-workspace |

## Architecture

```
CLI (baize-cli)
 ├── TUI (baize-tui) ──HTTP──► Daemon (baize-daemon)
 └── direct commands              ├── Adapters (baize-adapters)
                                  ├── Storage (baize-storage) ► SQLite
                                  ├── Config (baize-config)
                                  └── Workspace (baize-workspace) ► git
```

- The daemon holds all business state and exposes HTTP + SSE APIs.
- The TUI is a thin client that talks to the daemon via raw TCP HTTP.
- UI never directly calls agents; all agent interaction goes through the daemon.

## Code Conventions

### Module Structure

Each crate is currently a single-file library (`src/lib.rs`). When a crate grows beyond ~500 lines, split into `src/{module}.rs` files with `mod module;` in `lib.rs`. Follow the existing pattern of keeping public types at the top, implementations in the middle, and `#[cfg(test)] mod tests` at the bottom.

### ID Types

All entity IDs use prefixed UUID v4 newtypes:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WorkspaceId(pub String);

impl WorkspaceId {
    pub fn new() -> Self {
        Self(format!("ws_{}", Uuid::new_v4()))
    }
}
```

Prefixes: `ws_`, `prj_`, `task_`, `evt_`, `route_`, `handoff_`, `perm_`. Exception: `ProviderId` uses the provider name string directly (e.g. `"codex"`).

### Error Handling

- Use `anyhow::Result<T>` as the primary error type for all fallible functions.
- Add context with `.with_context(|| format!(...))` or `.context(...)`.
- Use `anyhow::bail!()` for validation failures and impossible states.
- `thiserror` is available in workspace dependencies but not yet adopted; prefer it for library crate error types that callers need to match on.
- Daemon uses `format_error_chain()` to walk error sources for API responses.

### Serialization

All domain types derive `Serialize` and `Deserialize`. Storage uses JSON-in-SQLite pattern (serialize whole structs as JSON text in SQLite columns) for fast iteration before relational schema hardening.

### Testing Patterns

- Tests are inline: `#[cfg(test)] mod tests { ... }` at the bottom of each file.
- Use `tempfile::tempdir()` for file system and SQLite isolation.
- Daemon tests use `FakeAgentExecutor` / `FailingAgentExecutor` trait objects to avoid real provider calls.
- TUI tests use `ratatui::backend::TestBackend` for rendering assertions.
- Daemon HTTP tests use `tower::ServiceExt` for integration testing without binding a port.
- Never write tests that spend model quota.

### Daemon API Handlers

- HTTP handlers return `Json<serde_json::Value>`.
- Use the `with_store()` helper for Mutex-guarded storage access.
- Use `json_result()` helper to wrap `Result<T>` into JSON responses.
- Current MVP returns errors as HTTP 200 with `{"error": "..."}` — this is a known TODO to use proper status codes.

### TUI Conventions

- Built on ratatui + crossterm.
- Key bindings: `Ctrl-R` (refresh providers), `Ctrl-L` (load latest session), `Ctrl-N` (new session), `Ctrl-X` (cancel session), `Ctrl-H` (handoff preview), `Ctrl-Y` (accept handoff), `Ctrl-P` (refresh permissions), `Ctrl-A` (approve), `Ctrl-D` (deny), `Tab` (switch provider).

### CLI Conventions

- CLI uses clap with `#[derive(Parser)]` and `#[derive(Subcommand)]`.
- Command handling is split into `plan_cli_action()` (returns `CliAction` enum) and output functions that return `Result<String>`, making them independently testable.
- Default subcommand is `tui`.

## Key Dependencies

| Dependency | Version | Purpose |
|---|---|---|
| `anyhow` | 1.0.95 | Error handling |
| `axum` | 0.7.9 | HTTP framework |
| `chrono` | 0.4.39 | Timestamps (with serde) |
| `clap` | 4.5.26 | CLI parsing (with derive) |
| `crossterm` | 0.28.1 | Terminal backend |
| `ratatui` | 0.29.0 | TUI framework |
| `rusqlite` | 0.32.1 | SQLite (bundled) |
| `serde` / `serde_json` | 1.0.217 / 1.0.135 | Serialization |
| `tokio` | 1.43.0 | Async runtime (full features) |
| `tower-http` | 0.6.2 | CORS middleware |
| `tracing` | 0.1.41 | Structured logging |
| `uuid` | 1.12.0 | ID generation (v4, serde) |

All dependency versions are pinned in `[workspace.dependencies]`. Add new deps there first, then reference with `workspace = true` in crate Cargo.toml files.

## Configuration

- Format: TOML at `~/.config/baize/config.toml`
- Override data directory: `BAIZE_DATA_DIR` env var
- SQLite database: `{data_dir}/baize.db`

## Known MVP Gaps (from doc/mvp-implementation-plan.md)

These items are in scope for the current MVP but not yet implemented:

- TUI: scrollback, session list view, handoff detail view, better error display
- Daemon: proper HTTP status codes, pagination, session status transitions, structured handoff/permission list endpoints
- Routing: sticky routing, health-aware selection, quota inference, configurable thresholds
- Adapters: end-to-end Codex/Antigravity/OpenCode CLI validation, structured stderr, Copilot/OpenCode ACP proof-of-life
- Persistence: SQLite migration versioning, crash recovery, checkpoint references

Post-MVP items (do not implement): ACP lifecycle, multi-workspace TUI, desktop app, cloud sync, concurrent multi-agent.

## Development Standards

### Think Before Coding

**Don't assume. Don't hide confusion. Surface tradeoffs.**

Before implementing:
- State your assumptions explicitly. If uncertain, ask.
- If multiple interpretations exist, present them — don't pick silently.
- If a simpler approach exists, say so. Push back when warranted.
- If something is unclear, stop. Name what's confusing. Ask.

### Simplicity First

**Minimum code that solves the problem. Nothing speculative.**

- No features beyond what was asked.
- No abstractions for single-use code.
- No "flexibility" or "configurability" that wasn't requested.
- No error handling for impossible scenarios.
- If you write 200 lines and it could be 50, rewrite it.

Ask yourself: "Would a senior engineer say this is overcomplicated?" If yes, simplify.

### Surgical Changes

**Touch only what you must. Clean up only your own mess.**

When editing existing code:
- Don't "improve" adjacent code, comments, or formatting unless asked.
- Don't refactor things that aren't broken.
- Match existing style, even if you'd do it differently.
- If you notice unrelated dead code, mention it — don't delete it unless asked.

When your changes create orphans:
- Remove imports/variables/functions that YOUR changes made unused.
- Don't remove pre-existing dead code unless asked.

The test: Every changed line should trace directly to the user's request.

### Goal-Driven Execution

**Define success criteria. Loop until verified.**

Transform tasks into verifiable goals:
- "Add validation" → "Write tests for invalid inputs, then make them pass"
- "Fix the bug" → "Write a test that reproduces it, then make it pass"
- "Refactor X" → "Ensure tests pass before and after"

For multi-step tasks, state a brief plan:
```
1. [Step] → verify: [check]
2. [Step] → verify: [check]
3. [Step] → verify: [check]
```

Strong success criteria let you loop independently. Weak criteria ("make it work") require constant clarification.

### Mandatory Unit Testing

**Every new feature, bug fix, or refactor MUST include corresponding unit tests.** Code without tests will not be accepted.

- New public functions/methods must have at least one test covering the happy path and one covering error/failure cases.
- New domain types (structs, enums) must have construction and serialization round-trip tests.
- New HTTP endpoints must have integration tests using `tower::ServiceExt` (no port binding).
- New TUI components must have rendering tests using `ratatui::backend::TestBackend`.
- New CLI commands must have tests for both `plan_cli_action()` mapping and output formatting.
- When fixing a bug, add a regression test that reproduces the original failure.
- When refactoring, ensure existing tests still pass and add new tests for any newly exposed behavior.
- Place tests in the `#[cfg(test)] mod tests` block at the bottom of the same file.
- Run `cargo test --workspace` after every change to verify no regressions.

### Testing Checklist (before marking work done)

1. `cargo test --workspace` passes with 0 failures.
2. New code has test coverage (not just existing tests still passing).
3. `cargo fmt --all -- --check` passes.
4. `cargo clippy --workspace --all-targets` passes with 0 warnings.

## Things to Avoid

- Do not add new workspace-level dependencies without updating `[workspace.dependencies]`.
- Do not write tests that call real agent providers (spend model quota).
- Do not implement Post-MVP features listed above.
- Do not add `pub` visibility to types/functions that are only used within their crate.
- Do not use `unwrap()` in non-test code; use `?`, `.context()`, or explicit error handling.
- Do not add features beyond what was asked (no speculative generality).
- Do not refactor working code that isn't related to your change.
- Do not "improve" adjacent code, comments, or formatting unless asked.
