# Baize Technical Spec

Version: 0.1.0
Status: review passed

## 1. 目标

本文描述 Baize 的 Rust 技术设计。目标是先把内核、TUI、adapter、storage、workspace tracking 和 handoff pipeline 的边界定义清楚，供 review 后再进入实现。

本 spec 遵循已确认技术方向：

```text
Rust core
+ ratatui TUI
+ local daemon
+ SQLite storage
+ ACP-first adapter layer
+ future Tauri desktop app
```

## 2. 总体架构

Baize 采用 headless daemon + thin client 架构。

```text
          baize-tui                 future desktop
        ratatui client              Tauri client
             │                           │
             └──────── local API ────────┘
                         │
                  baize-daemon
                         │
        ┌────────────────┼────────────────┐
        │                │                │
   baize-core      baize-workspace   baize-storage
        │                │                │
        └────────────────┼────────────────┘
                         │
                  baize-adapters
                         │
                    baize-acp
                         │
      ┌──────────────────┼──────────────────┐
      │                  │                  │
    Codex            Gemini CLI       Copilot CLI
                                           │
                                      OpenCode
```

核心原则：

- UI 不直接调用 agent；
- daemon 持有 workspace/session/routing/handoff 状态；
- TUI 和未来 desktop 都通过同一套 local API 与 daemon 交互；
- adapter 优先使用 ACP，必要时 fallback 到 native/server/CLI；
- 所有重要动作都落 event log；
- provider-specific 状态只作为外部引用保存，不作为 Baize 的唯一事实来源。

## 3. Rust Workspace

建议目录：

```text
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
```

### 3.1 `baize-core`

核心领域模型和纯业务逻辑。

包含：

- `WorkspaceId`
- `TaskSessionId`
- `ProviderId`
- `AgentRunId`
- `TaskSession`
- `ProviderProfile`
- `ProviderHealth`
- `QuotaState`
- `RouteDecision`
- `HandoffSummary`
- `PermissionRequest`
- `CommandPolicy`
- routing policy trait
- handoff policy trait

不包含：

- SQLite 细节；
- HTTP server；
- terminal UI；
- provider subprocess；
- git command 执行。

### 3.2 `baize-daemon`

本地 headless kernel。

包含：

- daemon lifecycle；
- local API；
- event stream；
- service wiring；
- provider process lifecycle；
- task session orchestration；
- route execution；
- handoff execution；
- permission prompt coordination。

### 3.3 `baize-tui`

第一版主 UI。

包含：

- workspace selector；
- task session list；
- prompt input；
- agent stream viewer；
- route decision panel；
- provider health/quota panel；
- changed files panel；
- command/test result panel；
- permission prompt modal；
- handoff preview。

TUI 只调用 daemon API，不直接读写 workspace，也不直接启动 agent。

### 3.4 `baize-acp`

ACP client 实现。

包含：

- stdio transport；
- JSON-RPC framing；
- initialize/auth/session lifecycle；
- prompt request；
- session updates；
- permission callbacks；
- file/terminal client capabilities；
- capability mapping；
- protocol error normalization。

第一版只实现 Baize MVP 所需 ACP 子集，避免一次性追完整协议。

### 3.5 `baize-adapters`

provider adapter 实现。

第一批 provider：

```text
codex
gemini
copilot
opencode
```

每个 adapter 可分三层：

```text
ACP adapter
native/server adapter
CLI fallback adapter
```

同一个 provider 可以同时暴露多个 transport，daemon 根据 capability 和配置选择。

### 3.6 `baize-workspace`

workspace state tracking。

包含：

- git root detection；
- branch/status detection；
- dirty state；
- changed files；
- diff summary；
- command execution wrapper；
- test script detection；
- checkpoint hooks；
- file snapshot references；
- hunk attribution helper。

### 3.7 `baize-storage`

SQLite 和本地文件存储。

包含：

- database schema；
- migrations；
- event log；
- repositories；
- file artifact store；
- transcript references；
- handoff artifact store；
- checkpoint references。

### 3.8 `baize-config`

TOML 配置。

包含：

- global config；
- workspace config；
- provider profiles；
- route policy；
- command policy；
- checkpoint policy；
- path normalization；
- config validation。

