# Baize Requirements Spec

更新：2026-11-21。本文规定目标行为；当前已实现能力与产品验收缺口分别记录在 [MVP 计划](mvp-implementation-plan.md)。

## 1. 概述

Baize 提供可更换执行 agent 的持续开发会话。用户在同一个 Baize session 中讨论和开发；更换 agent 时，系统结合当前 workspace、近期相关对话和仍然有效的约束继续工作。

接入目标包括 CLI 与具备可用接口的 provider Desktop 执行环境。需要保留的原生能力包括内置 prompt、MCP、skills、computer use、browser use 等；不能仅以模型或品牌相同推定两个入口等效。

第一阶段目标是验证：用户在 A → B → A 的切换中能否保持同一个 Baize session，减少重述与纠偏，并获得声明的实际执行能力。统一配置、路由、额度观察和交接摘要服务于这一目标。

## 2. 用户问题

### 2.1 手动切换成本高

用户同时安装多个 agent，但切换时需要重复描述任务、复制上下文、解释当前 diff。

### 2.2 quota 和 rate limit 会中断任务

Claude、OpenAI、Gemini 等 provider 的配额和限速机制不同。用户通常在失败后才知道不可用。

### 2.3 用户工作与 provider session 绑定

每个 agent 都有自己的原生会话。换执行者时，用户容易被迫另开对话、重述近期讨论；切回旧 agent 时，它也不知道离开期间的新工作。需要保持 Baize session 连续，而不要求跨产品搬运全部内部状态。

### 2.4 工作区与对话无法共同承接

文件、文档、Git 和验证结果反映执行事实，近期对话包含当前意图、修正和下一步要求。只交接其中一类信息都可能导致重新解释或重复尝试。

### 2.5 用户不信任黑盒路由

如果系统自动切换 agent 但不解释原因，用户会担心质量、成本和安全边界变化。

### 2.6 统一入口损失原生能力

Desktop 产品可能整合内置 prompt、MCP、skills、computer use、browser use 等能力。用户希望保留实际执行环境，不能把关注 Desktop 简化为对窗口、布局或视觉呈现的偏好。

### 2.7 重复配置

用户希望一套通用要求在不同 agent 中生效，但项目指令、MCP 与 skills 已能承担部分复用工作。Baize 应在这些基础上减少重复设置，不要求先建立新的角色和知识管理体系。

## 3. 产品目标

### 3.1 MVP 目标

- 管理一个本地 workspace，连接至少两个实际可执行的 coding agent；
- 使用稳定的 Baize session 身份和主对话承接用户工作，不因 agent 更换另建上层会话；
- 在同 provider 原生续接、跨 provider 接手、切回旧 provider 时组织正确上下文；
- 组合当前 workspace 与近期相关原始对话，保留仍然有效的较早约束；
- 无法调用原 agent 时仍能准备交接，不以它先生成摘要作为前提；
- 记录用户消息、agent 输出、切换、diff、命令、验证结果与已知能力缺口；
- 提供明确的手动或辅助切换，默认同一时刻一个 agent 执行工作；
- 复用已有项目配置，标明当前共享配置映射与执行入口的支持范围。

当前工程以 CLI 路径验证上述目标。实现了 session ID、日志或 resume 参数，不等于已完成连续性产品验收。

### 3.2 后续能力

- 通过官方接口接入 provider Desktop 等更完整的原生执行环境；
- 一套用户通用配置在支持的执行入口中映射生效，并报告不支持项；
- 在接口允许时，将原生环境中的输入、输出、审批与执行状态关联回原 Baize session；
- 改善 Baize 自身的 UI、原生服务接入、上下文选择和中断恢复；
- 在连续会话验证后，再评估自动路由、精确额度、回放、团队与云能力。

Baize 自身的 Desktop UI 和 provider Desktop 执行接入是两个独立方向。

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

用户发起的一段持续开发会话，产品中称 Baize session，代码中沿用 `TaskSession`。它承接讨论、澄清、编码和反馈，不要求先有正式任务单。

