# Baize Research Notes

## 1. 当前结论

### 1.1 UI

第一版采用 TUI。

TUI 更贴近 coding agent 的真实使用环境，也更容易管理：

- 本地 repo；
- agent subprocess；
- route decision；
- diff；
- shell command；
- checkpoint；
- handoff log。

### 1.2 Adapter strategy

优先 ACP，但不要只押注 ACP。

建议策略：

```text
ACP first, native second, CLI fallback.
```

原因：

- ACP 是为 editor/client 与 coding agent 通信设计的协议；
- 它覆盖 session、prompt turn、permission、file system、terminal、plan、mode 等核心交互；
- 但不同 agent 的 ACP 支持程度会不同；
- quota、provider-specific session、transcript、checkpoint 等能力可能仍需要 native 或 CLI adapter。

## 2. ACP 调研摘要

ACP 的官方介绍说明，它标准化的是 code editor/IDE 与 coding agent 之间的通信，并适合本地和远程场景。官方文档也说明，本地 agent 通常作为 client 的子进程运行，通过 stdio 上的 JSON-RPC 通信；远程 agent 可通过 HTTP 或 WebSocket 通信。

ACP protocol overview 中的基础流程包括：

```text
initialize
authenticate
session/new
session/load
session/prompt
session/update
session/cancel
```

它还定义了 client 侧能力，例如：

- 权限请求；
- 读取文件；
- 写入文件；
- 创建终端；
- 获取终端输出；
- 等待命令退出；
- kill terminal。

对 Baize 的含义：

- ACP 很适合作为 Baize 的首选 adapter 协议；
- Baize 可以扮演 ACP client；
- agent 作为 ACP server；
- Baize TUI 负责展示 session updates、diff、tool calls、plans 和 permission prompts；
- 如果 agent 支持 `session/load`，Baize 可以通过协议级能力恢复 provider session。

限制：

- ACP 不等于 quota API；
- ACP 不保证每个 agent 都支持 session load；
- ACP 不保证能读取 provider 自己的完整 transcript；
- ACP 不保证所有 agent 都暴露 checkpoint 或细粒度 diff attribution。

## 3. Agent session 调研摘要

### 3.1 Claude Code

Claude Code 官方文档说明 session 是绑定 project directory 的 saved conversation，会持续保存到本地 transcript 文件，并支持：

```text
claude --continue
claude --resume
claude --resume <name>
/resume
/branch
/compact
/context
/export
```

Claude Code CLI transcript 默认保存在：

```text
~/.claude/projects/<project>/<session-id>.jsonl
```

文档还说明可以通过 `CLAUDE_CONFIG_DIR` 改变保存位置，也可以通过配置或环境变量影响 transcript 保留和写入。

Claude Agent SDK 另有 `SessionStore` 机制，可把 transcript mirror 到外部存储，并通过 `load` 在 resume 前读回。这个机制对 Baize 很有价值，因为它说明 Claude 侧存在较清晰的 session persistence 抽象。

对 Baize 的含义：

- Claude Code adapter 可以保存 session id、session name、project key、transcript path；
- 可以调用 `claude --resume <session-id>` 或 SDK resume；
- 可以在 handoff 时读取/export transcript，但要注意 compaction 后 agent 实际可见消息和 raw history 不完全相同。

### 3.2 Codex CLI

OpenAI 官方文档说明 Codex CLI 是本地运行的 coding agent，可读取、修改、运行当前目录代码，并提供 TUI、approval modes、MCP、hooks、SDK、app server 等入口。

当前需要继续调研：

- Codex CLI session id 如何稳定引用；
- TUI session 是否有官方 resume API；
- app server/SDK 是否暴露 session list、resume、transcript；
- 本地 transcript 的位置、格式和保留策略；
- 是否存在类似 Claude `SessionStore` 的机制。

对 Baize 的含义：

- 若 Codex 提供 app server 或 SDK session resume，应优先 native adapter；
- 若未来支持 ACP server，则优先 ACP adapter；
- 若只能通过 TUI/CLI，则需要 CLI fallback 和较保守的 session 引用。

### 3.3 Gemini CLI

