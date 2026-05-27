# Baize Vision

## 一句话愿景

Baize 是一个本地 workspace shell，用统一界面调度多个已安装的 coding agent，让用户在同一个代码工作区里持续完成任务，而不再被模型配额、rate limit、上下文丢失和手动切换打断。

## 背景

越来越多开发者同时使用 Claude Code、Codex、Gemini CLI、Cline、Aider 等 coding agent。它们各有长处，也各有配额、延迟、上下文窗口和交互习惯。

当前问题不是“没有足够强的模型”，而是：

- agent 之间缺少统一工作区状态；
- 用户需要手动判断哪个 agent 还能用；
- quota 或 rate limit 到来时，任务容易中断；
- 切换 agent 时，需要复制 prompt、解释上下文、搬运 diff；
- 多个 agent 修改同一个 repo 时，缺少统一追踪与交接。

Baize 的目标是把这些 agent 编织成一个稳定的本地工作层。

## 命名：为什么是白泽

“白泽”是中国古代传说中通晓万物、能识别各种异类并向人说明其特性的神兽。这个名字适合 Baize，因为这个项目的核心并不是制造又一个更强的 agent，而是帮助用户理解、辨认、调度和驯服已经存在的多个 coding agent。

在产品语义上，白泽对应几层含义：

- 识别：识别不同 agent 的能力边界、健康状态、quota 状态和适用任务；
- 指引：在复杂任务中给出可解释的 route decision，而不是让用户盲选模型；
- 守护：在 quota、rate limit、上下文漂移和多 agent 修改冲突出现前提醒用户；
- 交接：在一个 agent 无法继续时，把任务状态清楚交给下一个 agent；
- 知万物而不替万物：Baize 不取代 Claude Code、Codex、Gemini CLI、Cline 等工具，而是在它们之上提供统一的认知层和调度层。

因此，“白泽”既是项目名，也是应用名。英文写作 `Baize`，保留中文意象，同时便于作为 CLI、包名、进程名和产品标识使用。

## 产品定位

Baize 不是通用 LLM API router，也不是又一个独立 coding agent。

它更像：

```text
Workspace-native agent supervisor
```

也可以理解为：

```text
Agent runtime load balancer for local coding workspaces
```

它负责：

- 观察每个 agent 的健康状态、配额状态和近期表现；
- 根据任务类型、当前上下文、quota 和用户偏好选择合适 agent；
- 在一个任务内尽量保持 sticky routing；
- 当必须切换时，生成可靠的 handoff summary；
- 维护独立于任何单一 agent 的 workspace memory；
- 给用户一个统一、可解释、可恢复的工作体验。

## 核心原则

### 1. Workspace first

Baize 的中心不是模型，也不是聊天窗口，而是本地 workspace。

所有状态都围绕工作区组织：

- repo path；
- branch；
- file changes；
- command history；
- test results；
- task objective；
- agent sessions；
- handoff notes。

### 2. Sticky before clever

路由策略优先保证上下文稳定，而不是每轮都寻找理论最优 agent。

默认策略：

```text
同一任务优先继续使用同一个 agent。
只有 quota、rate limit、health failure 或用户明确要求时才迁移。
```

### 3. Handoff is a first-class artifact

agent 切换不是简单把聊天历史转发给下一个 agent。

Baize 应该生成结构化交接材料：

- 当前目标；
- 已完成工作；
- 未完成计划；
- 修改文件；
- 当前 diff 摘要；
- 已运行命令；
- 测试结果；
- 失败原因；
- 下一步建议；
- 用户偏好与约束。

### 4. Explain every important route

用户需要知道为什么系统选择、继续或切换某个 agent。

例如：

```text
继续使用 Claude，因为当前任务仍在同一 refactor session 内，quota 充足。
```

或：

```text
建议切换到 Codex，因为 Claude quota 已低于阈值，当前任务进入测试修复阶段。
```

### 5. Prefer supervision over replacement

Baize 不试图替代 Claude Code、Codex、Gemini CLI、Cline 等工具。

它应该复用用户已经安装、配置和信任的 agent，并在它们之上提供：

- 调度；
- 状态；
- 记忆；
- 恢复；
- 交接；
- 风险控制。

## 目标用户

### 个人开发者

已经在日常开发中使用多个 coding agent，希望减少切换成本和上下文损耗。

### 高频 agent 用户

经常遇到 quota、rate limit、token exhaustion，希望任务不中断。

### 小型工程团队

希望在统一工作区内观察 agent 修改、记录决策、降低多人或多 agent 协作风险。

### AI tooling power users

愿意配置 provider、policy 和 routing profile，追求更高的开发吞吐。

## 最终用户视角的理想效果

用户打开 Baize，选择一个本地 repo，然后直接描述任务：

```text
帮我把这个认证模块重构成 provider-based 架构，并补测试。
```

Baize 自动判断：

- 当前 repo 类型；
- 最近修改文件；
- 可用 agent；
- agent quota；
- 任务类型；
- 是否存在未完成 session。

然后选择一个 agent 开始工作，并在 UI 中显示：

```text
Using Claude Code
Reason: large refactor, current quota healthy, no active conflicting session.
```

一段时间后，如果 Claude quota 接近阈值，Baize 不会突然静默切换，而是提示：

```text
Claude quota is low. This task is now mostly test/debug work.
Recommended: continue in Codex.

[Continue Claude] [Switch to Codex] [Autopilot this task]
```

用户选择切换后，Baize 自动生成 handoff：

```text
Codex 接手时已经知道：
- 哪些文件改过；
- 为什么这样改；
- 哪些测试失败；
- 下一步应该修哪里；
- 用户要求不要改公共 API。
```

用户感受到的是：

```text
我一直在同一个 workspace 里推进任务。
背后换过 agent，但任务没有断。
```

## 长期形态

成熟后的 Baize 可以包含：

- 本地 daemon；
- desktop 或 web workspace UI；
- CLI；
- agent adapter SDK；
- policy/routing engine；
- quota observer；
- workspace memory store；
- diff/checkpoint manager；
- handoff generator；
- provider health dashboard；
- team audit log；
- plugin marketplace for adapters；
- IDE integrations。

## 非目标

短期内 Baize 不追求：

- 替代现有 coding agent；
- 直接训练或托管模型；
- 做成纯 API gateway；
- 完美抽象所有 agent 能力；
- 对 provider quota 做不可靠的精确承诺；
- 在用户不知情的情况下执行高风险自动切换。

## 成功标准

Baize 成功的标志不是“支持多少模型”，而是：

- 用户能在一个 UI 里稳定完成跨 agent 的 coding task；
- quota/rate limit 不再导致任务突然中断；
- agent 切换后的上下文恢复质量足够好；
- 用户能理解每次 route decision；
- workspace diff、命令和测试结果可追踪；
- 多 agent 修改同一 repo 时风险可控。