- Baize session 的身份、主对话和 workspace 关联跨 agent 切换保持不变；
- provider 原生 session 是下层执行引用；同一个 Baize session 可以关联多条；
- 恢复旧 provider session 时必须补充它尚未知晓的新对话与工作区变化；
- 不同工作目标可以另建 Baize session，连续性不等于无限增长的单一模型上下文。

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

coding agent 执行环境的适配对象，例如 Codex、Antigravity、OpenCode、Copilot 或 Claude Code。

| 概念 | 示例与作用 |
|---|---|
| Provider | 产品或工具身份 |
| 执行入口 | CLI、原生服务、Desktop 会话等，决定实际运行环境 |
| Transport | ACP、官方 RPC/SDK、HTTP 或 CLI 等通信方式 |
| 原生配置与能力 | 该入口的内置 prompt、MCP、skills、computer/browser use、权限与工具 |

同一个 provider 可以有不同执行入口，其能力不保证相同。打开 Desktop 窗口或深链接仅是导航，不能据此声明已获得 Desktop 的执行能力。

### 4.4 Handoff Summary

上下文较长、预算不足或需要人工检查时使用的交接材料。摘要是辅助信息，不是每次切换必经的 agent 调用，也不是全部原始历史的替代。

按相关性可包含：

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

MVP 至少支持两个实际可执行的 adapter。当前 CLI 基线是 Codex、Antigravity、OpenCode；Copilot 等路径以验证结果标注，不用注册或健康检查代替执行证明。

新增入口按实际能力选择：官方原生接口能保留更多所需能力时优先使用，ACP 能满足需求时复用标准协议，CLI 提供兼容路径。provider Desktop 接入需单独验证能力与会话关联，不与 Baize 自身 UI 的开发混为一项。

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

不要假设所有 agent 或同品牌的不同执行入口都支持同样能力。下列类型只是基础能力示意，不表示当前实现已经覆盖 Desktop。

目标能力声明还需说明原生 prompt、项目指令、MCP、skills、computer use、browser use、会话控制、事件回流和权限交互的支持范围。区分未验证、不支持和已验证，并注明执行入口；缺失能力不能静默被更低能力的入口替代。

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

- 所选执行入口是否存在或可连接（当前 MVP 检查 CLI）；
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

路由支持持续会话，MVP 使用规则与用户显式选择：

- 可继续时保持当前 agent；
- 用户可因为能力、效果、模型或 prompt 差异主动更换执行者；
- health、quota、rate limit 和失败信号可以提示候选，不是唯一切换理由；
- 推荐需结合实际入口能力与用户要求，不仅按可调用性选择；
- 缺少所需工具或无法承接会话时说明缺口，不静默降级；
- 具体切换是否执行由用户模式和授权决定，切换记录归原 Baize session。

不以自动判断“哪家更擅长某类任务”作为 MVP 连续性成立的前提。

### 5.7 Sticky routing

MVP 必须支持 sticky routing。

默认：

```text
同一 Baize session 内默认继续使用当前 provider；用户仍可在原 session 中显式更换执行者。
```

允许配置：

- sticky window；
- quota threshold；
- failure threshold；
- manual override。

### 5.8 切换上下文与 handoff

切换前应组合：

1. 当前 workspace、代码/文档引用、Git 修改与可用验证证据；
2. 近期相关原始对话，包括当前请求、用户纠正和下一步意图；
3. 仍然有效的较早目标与约束；
4. 必要时的摘要，注明其来源与局限。

保留消息角色、执行来源和顺序；agent 建议不能自动变成用户已确认要求。历史按预算选择，不默认全量转发，不把固定最后 N 条当作完整语义保证。

同 provider 原生续接可避免重复注入已有历史。切回旧 agent 时补充其最后已知位置之后的相关增量，并重新检查 workspace；原生 session ID 不证明它了解最新状态。

交接不能依赖原 agent 仍在线或有剩余额度。源不可用时，可从 Baize 记录和 workspace 准备上下文；必要信息不足时明确缺口。

默认在可确认的回合边界切换。进行中的工具调用需先结束或明确中断，核对部分修改；失败或结果未知时保留原 Baize session 与已知状态，不盲目重复有副作用的操作。

### 5.9 Workspace 与会话记录

