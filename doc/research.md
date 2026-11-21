# Baize Research Notes

产品结论更新：2026-11-21。以下 provider 与协议观察保留原调研口径，未因产品理念更新而重新验证版本、能力或支持状态。当前实现以 `mvp-implementation-plan.md` 和代码为准。

## 1. 当前结论

### 1.1 产品目标与入口

Baize 保持同一个上层开发 session，在 agent 切换时组合当前 workspace、近期相关原始对话和有效约束，减少用户重述。历史迁移、摘要、路由和额度观察都是支撑机制。

当前 TUI/CLI 起点源于最初的工具生态，不代表终端是唯一或能力最完整的执行环境。用户关注 provider Desktop 的内置 prompt、MCP、skills、computer use、browser use 等便利能力，不能把这一诉求归为 UI 美观。

Baize 自身 Desktop UI 与 provider Desktop 实际执行接入分别评估。打开窗口或同品牌 CLI 不证明已经取得 Desktop 能力。

### 1.2 Adapter strategy

按能力和接口可控性选择接入路径：

- 官方原生接口能够保留更多所需执行能力时优先使用；
- ACP 满足需求时复用标准会话协议；
- CLI 提供当前执行基线与兼容路径，不静默声称和其他入口等效。

验证需要覆盖实际入口的 prompt、MCP、skills、computer/browser use、配置作用域、session 控制、消息/状态回流与权限交互。统一协议和同一模型不足以证明开发效果相同。

### 1.3 配置与上下文

通用配置优先复用项目 `AGENTS.md`、MCP、skills 与 provider 已有设置；支持映射时显示生效范围与缺口，保留原生 prompt 和专属工具。

切换使用 workspace + 近期相关对话，摘要按需辅助。无需默认全量转发历史、转换私有存档或等待源 agent 生成摘要。A → B → A 时需要补充旧原生 session 未知的新增信息。

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

- ACP 是 Baize 可复用的标准接入路径之一，是否优先取决于所需原生能力；
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

本机验证结果：

- `codex exec` 支持非交互运行；
- `codex exec --json` 支持 JSONL 事件输出；
- `codex exec --output-schema` 可约束最终响应结构；
- `codex resume` 支持 session id 和 `--last`；
- `codex resume --include-non-interactive` 可把非交互 session 纳入恢复范围；
- `codex mcp-server` 和 experimental `app-server` 存在；
- 当前 help 输出未显示 ACP server。

对 Baize 的含义：

- 若 Codex 提供 app server 或 SDK session resume，应优先 native adapter；
- 当前第一条可验证路径是 `codex exec --json`；
- resume 可通过 `codex resume <session-id>` 或 `codex exec resume` 继续调研；
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
- 本机验证显示 Gemini CLI 支持 `--acp`、`--prompt`、`--resume`、`--session-id`、`--list-sessions`、`--output-format json|stream-json`；
- Gemini 是 Baize 第一批 ACP validation 的优先目标。

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

切换以当前 workspace、近期相关原始对话和有效约束为基础，摘要按预算和相关性辅助。原 agent 可用时可以生成摘要，但源失联或额度耗尽不能阻止交接准备。

流程：

```text
1. 保持原 Baize session，在可确认的回合边界准备切换
2. Baize 收集 workspace state 和目标尚未知晓的近期对话
3. 补充仍然有效的约束与机械事实
   - git diff summary
   - changed files
   - commands run
   - test result
   - route history
4. 需要时生成摘要，按用户模式确认切换
5. 目标 agent 在实际执行环境中接收上下文和当前请求
6. 输入、输出和执行结果仍关联原 Baize session
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

优先使用 Baize 主对话记录与 workspace；必要时通过支持的接口读取 provider 历史，保留来源与角色后补充相关上下文。摘要不作为必须再调用原 agent 的前置步骤。

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

保留 Rust daemon、薄客户端和本地存储，把 Baize session 与 provider 原生 session 分层。当前 CLI 路径用于验证连续会话，原生服务和 provider Desktop 接入按能力单独验证。

当前上下文包已有原生续接、跨 provider 增量与完整上下文的基础策略，但不等于所有真实切换场景已经无缝完成。

### 6.2 验证顺序

1. 同一 Baize session 下的 A → B → A，承接近期讨论和当前 workspace。
2. 较早有效约束、原 agent 不可用、目标启动失败与结果未知。
3. 现有项目配置的复用与一份通用配置的实际映射范围。
4. Provider Desktop 原生能力能否通过支持的接口用于实际执行并回流状态。
5. 与原生工具、手工交接比较重述、重复尝试与产物质量。

当前代码的 CLI 基线为 Codex、Antigravity、OpenCode；其他 provider 与入口以单独验证结果记录，不再用早期 provider 排序代替能力验证。

### 6.3 需要继续确认

- 不同入口的能力与配置是否实际生效，而不只是参数或文件存在；
- session 控制、历史、工具状态和结果回流的公开边界；
- 上下文选择如何兼顾近期意图、有效约束和目标已有历史；
- 原生入口切换后是否损失用户依赖的工具能力；
- 是否需要额外标准协议入口，应在持续会话闭环验证后决定。

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

## Rust UI 研究更新（2026-11-21）

已读取引用讨论并访问官方来源：TUI 延续 Ratatui + Crossterm，借鉴 Yazi、GitUI、Television、Presenterm 的分栏、查找与阅读交互；TachyonFX 保留为可选动效。Baize Desktop 改为 GPUI + GPUI Kit、Iced、Slint、egui、Tauri 候选评估。详见 [来源与选型判断](ui-research.md) 和 [终端工作台设计](tui-design.md)。本次不重新宣称 provider Desktop 或所有原生工具能力已通过验证。