Gemini CLI 官方 checkpointing 文档说明，它可在 AI 工具修改文件前创建 checkpoint。checkpoint 包含：

- shadow git repository 中的 git snapshot；
- conversation history；
- tool call。

文档说明 checkpoint 数据通常位于：

```text
~/.gemini/tmp/<project_hash>/checkpoints
```

并通过 `/restore` 回滚。

对 Baize 的含义：

- Gemini adapter 可以利用 checkpoint 作为安全恢复机制；
- Baize 应保存 Gemini checkpoint location 和 restore reference；
- Gemini 的 checkpoint 机制偏“恢复与回滚”，不必然等同于跨 agent handoff 的标准 transcript。

### 3.4 GitHub Copilot CLI

GitHub Copilot CLI 官方文档说明它支持 macOS、Linux，以及 Windows PowerShell 和 WSL。它提供 interactive 和 programmatic 两种使用模式，并支持 MCP servers、custom agents、hooks、skills、Copilot Memory、model selection 和自定义 model provider。

Copilot CLI 已提供 ACP server，官方文档说明可通过 ACP 在第三方工具、IDE 或自动化系统中使用 Copilot CLI。当前 ACP server 标记为 public preview。

官方示例展示了通过 TypeScript 的 `@agentclientprotocol/sdk`，以 stdio 方式启动和连接：

```text
copilot --acp --stdio
```

Copilot CLI 的配置目录文档说明：

```text
~/.copilot
```

用于保存配置、session data 和 customizations。

对 Baize 的含义：

- Copilot CLI 应进入第一批优先 adapter；
- 首选 ACP adapter；
- session data、memory、custom agents 和 hooks 可作为 native/CLI fallback 的调研重点；
- Copilot premium request 用量可以作为 quota observer 的特殊维度；
- Copilot 的 allow/deny tool 模型与 Baize shell policy 有较好映射关系。

风险：

- ACP server 是 public preview，接口和行为可能变化；
- Copilot quota 可能更接近 premium request 计数，而不是 token/RPM；
- organization policy 可能影响 CLI、MCP、model 和 agent 能力。

### 3.5 OpenCode

OpenCode 官方文档说明它支持 ACP，可通过：

```text
opencode acp
```

启动为 ACP-compatible subprocess，并通过 stdio 上的 JSON-RPC 与 editor/client 通信。

OpenCode CLI 默认启动 TUI，也支持 programmatic command：

```text
opencode run "Explain how closures work in JavaScript"
```

CLI 支持继续和指定 session：

```text
opencode --continue
opencode --session <session-id>
opencode --fork
```

OpenCode 还提供 JS/TS SDK，用于连接和控制 OpenCode server。官方站点说明 OpenCode 有 terminal、IDE、desktop app 形态，desktop beta 支持 macOS、Windows 和 Linux。Windows 文档建议使用 WSL 以获得更好的文件系统性能、终端支持和开发工具兼容性。

对 Baize 的含义：

- OpenCode 是 Baize 的高优先级 adapter；
- 首选 ACP adapter；
- native fallback 可以使用 OpenCode JS/TS SDK 或 server API；
- session id、server mode、desktop sidecar 思路对 Baize 自己的 daemon/desktop 形态有参考价值；
- OpenCode 的 multi-session 能力与 Baize parallel task/session manager 可形成互补。

风险：

- OpenCode 自身已经有 CLI/TUI/desktop/server，多层集成方式需要选择边界；
- 作为 provider-flexible agent，OpenCode 的 quota 可能来自下游模型 provider，而不是 OpenCode 自身；
- 如果通过 OpenCode 再连 Copilot/Claude/Gemini，Baize 需要避免形成不透明的二级路由。

## 4. Handoff 方案建议

### 4.1 推荐方案

handoff summary 由 agent 生成，Baize 校验和补充。

流程：

```text
1. Baize 收集 workspace state
2. 当前 agent 生成 handoff summary
3. Baize 附加机械事实
   - git diff summary
   - changed files
   - commands run
   - test result
   - route history
4. 用户确认或自动批准
5. 目标 agent 接收 handoff
```

### 4.2 读取上一个 agent 状态的方案

可以做，但应作为增强能力，不应作为唯一机制。

