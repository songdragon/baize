# Folded process output: interactive TUI preview

This is an **offline Ratatui example for design review**. It uses the existing
Baize workbench shell, colors and terminal backend. It does not contact a daemon,
run a provider or modify a saved conversation. The main application now uses
this presentation in formal conversations. The example remains an offline
design reference; formal TUI controls are F9 to reveal the process, Space when
focused to fold, and Esc to return to the composer.

```sh
cargo run -p baize-tui --example streaming-preview
```

The example replays fixture operation updates and response chunks. During work,
the process block stays compact and shows the current operation. On completion,
it becomes one summary line above the response. The response uses normal prose,
without log prefixes or a vertical quotation border.

| Key | Action |
| --- | --- |
| Space | Expand or collapse the process block |
| Up / Down | Select an operation when expanded |
| Enter | Show or hide the selected operation's details |
| 1 / 2 / 3 | Jump to running, completed or expanded state |
| R | Replay the offline sequence |
| P | Pause or resume playback |
| PageUp / PageDown, Home / End | Scroll the conversation |
| Q / Esc / Ctrl-C | Quit and restore the terminal |

The example keeps failed-operation warnings visible even when folded. Raw
parameters and output belong inside operation details, rather than the answer.

Export the same renderer's actual terminal cells for visual review:

```sh
cargo run -p baize-tui --example streaming-preview -- --capture target/previews/folded-process
```

The resulting JSON files contain every cell's symbol, foreground, background and
bold style from Ratatui `TestBackend`. They are renderer captures, not generated
design mockups. Run the example's rendering and interaction-state tests with:

```sh
cargo test -p baize-tui --example streaming-preview
```
