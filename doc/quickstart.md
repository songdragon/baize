# Baize MVP Quickstart

This guide covers the current local MVP: a headless daemon, HTTP/SSE APIs and a thin TUI client.

## Run The TUI

From a repository or project directory:

```sh
cargo run -p baize-cli -- tui
```

For local development, keep Baize state inside the repository:

```sh
BAIZE_DATA_DIR=.baize/data cargo run -p baize-cli -- tui
```

The TUI starts the daemon automatically when needed. To run the daemon directly:

```sh
BAIZE_DATA_DIR=.baize/data cargo run -p baize-cli -- daemon
```

## Provider CLI Setup

Baize currently wires prompt execution through:

- Codex: `codex exec --json <prompt>`
- OpenCode: `opencode run --format json <prompt>`
- Antigravity: `/Users/songdragon/.local/bin/agy --print <prompt>`

Gemini CLI is no longer a default prompt provider for Baize because individual Gemini Code Assist accounts are directed to Antigravity. Baize keeps Gemini only as a legacy diagnostic profile, so old sessions remain readable but new default routing avoids it.

If an older config file still contains the previous default order `codex, gemini, copilot, opencode`, the daemon treats that order as `codex, antigravity, opencode, copilot` at runtime.

Install and authenticate provider CLIs with their own login/setup flows before sending real prompts through Baize. The MVP validation commands do not spend model quota by themselves:

```sh
cargo run -p baize-cli -- doctor
cargo run -p baize-cli -- providers
cargo run -p baize-cli -- validate
cargo run -p baize-cli -- validate codex
cargo run -p baize-cli -- validate antigravity
cargo run -p baize-cli -- validate opencode
cargo run -p baize-cli -- smoke codex
cargo run -p baize-cli -- smoke antigravity
cargo run -p baize-cli -- smoke opencode
cargo run -p baize-cli -- ask --provider codex "summarize this project"
```

`doctor` reports each provider's readiness, setup issues and suggested next actions. If a provider is not authenticated, Baize will report provider stderr and a structured provider error where possible.

The smoke commands check provider discovery, command construction and structured-output parser behavior without sending a real prompt. To explicitly run a real provider prompt, add `--run-prompt`:

```sh
cargo run -p baize-cli -- smoke codex --run-prompt --timeout-seconds 30
cargo run -p baize-cli -- smoke opencode --run-prompt --timeout-seconds 30
cargo run -p baize-cli -- smoke antigravity --run-prompt --timeout-seconds 30
```

Only use `--run-prompt` when the provider CLI is installed, authenticated and you are ready to spend provider quota.

## Execution Policy

Prompt execution follows `workspace.command_policy` in `~/.config/baize/config.toml`:

| Policy | Codex | Antigravity | OpenCode |
|---|---|---|---|
| `ask` | `--sandbox workspace-write` | `--mode accept-edits --sandbox` | no extra approval flag |
| `allow_project` | `--sandbox workspace-write` | `--mode accept-edits --dangerously-skip-permissions` | `--auto` |
| `deny` | `--sandbox read-only` | `--mode plan --sandbox` | no extra approval flag |

The default is `ask`. Use `deny` when you want Baize to inspect or plan without modifying files.

## TUI Keys

| Key | Action |
|---|---|
| `Enter` | Submit the prompt |
| `Tab` | Switch selected provider |
| `Ctrl-R` | Refresh provider health |
| `Ctrl-L` | Load latest session |
| `Ctrl-N` | Start a new session draft |
| `Ctrl-X` | Cancel current session |
| `Ctrl-E` | Mark current session complete |
| `Ctrl-H` | Create handoff preview |
| `Ctrl-Y` | Accept pending handoff |
| `Ctrl-P` | Refresh pending permissions |
| `Ctrl-A` | Approve selected permission |
| `Ctrl-D` | Deny selected permission |
| `F2` / `F3` | Select previous/next recent session |
| `F4` | Load selected recent session |
| `Up` / `Down` | Select pending permission |
| `Esc` or `Ctrl-C` | Exit |

## Local API Examples

Start the daemon:

```sh
BAIZE_DATA_DIR=.baize/data cargo run -p baize-cli -- daemon
```

In another terminal:

```sh
curl -s http://127.0.0.1:7878/health
curl -s http://127.0.0.1:7878/providers
curl -s -X POST http://127.0.0.1:7878/providers/diagnose
curl -s http://127.0.0.1:7878/providers/antigravity/diagnose
curl -s http://127.0.0.1:7878/providers/opencode/validate
curl -s "http://127.0.0.1:7878/workspaces/status?path=."
```

Create a workspace:

```sh
curl -s -X POST http://127.0.0.1:7878/workspaces \
  -H 'content-type: application/json' \
  -d '{"path":"."}'
```

List workspaces:

```sh
curl -s http://127.0.0.1:7878/workspaces
curl -s 'http://127.0.0.1:7878/workspaces?name=WORKSPACE_NAME'
```

List projects in a workspace after replacing `WORKSPACE_ID`:

```sh
curl -s http://127.0.0.1:7878/workspaces/WORKSPACE_ID/projects
curl -s 'http://127.0.0.1:7878/workspaces/WORKSPACE_ID/projects?kind=directory&vcs=none'
curl -s http://127.0.0.1:7878/projects/PROJECT_ID
```

Create a session after replacing `WORKSPACE_ID`:

```sh
curl -s -X POST http://127.0.0.1:7878/sessions \
  -H 'content-type: application/json' \
  -d '{"workspace_id":"WORKSPACE_ID","objective":"inspect this project","provider_id":"codex"}'
```

Send a prompt after replacing `SESSION_ID`:

```sh
curl -s -X POST http://127.0.0.1:7878/sessions/SESSION_ID/prompt \
  -H 'content-type: application/json' \
  -d '{"prompt":"summarize the current project","timeout_seconds":30}'
```

Mark a session complete:

```sh
curl -s -X POST http://127.0.0.1:7878/sessions/SESSION_ID/complete
```

Read session events:

```sh
curl -s http://127.0.0.1:7878/sessions/SESSION_ID/events
```

Read historical events as JSON:

```sh
curl -s 'http://127.0.0.1:7878/events/history?session_id=SESSION_ID&limit=20'
curl -s 'http://127.0.0.1:7878/events/history?provider_id=codex&event_type=session.agent.output'
```

Subscribe to the live event stream:

```sh
curl -N http://127.0.0.1:7878/events
```

## Test And Coverage Commands

```sh
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets
cargo llvm-cov --workspace --summary-only
```

The unit and integration tests use fake executors and parser fixtures. They should not spend model quota.
