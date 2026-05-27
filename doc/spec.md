# Baize Requirements Spec

## 1. 概述

Baize 是一个本地 workspace facade，用于连接并调度多个已安装的 coding agent。它在用户和 agent 之间提供统一的 workspace UI、路由策略、配额观察、session 管理和上下文交接能力。

第一阶段目标是验证：

```text
多个 coding agent 能否在同一个 repo 上，通过统一工作区状态和 handoff 机制，稳定接力完成任务。
```

## 2. 用户问题

### 2.1 手动切换成本高

用户同时安装多个 agent，但切换时需要重复描述任务、复制上下文、解释当前 diff。

### 2.2 quota 和 rate limit 会中断任务

Claude、OpenAI、Gemini 等 provider 的配额和限速机制不同。用户通常在失败后才知道不可用。

### 2.3 session 上下文不可迁移

每个 agent 都有自己的会话状态。一个 agent 写到一半，另一个 agent 不知道已经发生了什么。

### 2.4 多 agent 协作缺少工作区状态

文件修改、命令执行、测试结果、失败原因、用户约束没有统一记录。

### 2.5 用户不信任黑盒路由

如果系统自动切换 agent 但不解释原因，用户会担心质量、成本和安全边界变化。

## 3. 产品目标

### 3.1 MVP 目标

MVP 应该做到：

- 管理一个本地 workspace；
- 连接至少两个 coding agent adapter；
- 为每个任务创建 Baize-level session；
- 基于规则做 provider selection；
- 在同一任务中默认 sticky routing；
- 观察 provider health 和基础 quota 信号；
- 检测 rate limit / quota exhaustion / agent failure；
- 在切换前生成 handoff summary；
- 记录文件 diff、命令、测试结果和任务状态；
- 在 UI 或 CLI 中解释路由原因。

### 3.2 长期目标

最终版本应该做到：

- 支持主流 coding agent adapter；
- 支持用户自定义 routing policy；
- 支持自动、半自动、手动三种切换模式；
- 支持高质量 workspace memory；
- 支持 task replay 和 checkpoint restore；
- 支持 provider performance learning；
- 支持团队审计和协作；
- 支持 IDE、desktop、web、CLI 多入口；
- 支持 adapter SDK 和插件生态。

## 4. 核心概念

### 4.1 Workspace

一个 workspace 对应一个本地项目目录。

字段示例：

```json
{
  "id": "ws_123",
  "repoPath": "/path/to/repo",
  "branch": "main",
  "vcs": "git"
}
```

### 4.2 Task Session

用户发起的一段连续工作。

字段示例：

```json
{
  "id": "task_123",
  "workspaceId": "ws_123",
  "objective": "Refactor auth module and add tests",
  "activeProvider": "claude-code",
  "status": "running"
}
```

### 4.3 Agent Provider

一个已安装 coding agent 的适配器。

示例：

- Claude Code；
- Codex CLI；
- Gemini CLI；
- Cline；
- Aider。

### 4.4 Handoff Summary

agent 切换时生成的结构化交接材料。

应包含：

- task objective；
- current status；
- files changed；
- diff summary；
- commands run；
- test results；
- known failures；
- decisions made；
- user constraints；
- recommended next step。

### 4.5 Routing Decision

Baize 每次选择或继续使用 provider 时生成的决策记录。

示例：

```json
{
  "selectedProvider": "codex",
  "reason": "Claude quota is low and task is now test/debug heavy.",
  "mode": "assisted",
  "confidence": 0.78
}
```

## 5. MVP 功能需求

### 5.1 Workspace 初始化

用户可以选择一个本地 repo 作为 workspace。

系统应识别：

- repo path；
- git branch；
- dirty state；
- project type；
- package/test scripts；
- 最近修改文件。

### 5.2 Agent adapter registry

系统应维护可用 agent 列表。

MVP 至少支持两个 adapter。支持优先级：

```text
1. Codex
2. Gemini CLI
3. GitHub Copilot CLI
4. OpenCode
```

后续候选：

```text
5. Claude Code
6. Cline
7. Aider
```

每个 adapter 至少提供：

```ts
interface AgentProvider {
  id: string
  displayName: string
  capabilities: AgentCapabilities
  health(): Promise<ProviderHealth>
  runTask(input: AgentTaskInput): Promise<AgentRunResult>
}
```

### 5.3 Capability-based adapter

不要假设所有 agent 都支持同样能力。

capabilities 示例：

```ts
type AgentCapabilities = {
  interactiveChat: boolean
  nonInteractivePrompt: boolean
  patchMode: boolean
  shellAccess: boolean
  structuredOutput: boolean
  usageTelemetry: boolean
}
```

### 5.4 Provider health check

系统应能判断 provider 是否大致可用。

MVP 可支持：

- CLI 是否存在；
- auth 是否可用；
- 最近一次调用是否失败；
- 是否出现 rate limit 或 quota exceeded；
- 最近平均响应耗时。