### 3.9 `baize-cli`

命令入口。

命令建议：

```text
baize
baize tui
baize daemon
baize status
baize doctor
baize config
baize providers
baize sessions
```

## 4. Workspace And Project

Baize 需要区分 `workspace` 和文件系统上的 `project`。

建议定义：

```text
Project:
  文件系统上的代码项目根。
  通常是一个 git repo root，也可能是 monorepo 中的一个 package/app 目录。

Workspace:
  Baize 对一个或多个 project 的操作上下文。
  包含 session、provider、routing、policy、event log、handoff、checkpoint、UI state。
```

### 4.1 为什么要区分

如果把 workspace 直接等同于 repo path，MVP 会简单，但长期会卡住：

- monorepo 中用户可能只想让 agent 操作某个 package；
- 一个任务可能跨多个 repo，例如 frontend + backend；
- 同一个 repo 可能有多个 Baize 工作上下文，例如不同 branch、不同 policy、不同 provider profile；
- Windows/WSL 下，同一个 project 可能有 Windows path 和 WSL path 两种表示；
- agent 的 cwd、git root、permission boundary、Baize session boundary 不总是同一个东西。

### 4.2 MVP 关系

MVP 中采用简化关系：

```text
1 workspace = 1 project = 1 git repo root or selected directory
```

但数据模型保留扩展空间：

```rust
struct Workspace {
    id: WorkspaceId,
    name: String,
    primary_project_id: ProjectId,
    policy_profile_id: PolicyProfileId,
    created_at: DateTime,
    updated_at: DateTime,
}

struct Project {
    id: ProjectId,
    workspace_id: WorkspaceId,
    root: PathBuf,
    kind: ProjectKind,
    vcs: VcsKind,
    trust_level: TrustLevel,
    created_at: DateTime,
    updated_at: DateTime,
}
```

### 4.3 Project Root

`project.root` 是 agent 实际操作文件系统时的默认边界。

它用于：

- provider cwd；
- git status/diff；
- command execution working directory；
- permission boundary；
- file watch；
- checkpoint；
- transcript/path references。

### 4.4 Workspace Root

`workspace` 不一定对应一个真实目录。它是 Baize 的逻辑容器。

它用于：

- task sessions；
- provider sessions；
- route decisions；
- handoff summaries；
- event log；
- UI state；
- user policy；
- project membership。

### 4.5 Permission Boundary

默认权限边界是：

```text
project.root
```

未来支持多 project workspace 时，权限边界可以是：

```text
allowed_roots = [project_a.root, project_b.root]
```

写入 allowed roots 外部路径必须确认或拒绝。

### 4.6 命名建议

用户界面可以优先使用 `workspace`，因为用户是在 Baize 中进入一个工作上下文。

技术层面必须保留 `project`，因为 agent、git、shell、filesystem 操作都需要具体 root path。

因此：

```text
用户看到：Workspace
系统建模：Workspace has Project(s)
MVP 实现：Workspace has exactly one Project
```

## 5. Local API

MVP 推荐：

```text
HTTP JSON API
+ SSE event stream
```

原因：

- TUI 和 future desktop 都容易接入；
- 调试简单；
- 不绑定 UI 技术；
- 可逐步扩展为 WebSocket 或 JSON-RPC。

### 5.1 API 草案

```text
GET  /health
GET  /workspaces
POST /workspaces
GET  /workspaces/:id/status

GET  /providers
GET  /providers/:id/health
POST /providers/:id/check

GET  /sessions
POST /sessions
GET  /sessions/:id
POST /sessions/:id/prompt
POST /sessions/:id/cancel
POST /sessions/:id/handoff

GET  /sessions/:id/events
GET  /sessions/:id/diff
GET  /sessions/:id/handoff/:handoff_id

POST /permissions/:id/approve
POST /permissions/:id/deny
```

### 5.2 Event Stream

事件通过 SSE 推给 TUI。

事件类型：

```text
workspace.status.changed
provider.health.changed
provider.quota.changed
session.created
session.route.decided
session.agent.started
session.agent.output
session.agent.tool_call
session.agent.completed
session.agent.failed
workspace.diff.changed
command.started
command.output
command.completed
permission.requested
permission.resolved
handoff.created
handoff.accepted
handoff.failed
```