MVP 应在原 Baize session 中关联记录：

- user prompts 与可取得的近期 agent 回复；
- selected provider、执行入口及原生 session 引用；
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

### 5.13 共享配置

用户通用要求以一份逻辑配置维护，Baize 自动向支持的 agent 应用相应配置，不要求用户逐家复制。优先复用项目已有的 `AGENTS.md`、MCP、skills 与 provider 配置。需要映射时，显示支持项、有效作用范围、未支持项与显式覆盖，不要求重复创建同义角色。

保留目标产品的内置 prompt、专属工具和配置，不通过统一系统提示词替换原生执行机制。共享配置减少设置负担，但不是跨 agent session 连续性的替代。

此项是目标要求；当前 MVP 尚未实现跨产品配置同步或所有入口的等效配置。

### 5.14 主对话连续

默认使用同一条 Baize 主对话、同一输入入口呈现工作，标明回答来自哪个 agent、在哪个执行环境完成。内部 handoff 是系统操作，不伪装成用户重新输入的消息。

底层原生 session 可多条，用户无需为每次切换管理新标签。读取历史和恢复 Baize session 后，仍能定位工作区、近期讨论、当前执行者与已知进度。

### 5.15 Provider Desktop 执行接入

在 provider 提供可用接口时，研究把其 Desktop 能力环境作为实际执行者接入。验收需证明目标能力确实可用，并将可取得的输入、输出、状态与审批关联回原 Baize session。

仅打开应用、共享认证、使用同品牌模型或启动相关 CLI，都不能单独满足该要求。无法控制或回流的部分应标明；不承诺转移原生工具进程和内部状态。

## 6. 最终用户视角

### 6.1 第一次使用

用户选择本地 workspace，复用已有项目指令、MCP、skills 与 provider 配置。界面显示可用 agent、实际执行入口和已知能力差异，不要求先配置多个角色。

### 6.2 开始讨论和开发

用户进入或新建 Baize session，可以先讨论方案再编码。主对话、目标与 workspace 始终关联，当前 agent 与实际执行入口可见。

例如：“帮我拆分 auth 模块，先讨论方案。保持公共 API，不引入新数据库。”

### 6.3 持续反馈

用户查看回答、diff、文档和验证结果，继续纠正或补充要求。Baize 保留近期对话与有效约束，不要求用户先把讨论整理成正式文档。

### 6.4 选择另一位执行者

用户因为效果、能力、额度或可用性选择切换，例如：“换 Codex 接着实现刚才确认的方案。”

Baize 保持原 session，说明目标入口与能力差异，在可确认的边界组合 workspace 与相关对话。摘要按需要辅助，原 agent 不可用不应阻止准备交接。

### 6.5 接手与切回

目标 agent 接收当前修改、有效约束和近期意图，继续回答原请求。用户仍在同一个 Baize session 中交流。

再切回原 agent 时，Baize 补上它离开期间的新要求、对话与 workspace 变化，避免它按旧状态重做或覆盖工作。

如果在 provider Desktop 中执行，应证明使用的是所选能力环境，并通过已支持的接口回流会话信息；无法回流的部分明确显示。

### 6.6 完成与恢复

用户检查最终 diff、文档、测试证据和未解决事项后决定完成。后续可恢复该 Baize session；记录的结果需与当前 workspace 核对，不能把旧测试信号当成最新状态。

不以 handoff 次数、路由规则数量或历史收集规模作为任务完成的依据。

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
- file change summary；
- 可用的近期对话、有效约束和 provider 原生 session 引用。

可恢复不等于已有执行可以无条件重放；部分完成和结果未知时先核对。

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

- `apps/desktop/`: 未来 Baize desktop app（框架待原型验证）；
- `crates/baize-core/`: 核心领域模型、task session、routing、handoff、quota 抽象；
- `crates/baize-daemon/`: headless daemon，本地 API、事件流和生命周期管理；
- `crates/baize-tui/`: ratatui TUI 客户端；
- `crates/baize-acp/`: ACP client、transport、capability mapping；
- `crates/baize-adapters/`: Codex、Antigravity、OpenCode、Copilot 等 provider 的执行入口适配；
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
- 复杂 AI-based routing；
- 跨产品完整复制私有 session、内部工具状态或后台进程；
- 把复杂任务管理、角色配置或决策卡片作为持续会话的前提。