### 5.5 Quota observer

系统应维护 quota 状态，但必须允许不确定性。

状态类型：

```ts
type QuotaConfidence = "exact" | "estimated" | "unknown"
```

示例：

```json
{
  "provider": "claude-code",
  "remainingPercent": 12,
  "resetEtaMinutes": 48,
  "confidence": "estimated"
}
```

### 5.6 Routing engine

MVP 使用规则引擎。

默认规则：

- 如果 task session 已有 active provider，且 provider healthy，且 quota 未低于阈值，则继续使用；
- 如果 provider quota 低于阈值，提示用户切换；
- 如果 provider rate limited，建议迁移；
- 如果 provider unavailable，自动进入 provider selection；
- 大型重构优先选择强规划 agent；
- 测试修复和命令密集任务优先选择本地执行稳定的 agent；
- 搜索、解释和大范围阅读任务可选择长上下文或低成本 agent。

### 5.7 Sticky routing

MVP 必须支持 sticky routing。

默认：

```text
同一 task session 内保持同一 provider。
```

允许配置：

- sticky window；
- quota threshold；
- failure threshold；
- manual override。

### 5.8 Handoff generation

当用户或系统决定切换 agent 时，Baize 应生成 handoff summary。

MVP 可以通过 workspace state 和 session log 生成，不要求完美自动理解所有代码。

### 5.9 Workspace state tracking

MVP 应记录：

- user prompts；
- selected provider；
- route decisions；
- files changed；
- git diff summary；
- commands run；
- command outputs summary；
- test result status；
- agent errors。

### 5.10 User confirmation modes

MVP 至少支持：

```text
Manual: 只提示，不自动切换。
Assisted: 推荐切换，用户确认。
```

长期支持：

```text
Autopilot: 低风险任务自动切换，高风险任务确认。
```

### 5.11 Route explanation

每次关键路由都应向用户解释。

示例：

```text
Using Codex because the active Claude session is rate limited and this task is now focused on failing tests.
```

### 5.12 Diff/checkpoint awareness

MVP 至少应展示当前 workspace dirty state，并在 agent 切换前提醒用户。

长期应支持：

- checkpoint；
- revert selected agent changes；
- per-agent diff attribution；
- conflict detection。

## 6. 最终用户视角

### 6.1 第一次使用

用户打开 Baize，选择本地 repo。

Baize 显示：

- 当前 branch；
- dirty state；
- 检测到的项目类型；
- 可用 agent；
- 每个 agent 的 health/quota 概览。

用户无需先选择模型，可以直接输入任务。

### 6.2 发起任务

用户输入：

```text
帮我重构 auth 模块，把 provider 逻辑拆出来，并补测试。
```

Baize 选择 agent，并显示：

```text
Using Claude Code
Reason: large refactor, healthy quota, no active session conflict.
```

### 6.3 任务进行中

用户看到：

- 当前 agent；
- 正在修改的文件；
- 运行过的命令；
- 测试状态；
- 当前 route confidence；
- quota warning。

### 6.4 quota 即将耗尽

Baize 提示：

```text
Claude quota is low. Estimated remaining: 8%.
The task is now mostly test/debug work.

Recommended: switch to Codex.
```

用户可以选择：

```text
Continue Claude
Switch to Codex
Enable autopilot for this task
```

### 6.5 agent 迁移

用户选择切换后，Baize 生成 handoff summary，并交给 Codex。

Codex 接手时知道：

- 当前目标；
- 已修改文件；
- 为什么修改；
- 哪些测试失败；
- 下一步建议；
- 用户要求。

用户继续在同一个 UI 里对话，不需要手工复制上下文。

### 6.6 任务完成

Baize 展示：

- 最终 diff；
- 涉及 agent；
- route history；
- commands run；
- test result；
- handoff count；
- unresolved risks。

## 7. 非功能需求

### 7.1 本地优先

workspace 文件、session log 和 handoff summary 默认保存在本地。

### 7.2 可解释性

系统必须记录 route decisions，方便用户理解和调试。

### 7.3 安全

高风险操作需要用户确认。

包括：

- destructive shell command；
- 大规模文件删除；
- 跨 workspace 写入；
- 自动切换到 shellAccess 更强的 agent；
- 提交、推送、发布等操作。

### 7.4 可恢复

系统应能从中断中恢复 task session。

MVP 至少能恢复：

- task objective；
- active provider；
- route history；
- file change summary。

### 7.5 可扩展

adapter 应是插件式结构，避免核心系统绑定特定 agent。

## 8. 建议项目目录

MVP 初始目录：

```text
baize/
  apps/
    desktop/
  crates/
    baize-core/
    baize-daemon/
    baize-tui/
    baize-acp/
    baize-adapters/
    baize-workspace/
    baize-storage/
    baize-config/
    baize-cli/
  doc/
    vision.md
    spec.md
    research.md
    tech-selection.md
    technical-spec.md
```