每个事件都应包含：

```text
event_id
timestamp
workspace_id
session_id optional
provider_id optional
payload
```

## 6. Core Data Model

### 6.1 Workspace

```rust
struct Workspace {
    id: WorkspaceId,
    name: String,
    primary_project_id: ProjectId,
    created_at: DateTime,
    updated_at: DateTime,
}
```

### 6.2 Project

```rust
struct Project {
    id: ProjectId,
    workspace_id: WorkspaceId,
    root: PathBuf,
    kind: ProjectKind,
    vcs: VcsKind,
    active_branch: Option<String>,
    trust_level: TrustLevel,
    created_at: DateTime,
    updated_at: DateTime,
}
```

### 6.3 Task Session

```rust
struct TaskSession {
    id: TaskSessionId,
    workspace_id: WorkspaceId,
    objective: String,
    active_provider_id: Option<ProviderId>,
    status: TaskSessionStatus,
    stickiness: StickyPolicy,
    created_at: DateTime,
    updated_at: DateTime,
}
```

### 6.4 Provider Profile

```rust
struct ProviderProfile {
    id: ProviderId,
    kind: ProviderKind,
    display_name: String,
    priority: u32,
    transports: Vec<ProviderTransport>,
    capabilities: ProviderCapabilities,
    enabled: bool,
}
```

### 6.5 Provider Health

```rust
struct ProviderHealth {
    provider_id: ProviderId,
    status: HealthStatus,
    latency_ms: Option<u64>,
    last_error: Option<String>,
    checked_at: DateTime,
}
```

### 6.6 Quota State

```rust
struct QuotaState {
    provider_id: ProviderId,
    remaining_percent: Option<f32>,
    reset_eta_seconds: Option<u64>,
    confidence: QuotaConfidence,
    source: QuotaSource,
    observed_at: DateTime,
}
```

### 6.7 Route Decision

```rust
struct RouteDecision {
    id: RouteDecisionId,
    session_id: TaskSessionId,
    selected_provider_id: ProviderId,
    previous_provider_id: Option<ProviderId>,
    reason: String,
    confidence: f32,
    mode: RoutingMode,
    created_at: DateTime,
}
```

### 6.8 Handoff Summary

```rust
struct HandoffSummary {
    id: HandoffId,
    session_id: TaskSessionId,
    from_provider_id: ProviderId,
    to_provider_id: ProviderId,
    summary_markdown: String,
    mechanical_facts: HandoffFacts,
    status: HandoffStatus,
    created_at: DateTime,
}
```

## 7. Adapter Interface

Adapter trait 草案：

```rust
#[async_trait]
trait AgentProvider {
    fn id(&self) -> ProviderId;
    fn capabilities(&self) -> ProviderCapabilities;

    async fn health(&self) -> Result<ProviderHealth>;
    async fn quota(&self) -> Result<QuotaState>;

    async fn start_session(
        &self,
        input: StartSessionInput,
    ) -> Result<ProviderSessionRef>;

    async fn resume_session(
        &self,
        session_ref: ProviderSessionRef,
    ) -> Result<ProviderSessionHandle>;

    async fn prompt(
        &self,
        handle: ProviderSessionHandle,
        input: PromptInput,
    ) -> Result<AgentRunStream>;

    async fn request_handoff(
        &self,
        handle: ProviderSessionHandle,
        input: HandoffRequest,
    ) -> Result<HandoffDraft>;

    async fn cancel(
        &self,
        handle: ProviderSessionHandle,
    ) -> Result<()>;
}
```

Provider session reference 必须可序列化：

```rust
struct ProviderSessionRef {
    provider_id: ProviderId,
    transport: ProviderTransportKind,
    external_session_id: Option<String>,
    resume_command: Option<Vec<String>>,
    transcript_path: Option<PathBuf>,
    checkpoint_ref: Option<String>,
    metadata: JsonValue,
}
```

## 8. Routing

MVP 使用规则引擎。

默认流程：