原因：

- agent transcript 格式不同；
- transcript 可能包含隐私或 provider-specific metadata；
- 有些 agent 会 compact，raw history 不等于 resume 时可见 context；
- 有些 session 数据可能有保留期限；
- 目标 agent 直接阅读另一个 agent 原始日志，成本高且容易误读。

建议：

```text
Baize 读取 provider transcript -> 生成 normalized session events -> 交给 agent 生成 handoff。
```

不要：

```text
把 Claude 的完整 JSONL 原样塞给 Codex。
```

## 5. Attribution 粒度建议

### 5.1 MVP

MVP 建议做到：

```text
agent run 级别 attribution
file 级别 attribution
```

即记录：

- 哪个 agent run 开始；
- run 前 git status；
- run 后 git status；
- run 改了哪些文件；
- run 触发了哪些命令；
- run 产生了哪些测试结果。

### 5.2 下一阶段

下一阶段做到：

```text
hunk 级别 attribution
```

即通过每次 run 前后的 diff 对比，把新增/修改 hunk 归到对应 agent run。

这对 review 和 handoff 最有价值。

### 5.3 长期

长期可考虑：

```text
semantic attribution
```

例如：

- 这个 hunk 属于哪个 decision；
- 哪个 hunk 是修测试；
- 哪个 hunk 是格式化；
- 哪个 hunk 是 agent A 写、agent B 修改。

line 级别 attribution 不建议作为第一目标，因为重构、格式化、移动文件会让 line ownership 噪声很大。

## 6. 当前建议

### 6.1 技术路线

```text
TUI
+ Rust daemon/core
+ ratatui TUI
+ Baize internal task session
+ ACP-first provider adapter
+ per-agent native/CLI fallback
+ workspace state tracker
+ agent-generated handoff
+ configurable checkpoint and command policy
```

### 6.2 MVP adapter 优先级

建议：

```text
1. Codex
2. Gemini CLI
3. GitHub Copilot CLI
4. OpenCode
```

原因：

- Codex 是 Baize 的核心目标 agent，应优先验证；
- Gemini CLI 的 checkpointing 对恢复能力很有价值；
- Copilot CLI 已提供 ACP server，且有 session data、custom agents、hooks、memory、premium request 等值得集成的能力；
- OpenCode 明确支持 ACP，同时提供 server、SDK、TUI、desktop 多形态，是验证 ACP-first 和 native fallback 的好对象。

Claude Code 暂放入第二梯队，但仍是重要候选，因为它的 session/transcript/resume 机制资料较清晰。

### 6.3 需要继续确认

- Codex CLI 最新版本是否提供 ACP server；
- Gemini CLI 是否提供 ACP server；
- GitHub Copilot CLI ACP server public preview 的稳定性和 capability 覆盖；
- OpenCode ACP 与 server/SDK 在 session、diff、permissions、events 上的差异；
- Claude Code 是否提供或计划提供 ACP server；
- Cline/Aider 的 ACP 或自动化接口成熟度；
- Baize 是否需要自己实现 ACP server，让 IDE 也能把 Baize 当成一个 agent。

## 7. Sources

- Agent Client Protocol introduction: https://agentclientprotocol.com/
- Agent Client Protocol overview: https://agentclientprotocol.com/protocol/overview
- GitHub Copilot CLI ACP server: https://docs.github.com/en/copilot/reference/copilot-cli-reference/acp-server
- GitHub Copilot CLI overview: https://docs.github.com/en/copilot/concepts/agents/copilot-cli/about-copilot-cli
- Claude Code sessions: https://code.claude.com/docs/en/sessions
- Claude Code SessionStore: https://code.claude.com/docs/en/agent-sdk/session-storage
- OpenAI Codex CLI: https://developers.openai.com/codex/cli
- Gemini CLI checkpointing: https://google-gemini.github.io/gemini-cli/docs/checkpointing.html
- OpenCode ACP: https://open-code.ai/en/docs/acp
- OpenCode CLI: https://dev.opencode.ai/docs/cli/
- OpenCode SDK: https://opencode.ai/docs/sdk/
- OpenCode Windows WSL: https://open-code.ai/en/docs/windows-wsl