目录职责：

- `apps/desktop/`: 未来 Tauri desktop app；
- `crates/baize-core/`: 核心领域模型、task session、routing、handoff、quota 抽象；
- `crates/baize-daemon/`: headless daemon，本地 API、事件流和生命周期管理；
- `crates/baize-tui/`: ratatui TUI 客户端；
- `crates/baize-acp/`: ACP client、transport、capability mapping；
- `crates/baize-adapters/`: Codex、Gemini、Copilot、OpenCode 等 provider adapter；
- `crates/baize-workspace/`: repo 状态、diff、命令、文件上下文；
- `crates/baize-storage/`: SQLite storage、event log、migration；
- `crates/baize-config/`: TOML 配置、schema、profile；
- `crates/baize-cli/`: 命令入口，启动 daemon/TUI、配置和诊断命令；
- `doc/`: 产品愿景、需求、架构设计、决策记录；
- `doc/research.md`: ACP、agent session、handoff、attribution 等调研结论；
- `doc/tech-selection.md`: 技术选型、运行时、跨平台和未来 desktop app 策略；
- `doc/technical-spec.md`: Rust 技术 spec、模块边界、API、数据模型和实现计划。

## 9. MVP 不做

MVP 暂不做：

- 完整 IDE；
- 完整团队权限系统；
- provider quota 的绝对精确统计；
- 所有 agent 的完整能力适配；
- 自动修复复杂 merge conflict；
- 云端同步；
- marketplace；
- 复杂 AI-based routing。

## 10. 已回答的产品决策

### 10.1 第一版 UI

第一版使用 TUI。

原因：

- 符合 coding agent 用户的终端工作流；
- 比 desktop/web 更快验证核心调度闭环；
- 更容易嵌入本地 repo、命令、diff、日志和 agent process；
- 未来仍可在同一核心层上增加 web 或 desktop UI。

### 10.2 Adapter 优先协议

优先 ACP 协议。

如果某个 agent 的 ACP 能力无法覆盖 Baize 的 MVP 或长期需求，再针对该 agent 增加 fallback adapter。

建议 adapter 分层：

```text
ACP adapter
  优先使用标准 ACP 能力

Native adapter
  使用 agent SDK、app server、remote-control 或官方自动化接口

CLI adapter
  最后 fallback 到命令行进程、stdout/stderr、session 文件和约定式解析
```

### 10.3 Session 状态持久化和引用

需要调研每个 agent 的具体机制。

Baize 不应假设所有 agent 都有统一 session store。应在 Baize 内部维护自己的 task session，同时把 provider session id、resume command、transcript location 或 checkpoint location 作为外部引用保存。

### 10.4 Handoff summary

期望由 agent 生成。

优先方案：

```text
当前 agent 生成 handoff summary。
Baize 用 workspace state、diff、命令记录和测试结果校验/补充。
目标 agent 接收 handoff 后继续任务。
```

备选方案：

```text
如果 provider 暴露 transcript/session 文件，Baize 可让目标 agent 读取这些过程状态，再生成自己的接手摘要。
```

注意：不能只依赖目标 agent 自行阅读前一个 agent 的原始历史，因为不同 agent 的 transcript 格式、权限边界和压缩语义不同。

### 10.5 Git checkpoint

可配置。

MVP 可以提供策略选项：

```text
off
before_agent_run
before_handoff
before_destructive_action
always
```

默认建议：

```text
before_handoff
```

### 10.6 Shell command 权限

按用户要求提供配置项。

建议最小权限模型：

```text
ask
allow_safe
allow_project
deny
```

高风险命令必须再次确认，即使用户开启较宽松策略。

### 10.7 Route policy 配置方式

第一版使用配置文件。

产品功能完善后，再提供 UI 设置。

### 10.8 Agent 修改归因粒度

需要继续调研后决定。

初步建议：

```text
MVP: command/run 级别 + file 级别
Next: hunk 级别
Long term: line/hunk 级别 + semantic decision log
```

原因：file 级别容易实现但解释力有限；hunk 级别对 handoff 和 review 最有价值；line 级别成本高，且在格式化、重构、批量移动时容易产生误导。

## 11. 仍需调研的问题

- ACP 在 Codex、Gemini CLI、GitHub Copilot CLI、OpenCode、Claude Code、Cline、Aider 上的实际支持程度；
- ACP 的 session/load、file system、terminal、permission、plan、diff 能力是否满足 MVP；
- 各 agent session/transcript/checkpoint 的本地保存位置、格式、保留周期和 resume API；
- 读取其他 agent transcript 生成 handoff 的安全边界和隐私风险；
- 不同 attribution 粒度对性能、准确性和用户体验的影响；
- TUI 中如何展示 provider health、quota、route history、diff 和 handoff。
