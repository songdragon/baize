# Provider Context Research

Verified on 2026-07-09. This note records the provider capabilities Baize needs
for provider-aware context packing. Token window values are marked `unknown`
unless confirmed by official docs or local help.

## Streaming update — 2026-11-22

The local `agy --help` and [official headless documentation](https://antigravity.google/docs/cli/headless.md) now establish `--output-format stream-json`, `init/step_update/result` NDJSON, step text deltas, tool information and `conversation_id`. Baize enables that output mode and models structured output as supported. This supersedes the historical Antigravity structured-output/native-ID unknown entries below, without re-verifying its other native or Desktop capabilities. See [streaming implementation and tests](cli-streaming-output.md).

## Product Alignment — 2026-11-21

This note models the initial CLI execution profiles, not complete provider or Desktop capabilities. Its original verification date and unknown values remain unchanged. Do not infer Desktop built-in prompts, MCP, skills, computer use, browser use, or equivalent behavior from a CLI capability flag.

The product target is one persistent Baize session across execution-agent switches. Current workspace facts and relevant recent dialogue are complementary inputs; still-active older constraints and optional summaries fill gaps. Full private-transcript import and a new source-generated summary are not prerequisites.

Integration should select supported native interfaces, ACP, or CLI according to actual required capabilities. Provider Desktop execution must be verified independently from Baize's own Desktop UI. Shared configuration should reuse project instructions, MCP, skills, and provider settings while preserving native prompts and tools.

The implementation strategies at the end of this note are a baseline, not acceptance of seamless switching. Required follow-up verification includes A → B → A, workspace changes outside the last provider run, effective older constraints, source unavailability, target failure, and configuration/entry-specific tool availability.

## Codex CLI

Sources:
- Local help: `codex --help`, `codex exec --help`, `codex exec resume --help`
- Official docs: https://developers.openai.com/codex/cli/reference
- Official docs: https://developers.openai.com/codex/noninteractive
- Official docs: https://developers.openai.com/codex/mcp

Modeled capabilities:
- command: `codex`
- transport: CLI
- prompt entrypoint: `codex exec [PROMPT]`
- native resume: yes, `codex exec resume <SESSION_ID> [PROMPT]` or `--last`
- native session id capture: JSONL `thread.started.thread_id` when using `--json`
- structured output: yes, `--json` JSONL events and `--output-schema <FILE>`
- MCP: yes, `codex mcp` and `codex mcp-server`
- workspace args: yes, `--cd <DIR>` and repeatable `--add-dir <DIR>`
- file attachment: image attachment via `--image <FILE>`
- conversation export/import: unknown from researched CLI help/docs
- context window tokens: unknown
- default Baize context budget: 24000 tokens, source `ConfigDefault`, confidence `Low`
- reserved output tokens: 4000
- preferred context format: XML-tagged Markdown

## Google Antigravity CLI

Sources:
- Local help: `/Users/songdragon/.local/bin/agy --help`
- Local guide: `/Users/songdragon/.gemini/antigravity-cli/builtin/skills/antigravity_guide/references/cli.md`
- Official docs pointer: https://antigravity.google/docs/cli/reference
- Official docs pointer: https://antigravity.google/docs/cli/features

Modeled capabilities:
- command: `/Users/songdragon/.local/bin/agy`
- transport: CLI
- prompt entrypoint: `agy --print <PROMPT>`
- important invocation detail: prompt must immediately follow `--print`
- native resume: yes, `--conversation <ID>` and `--continue`
- native session id capture: unknown
- structured output: unknown from help, modeled as false for MVP
- ACP: unknown from help, modeled as false for MVP
- MCP: unknown from help, modeled as false for MVP
- workspace args: repeatable `--add-dir`, plus `--project` and `--new-project`
- file attachment: unknown
- conversation export/import: unknown
- context window tokens: unknown
- default Baize context budget: 16000 tokens, source `ConfigDefault`, confidence `Low`
- reserved output tokens: 4000
- preferred context format: XML-tagged Markdown

## OpenCode

Sources:
- Local help: `opencode --help`, `opencode run --help`, `opencode acp --help`
- Official docs: https://opencode.ai/docs/cli/
- Official docs: https://opencode.ai/docs/acp/

Modeled capabilities:
- command: `opencode`
- transport: CLI and ACP
- prompt entrypoint: `opencode run [message..]`
- native resume: yes, `opencode run --session <ID>` and `--continue`
- native session id capture: JSON event/session output, exact adapter field parser-owned
- structured output: yes, `opencode run --format json`
- ACP: yes, `opencode acp`
- MCP: yes, `opencode mcp`
- workspace args: `opencode run --dir <DIR>` and `opencode acp --cwd <DIR>`
- file attachment: yes, `opencode run --file <FILE>`
- conversation export/import: yes, `opencode export [sessionID]` and `opencode import <file>`
- context window tokens: unknown
- default Baize context budget: 16000 tokens, source `ConfigDefault`, confidence `Low`
- reserved output tokens: 4000
- preferred context format: XML-tagged Markdown

## GitHub Copilot CLI

Sources:
- Local help: `copilot --help` and `copilot help` both failed with `command not found`
- Official docs: https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-programmatic-reference
- Official docs: https://docs.github.com/en/copilot/reference/copilot-cli-reference/acp-server
- Official docs: https://docs.github.com/en/copilot/concepts/agents/copilot-cli/context-management
- Official docs: https://docs.github.com/en/copilot/concepts/context/mcp
- Official docs: https://docs.github.com/copilot/concepts/agents/copilot-cli/chronicle

Modeled capabilities:
- command: `copilot`
- transport: CLI and ACP
- prompt entrypoint: `copilot -p <PROMPT>` from programmatic docs
- native resume: yes conceptually through Copilot sessions, exact local CLI flag unknown
- native session id capture: unknown because local CLI is unavailable
- structured output: unknown from official docs searched, modeled as false for MVP
- ACP: yes, `--acp`, stdio and TCP modes
- MCP: yes, local and remote MCP servers; GitHub MCP server is built in
- workspace args: unknown
- file attachment: unknown
- conversation export/import: transcript export is documented; import unknown
- context window tokens: unknown numeric value; docs state the size varies by model and Copilot CLI uses automatic compaction around context pressure
- default Baize context budget: 16000 tokens, source `ConfigDefault`, confidence `Low`
- reserved output tokens: 4000
- preferred context format: XML-tagged Markdown

## Baize Defaults

- Same provider with captured native session id and no cross-provider delta:
  send the raw prompt and pass the native session id.
- Same provider without native session id:
  send a full Baize context pack.
- Switching provider, or returning to a stale provider after another provider
  worked:
  send Baize delta context plus the current prompt. If the target provider has a
  native session id, Baize still passes it so the provider can combine native
  history with the cross-provider delta.
- Unknown context windows:
  keep `context_window_tokens = None`, use the conservative
  `default_context_budget_tokens`, and allow `context.provider_token_budgets` in
  config to override the default.