Provider Desktop 执行属于产品目标，当前 CLI MVP 不宣称已实现；Baize 自身的 Desktop UI 仍独立排期。

## 10. 已回答的产品决策

### 10.1 第一版 UI

当前版本提供基于 Ratatui 的持续会话工作台，包含对话恢复、编辑、查找和检查视图；设计与验收见 [tui-design.md](tui-design.md)。未来 UI 复用相同 daemon，Desktop 框架候选见 [ui-research.md](ui-research.md)。

这源于最初 CLI 为主的接入条件，不代表用户只需要终端界面，也不证明 CLI 拥有 Desktop 的原生 prompt、MCP、skills、computer/browser use。Provider Desktop 执行接入不要求先开发 Baize 自身的 Desktop UI。

### 10.2 Adapter 接入原则

按实际能力选择官方原生接口、ACP 或 CLI。能保留更多所需能力的官方会话接口优先；ACP 满足需求时复用标准协议；CLI 提供兼容执行路径。

接口优先级不能替代验证：需确认 session 控制、结果/事件回流、workspace、配置、权限与原生工具。发生能力变化或必须改用其他入口时说明原因，不静默声称等效。

### 10.3 Session 状态持久化和引用

需要调研每个 agent 的具体机制。

Baize 不应假设所有 agent 都有统一 session store。应在 Baize 内部维护自己的 task session，同时把 provider session id、resume command、transcript location 或 checkpoint location 作为外部引用保存。

### 10.4 Handoff summary

摘要按上下文长度、预算和人工检查需要生成。基本接续材料是当前 workspace 与近期相关对话，再补充仍然有效的约束。

原 agent 可用时可以提供摘要，Baize 附加工作区事实与验证证据；不可用时从已有记录准备交接。目标不要求原生 transcript 导入，不能依赖读取私有存档完成完整状态迁移。

摘要应保留来源、用户要求与 agent 建议的区别，失效内容不能作为当前指令。它不取代主对话，也不是额外维护的必填任务文档。

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

## 11. 仍需验证的问题

- 各执行入口实际提供的内置 prompt、MCP、skills、computer/browser use 是否可用；
- provider Desktop 的官方接口能否控制会话，并回流输入、输出、状态和审批；
- 近期对话选择如何保留用户纠正和较早但仍有效的约束；
- A → B → A 时如何定位已知上下文并补充增量，处理外部 workspace 修改；
- 原 agent 不可用、目标启动失败或结果未知时如何保持 session 连续；
- 通用配置能复用哪些现有机制，哪些需要 provider 专属映射；
- 与用户当前原生工具流程相比，重述、重复尝试和能力损失是否减少。

历史 provider 观察见 `research.md` 和 `provider-context-research.md`；其中的日期与未知项不因本次产品理念更新而视为已重新验证。

## 12. 连续会话验收

| 场景 | 期望结果 |
|---|---|
| A → B | Baize session ID 与主对话保持，B 同时获得当前 workspace 和近期相关对话 |
| A → B → A | A 补齐离开期间的新要求与工作区变化，不只恢复旧原生 session |
| 较早约束仍有效 | 超出近期窗口的关键要求仍被保留，已失效要求不继续作为指令 |
| 原 agent 额度耗尽或失联 | 不依赖源 agent 生成摘要，也能准备交接或明确缺失项 |
| 目标启动失败或结果未知 | 保留原 session 和已知执行状态，不盲目重跑有副作用的操作 |
| 共享配置 | 可检查支持项、作用范围与未支持项，保留 provider 专属 prompt 和工具 |
| Provider Desktop 接入 | 证明工作在声明的能力环境中执行；仅启动窗口不算完成 |
| 用户效果 | 与原生工具及手工交接基线比较，减少重述、纠偏与重复尝试，产物质量和关键能力不下降 |

上述是产品验收要求。当前实现中的 session/resume、上下文包、事件日志和 handoff 是基础能力，不作为全部场景已经通过的声明。
