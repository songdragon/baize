# Baize Quickstart

This guide covers the local daemon, HTTP/SSE APIs and terminal workbench. UI behavior and reliability checks are documented in [tui-design.md](tui-design.md).

Baize's product goal is one persistent development session across execution-agent switches, using workspace state plus relevant recent dialogue. The commands below exercise the current CLI implementation. Provider Desktop execution and shared agent-configuration mapping are not implemented; Baize's own future Desktop UI is a separate concern. See [the vision](vision.md), [requirements](spec.md), and [MVP validation priorities](mvp-implementation-plan.md).

A Baize session is the user-facing conversation, while provider-native session IDs are execution references underneath it. Existing resume/context-pack support is a baseline; do not interpret these examples as proof that all cross-provider context and native tools are preserved.

## Run The TUI

From a repository or project directory:

```sh
cargo run -p baize-cli -- tui
```

For local development, keep Baize state inside the repository:

```sh
BAIZE_DATA_DIR=.baize/data cargo run -p baize-cli -- tui
```

Ctrl-C, Ctrl-Q and the Quit command always open an exit confirmation, even when idle. Enter closes the terminal client; Esc keeps it open. Repeated Ctrl-C never bypasses confirmation. Closing the client does not cancel provider execution.

The TUI reads `daemon.host` and `daemon.port` from Baize configuration, starts the daemon automatically when needed, and leaves it running after the client exits. To run the daemon directly:

```sh
BAIZE_DATA_DIR=.baize/data cargo run -p baize-cli -- daemon
```

## Provider CLI Setup

Baize currently wires prompt execution through:

- Codex: `codex exec --json <prompt>`
- OpenCode: `opencode run --format json <prompt>`
- Antigravity: `/Users/songdragon/.local/bin/agy --print <prompt> --output-format stream-json`

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

## Live process output

Agent text, stderr diagnostics and available tool progress update as they arrive. Process output stays folded above the final prose reply. Press F9 to reveal the latest process; use Space to fold, Up/Down to select, Enter for full details, Left/Right for earlier turns and Esc to return to input. JSON messages merge using their original identity; there is no local typing animation. A failed SSE subscription means the prompt was not submitted and its draft is retained. A later disconnect reconnects and replays recorded events without rerunning the prompt. See [streaming implementation and tests](cli-streaming-output.md).

Restart an existing daemon after adapter/daemon changes: closing the TUI alone leaves the daemon running. Wait for any active provider turn to finish before restarting it. The folded process display is a TUI update and works with the `e02e94c` streaming daemon.

## TUI Keys

| Key | Action |
|---|---|
| `Enter` | Send to the selected provider in the current Baize conversation |
| `Alt-Enter`, `Shift-Enter` or `Ctrl-J` | Insert newline (terminal support varies) |
| `Left` / `Right`, `Up` / `Down`, `Home` / `End`, `Delete` / `Backspace` | Edit Unicode text and multiline input |
| `Tab` | Choose next executor; active and target providers remain distinct |
| `Ctrl-K` / `F1` | Search commands / keyboard help |
| `Ctrl-O` | Search saved workspace sessions and open with Enter |
| `Ctrl-R` | Refresh connection and diagnostics, retaining an available selected provider |
| `Ctrl-L` | Restore latest session in the loaded workspace list |
| `Ctrl-N` | New conversation, keeping the old session draft locally |
| `Ctrl-X` | Review cancellation; Enter cancels the Baize session, Esc stays; provider may continue |
| `Ctrl-E` | Mark session complete |
| `Ctrl-H` / `Ctrl-Y` | Prepare / accept a reviewed handoff |
| `Ctrl-P` | Refresh permissions and open their inspector |
| `Ctrl-A` / `Ctrl-D` | Review then approve / deny selected permission |
| `F2` / `F3` / `F4` | Previous / next / open selected session |
| `F5` | Refresh changes, target context and recorded execution results |
| `F6` / `Shift-Tab` | Forward / reverse focus: composer, conversation, sessions, inspector |
| `F7` / `F8` | Next inspector tab / toggle inspector |
| `PgUp` / `PgDn` / mouse wheel | Scroll conversation or focused inspector |
| `Ctrl-V`, `/image path`, `/unimage` | Clipboard image, image path, clear attachments |
| `Esc` | Close overlay or return focus to composer |
| `Ctrl-Q` or `Ctrl-C` | Always confirm exit; Enter quits, Esc stays |

Wide terminals can show conversations, the main dialogue and an inspector. Below 100 columns, use F6 or Shift-Tab to focus the inspector. F5 reconciles recorded results after a disconnect; it never automatically resubmits work. Recover the last submission for editing through Ctrl-K, and inspect uncertain results before sending again. Both Ctrl-A and Ctrl-D first open a hidden permission for review; press again after reviewing the same target. Baize permission records are separate from provider-native approval dialogs.

Draft text, cursor and attachment paths are saved in `{data_dir}/tui-drafts.json`, grouped by workspace and session. `BAIZE_DATA_DIR` overrides the data directory; otherwise Baize uses the system application data directory (on macOS, `~/Library/Application Support/baize`). New-conversation drafts return on startup; Ctrl-O / Ctrl-L restores drafts attached to saved sessions. Saving uses atomic replacement and Unix mode `0600`; a damaged file is preserved and a visible error replaces the normal footer. The file contains local plain text and attachment paths, not copies of attached images. Clearing a draft and allowing the next UI refresh removes it from the cache. A running or uncertain submission remains recoverable until its recorded result is reconciled; recovery never sends it automatically. Confirmed finished submissions are removed unless you have started another draft.

An offline UI fixture is available without any daemon or provider calls:

```sh
cargo run -p baize-tui --example workbench-preview -- 150 32
```

For a POSIX PTY smoke test with a local fixture daemon (requires the configured daemon port to be free):

```sh
cargo build -p baize-cli
python3 scripts/tui_smoke.py
```

The smoke test reads configuration without changing it, checks terminal restoration and sends zero provider prompts.

To test the release client with an already-running loopback daemon, using temporary drafts and no provider prompts:

```sh
cargo build --release -p baize-cli
python3 scripts/tui_interaction_smoke.py
```

This POSIX/Python 3.11+ check covers crash/restart recovery, focus, permission review, cancellation review and terminal restoration. Normal TUI startup registers the current workspace and probes diagnostics; the check never confirms cancellation or permission decisions. See [the interaction audit](tui-interaction-audit.md) for issues and verification evidence.

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