```text
1. 如果 session 有 active provider
2. 检查 provider health
3. 检查 quota threshold
4. 检查 sticky window
5. 如果仍可用，继续使用
6. 如果不可用或低于阈值，生成候选 provider
7. 生成 route decision
8. 根据用户模式 manual/assisted/autopilot 执行或等待确认
```

Routing 输入：

- task objective；
- active provider；
- provider priority；
- provider health；
- quota state；
- task type；
- workspace dirty state；
- last failures；
- user policy；
- provider capabilities。

## 9. Handoff Pipeline

Handoff 分两部分：

```text
Agent-generated summary
+ Baize mechanical facts
```

流程：

```text
1. Baize 收集 workspace facts
2. Baize 请求当前 agent 生成 handoff draft
3. Baize 附加机械事实
4. Baize 保存 handoff artifact
5. 用户确认或 policy 自动确认
6. Baize 将 handoff 注入目标 provider prompt
7. route decision 记录迁移原因
```

Mechanical facts 包括：

- changed files；
- git diff summary；
- commands run；
- test result；
- route history；
- provider errors；
- checkpoint references；
- user constraints。

## 10. Storage Design

SQLite 表草案：

```text
workspaces
projects
task_sessions
provider_profiles
provider_session_refs
provider_health_samples
quota_samples
route_decisions
agent_runs
events
commands
file_changes
handoffs
permissions
checkpoints
```

本地 artifact：

```text
~/.local/share/baize/
  baize.db
  workspaces/
    <workspace-id>/
      handoffs/
      transcripts/
      checkpoints/
      snapshots/
```

配置：

```text
~/.config/baize/config.toml
<workspace>/.baize/config.toml
```

## 11. Permission And Command Policy

命令权限等级：

```text
ask
allow_safe
allow_project
deny
```

高风险操作永远需要确认：

- 删除大量文件；
- 写 allowed project roots 外部路径；
- `git push`；
- `git reset --hard`；
- 修改 shell/profile；
- 修改 SSH/GPG/keychain；
- package publish；
- cloud deploy。

Baize 的 permission model 应独立于 provider。provider 自己也有权限机制时，Baize 只做上层约束，不假设 provider 机制完全可靠。

## 12. Cross-platform

支持目标：

```text
macOS: native
Linux: native
Windows: WSL preferred, native best-effort
```

路径策略：

- core 内部区分 workspace id、project root 和 provider cwd；
- adapter transport 保存原始 command；
- Windows native 单独处理 PowerShell/CMD；
- WSL 下优先在 Linux filesystem 中运行 workspace。

## 13. MVP Build Plan

建议实现顺序：

```text
1. Cargo workspace skeleton
2. baize-core domain types
3. baize-config TOML loader
4. baize-storage SQLite event log
5. baize-workspace git status/diff summary
6. baize-daemon local API + SSE
7. baize-acp stdio JSON-RPC transport
8. Codex/Gemini adapter validation
9. baize-tui session dashboard
10. routing decision records
11. permission prompt loop
12. handoff artifact pipeline
```

第一批 provider 建议：

```text
1. Codex
2. Gemini
3. Copilot
4. OpenCode
```

说明：产品和验证优先级都从 Codex/Gemini 开始。若 Codex/Gemini 的 ACP 入口或 session 控制能力不足，adapter 层应立即记录 capability gap，并通过 native/server/CLI fallback 继续验证 MVP 闭环。

## 14. Review Decisions

已确认：

- local API 第一版使用 HTTP + SSE；
- 第一条 adapter 验证路径选择 Codex/Gemini；
- SQLite schema 先做 append-only event log，再补 query tables；
- TUI 第一版只做单 workspace，暂不做多 workspace 切换；
- handoff 在 MVP 中允许先保存 markdown artifact，再做结构化字段抽取；
- desktop app 只保留目录，不进入 MVP。

## 15. Open Review Questions

待确认：

- MVP 是否允许用户手动选择 project root，而不是总是自动使用 git root；
- monorepo 中是否需要第一版支持 project subdir；
- Codex/Gemini fallback 是优先 programmatic CLI，还是优先复用它们自己的 session/resume 机制；
- event log 的第一版是否需要对 payload 做 schema version；
- permission policy 是否需要区分 read boundary 和 write boundary。
