# Baize Technical Selection

## 1. 目标

Baize 第一版是 TUI，但未来需要支持 desktop app，并且尽量不迁移内核。

技术选型要满足：

- 一套核心内核，多种 UI；
- 支持 macOS、Linux、Windows；
- Windows 优先支持 WSL 工作流，也保留原生 Windows 可能性；
- 优先使用 ACP 连接 coding agent；
- 便于集成 Codex、Gemini CLI、GitHub Copilot CLI、OpenCode；
- 适合本地 daemon、agent subprocess、pty、文件系统、git diff、SQLite、事件流；
- 未来可扩展到 desktop app、web UI、IDE integration。

## 2. 推荐结论

已确认建议：

```text
Rust kernel
+ local daemon
+ JSON-RPC/HTTP API
+ SQLite storage
+ ACP-first adapter layer
+ ratatui TUI client
+ future Tauri desktop app as thin client
```

UI 与内核分离：

```text
baize daemon
  本地 headless kernel
  workspace/session/routing/quota/handoff/adapters

baize tui
  第一版主 UI
  通过本地 API/事件流连接 daemon

baize desktop
  未来 desktop app
  复用同一个 daemon
```

## 3. 语言与生态对比

Baize 的长期内核会处理本地文件、shell command、agent subprocess、session transcript、权限策略和跨平台打包。这里的风险不只是“跑得快不快”，还包括：

- 供应链攻击面；
- runtime 体积；
- 本地权限边界；
- pty 和 process 管理；
- 文件系统一致性；
- crash recovery；
- desktop sidecar 分发；
- Windows/WSL 兼容。

因此，不建议把长期内核绑定在 Node.js 上。TypeScript 仍可用于未来 desktop renderer、配置工具或 adapter prototype，但不建议作为 Baize core runtime。

### 3.1 Rust

推荐级别：

```text
Primary recommendation
```

优势：

- 性能好，适合持续运行的 local daemon；
- 内存安全强，适合处理本地文件、命令、权限和长期事件流；
- 单 binary 分发能力好，适合 macOS/Linux/Windows/WSL；
- Tauri 生态天然使用 Rust，未来 desktop sidecar 和 app shell 更顺；
- `ratatui`/`crossterm` TUI 生态成熟，可做高质量终端界面；
- `tokio` 异步生态适合 subprocess、stream、HTTP、WebSocket、SSE；
- `serde`/`schemars` 适合定义稳定事件、配置、API schema；
- `sqlx`/`rusqlite` 可支撑 SQLite；
- `git2` 或调用 git CLI 可支撑 workspace state；
- `portable-pty` 等库可处理跨平台 pty。

劣势：

- 开发速度慢于 TypeScript/Go；
- 泛型、生命周期和 async 边界会提高贡献门槛；
- ACP TypeScript SDK 不能直接复用，需要根据协议实现 client，或生成/移植协议类型；
- OpenCode JS/TS SDK 不能直接复用，native fallback 可能需要 HTTP/server API 或 CLI。

适合 Baize 的原因：

```text
Baize 是本地高权限 supervisor，Rust 的安全性、分发和 Tauri 兼容性比 Node 的生态便利更重要。
```

### 3.2 Go

推荐级别：

```text
Strong alternative
```

优势：

- 开发效率高于 Rust；
- 静态 binary 分发简单；
- 跨平台网络、文件、process、HTTP API 很成熟；
- goroutine/channel 适合 daemon、事件流和 adapter lifecycle；
- 供应链复杂度通常低于 Node；
- `bubbletea`/`lipgloss`/`bubbles` TUI 生态成熟，适合快速做漂亮 TUI；
- SQLite、JSON-RPC、OpenAPI、git CLI 集成成本低。

劣势：

- 内存安全不如 Rust，仍有 data race、nil panic、slice 边界等风险；
- Tauri 生态不如 Rust 顺，未来 desktop sidecar 仍可行，但不是同语言优势；
- 高质量 terminal diff/review UI 可做，但 ratatui 的低层控制力更强；
- ACP client 也需要自行实现或移植。

适合 Baize 的原因：

```text
如果更看重实现速度、维护简单和团队易上手，Go 是最务实的选择。
```

### 3.3 Zig

推荐级别：

```text
Not recommended for MVP
```

优势：

- 性能和可控性强；
- 交叉编译能力好；
- runtime 小；
- 适合安全敏感的底层组件。

劣势：

- TUI、HTTP、SQLite、JSON-RPC、pty、desktop integration 生态都不如 Rust/Go；
- 招募贡献者和长期维护成本更高；
- Baize 当前主要难点在 agent integration 和产品闭环，不适合把大量精力放到底层基础设施。

### 3.4 Python

推荐级别：

```text
Prototype only
```

优势：

- 原型快；
- subprocess、文本处理、SQLite、prompt/handoff 实验方便；
- 可用于调研脚本和离线工具。

劣势：

- 分发、性能、依赖隔离和供应链风险都不适合作为长期本地 daemon；
- TUI 可以做，但跨平台稳定性和桌面 sidecar 体验不如 Rust/Go。

### 3.5 TypeScript / Node.js

推荐级别：

```text
Adapter prototype / desktop renderer only
```

优势：

- ACP 官方示例和常见 SDK 更容易复用；
- OpenCode JS/TS SDK 可直接使用；
- JSON-RPC、schema、web UI、desktop renderer 生态强；
- 早期 adapter prototype 很快。

劣势：

- Node runtime 和 npm 依赖树会扩大供应链攻击面；
- 单 binary 分发和离线安装体验较弱；
- 本地高权限 daemon 的安全边界较难收紧；
- 长期运行的 subprocess/pty/file watcher 复杂度容易积累；
- 性能通常够用，但 tail latency、内存占用和依赖冷启动不如 Rust/Go 可控；
- Windows native 场景下 shell、pty、path、node-gyp 等问题更多。

适合 Baize 的位置：

```text
不建议作为 core。
可以用于 desktop frontend、schema playground、adapter spike、文档站或开发工具。
```

## 4. 推荐方案

### 4.1 首选：Rust core + ratatui TUI + Tauri desktop

推荐架构：

```text
Rust daemon/core
+ Rust ACP client
+ Rust adapter abstraction
+ SQLite
+ ratatui TUI
+ HTTP/SSE or JSON-RPC local API
+ future Tauri desktop shell
```

这是性能、平台移植性、安全性综合最平衡的方案。

关键理由：

- core 和 future desktop shell 都可以围绕 Rust/Tauri 展开；
- TUI 使用 `ratatui`，长期可以做复杂 panes、diff、logs、status dashboard；
- daemon 是单 binary，适合本地工具分发；
- 对本地文件和命令执行这类高权限能力，Rust 比 Node 更合适；
- ACP 是 JSON-RPC 协议，Baize 可以自己实现 client，不必依赖 TypeScript SDK；
- 对 OpenCode 这类有 server API 的 agent，可以走 HTTP/native fallback。

### 4.2 次选：Go core + Bubble Tea TUI + Tauri/Electron desktop

推荐场景：

```text
如果更重视开发速度、团队易上手和维护简单，选择 Go。
```

Go 的优势是非常务实：daemon、HTTP API、cross compile、TUI 都很成熟。代价是安全性和 desktop 同构性略弱于 Rust。

### 4.3 不建议：Node core + Ink TUI

Node/TypeScript 适合快速验证 ACP 和 OpenCode SDK，但不适合 Baize 的长期内核。

如果需要利用 TS 生态，可以做成独立工具：

```text
tools/
  acp-spike/
  opencode-sdk-spike/
  schema-playground/
```

## 5. TUI 选型

### 5.1 Rust ratatui

推荐级别：

```text
Primary recommendation
```

优势：

- 高性能；
- 跨平台；
- 控制力强；
- 适合复杂 dashboard、multi-pane、logs、diff summary、status bar；
- 配合 Rust daemon 可共享 domain types；
- 未来可把 TUI 作为同一个 workspace binary 的模式之一。

劣势：

- UI 开发速度慢于 React Ink；
- 需要更多手写状态管理和布局逻辑；
- 对贡献者要求更高。

适合原因：

```text
Baize 的 TUI 是主产品入口，不只是 demo。ratatui 更适合长期打磨。
```

### 5.2 Go Bubble Tea

推荐级别：

```text
Strong alternative
```

优势：

- 开发体验好；
- TUI 生态成熟；
- 很适合 command palette、list、form、status、log view；
- Go binary 分发简单。

劣势：

- 如果 core 选 Rust，TUI 用 Go 会形成双语言；
- 复杂 diff/review UI 控制力略弱于 ratatui；
- 与未来 Tauri/Rust desktop sidecar 不同构。

### 5.3 TypeScript Ink

推荐级别：

```text
Prototype only
```

优势：

- React mental model；
- 开发快；
- 适合早期 spike。

劣势：

- Node runtime 和 npm 依赖树增加攻击面；
- terminal 控制力和长期稳定性不如 ratatui/Bubble Tea；
- 如果 core 不是 Node，会引入额外 runtime。

### 5.4 Python Textual

推荐级别：

```text
Prototype / internal tool only
```

优势：

- UI 原型快；
- widget 丰富。

劣势：

- 分发和 runtime 依赖不适合 Baize 长期主入口；
- 高权限本地 agent supervisor 不宜依赖 Python 虚拟环境。

## 6. 架构边界

核心原则：

```text
UI 不能持有核心业务状态。
所有 workspace/session/routing/handoff 状态属于 daemon。
```

### 6.1 Daemon

职责：

- workspace registry；
- task session manager；
- route decision engine；
- provider registry；
- ACP/native/CLI adapter lifecycle；
- quota/health observer；
- workspace state tracker；
- handoff generator；
- checkpoint policy；
- event log；
- local API。

### 6.2 TUI

职责：

- 展示 workspace；
- 输入 prompt；
- 展示 agent stream；
- 展示 permission prompts；
- 展示 quota/health；
- 展示 route explanation；
- 展示 diff/test/handoff；
- 调用 daemon API。

TUI 不直接调用 agent。

### 6.3 Desktop App

未来 desktop app 是 daemon 的另一个 client。

推荐形态：

```text
Tauri desktop shell
+ web frontend
+ Baize daemon sidecar
```

原因：

- Tauri 跨平台体积较轻；
- 适合把 Baize daemon 作为 sidecar；
- desktop UI 可以复用 daemon API；
- 不需要迁移核心业务逻辑。

如果未来需要更快实现复杂 UI，也可考虑 Electron，但仍应保持 daemon 独立，避免核心逻辑进入 renderer。

## 7. Storage 选型

推荐：

```text
SQLite
```

用途：

- workspace；
- task session；
- route decision；
- provider health；
- quota samples；
- command log；
- file change snapshot；
- handoff summary；
- event log。

大文件和 transcript 不直接塞入 SQLite，可保存引用：

```text
~/.baize/workspaces/<workspace-id>/
  events.db
  handoffs/
  transcripts/
  checkpoints/
```

## 8. API 选型

Daemon 暴露本地 API：

```text
HTTP JSON API
+ Server-Sent Events or WebSocket event stream
```

MVP 可以先用 HTTP + SSE。

未来 desktop、web、IDE bridge 都可复用。

内部 adapter 与 agent 之间：

```text
ACP over stdio
ACP over TCP/HTTP if provider supports it
native SDK
CLI fallback
```

## 9. Config 选型

推荐使用：

```text
TOML
```

建议第一版使用 TOML：

- 用户可写注释；
- Rust/Go 生态支持成熟；
- 对本地工具配置友好；
- 比 JSONC 更少依赖自定义解析行为。

配置位置：

```text
~/.config/baize/config.toml
<workspace>/.baize/config.toml
```

workspace 配置覆盖 global 配置。

## 10. 跨平台策略

### 10.1 macOS

原生支持。

### 10.2 Linux

原生支持。

### 10.3 Windows

分两层：

```text
Preferred: WSL
Native: best-effort
```

WSL 优先原因：

- coding agent、shell、git、包管理器和 dev tools 在 Linux 环境更一致；
- OpenCode 官方也建议 Windows 用户使用 WSL 获得更好的文件系统性能、终端支持和开发工具兼容性；
- Baize 的 workspace、agent subprocess、pty、shell policy 更容易统一。

Windows native 需要额外处理：

- PowerShell/CMD 差异；
- path normalization；
- pseudo terminal；
- symlink；
- file watching；
- shell command policy；
- agent 安装路径。

## 11. Packaging

MVP：

```text
single native binary
```

命令：

```text
baize
baize daemon
baize tui
```

后续：

- Homebrew；
- install script；
- desktop installer；
- Windows WSL setup guide。

## 12. 推荐依赖方向

核心：

- Rust stable；
- tokio；
- serde；
- tracing；
- clap；
- config/TOML parser；
- JSON-RPC implementation；
- SQLite driver；
- pty/process management；
- git integration。

TUI：

- ratatui；
- crossterm；
- keyboard input；
- terminal layout；
- log viewer；
- external pager integration。

Desktop future：

- Tauri；
- web frontend;
- same daemon API；
- bundled sidecar。

## 13. 风险与应对

### 13.1 Rust 开发速度慢

应对：

- 先实现最小 daemon、workspace、ACP client 和 TUI shell；
- adapter 做 capability-first，不追求一次性完美；
- 对不确定 agent 能力用 CLI fallback；
- 用 schema 和事件模型控制复杂度。

### 13.2 ACP TypeScript SDK 不能直接复用

应对：

- ACP 是 JSON-RPC 协议，可用 Rust 实现 client；
- 先支持 MVP 所需方法；
- 保持 ACP transport 和 capability mapping 独立；
- 如有必要，用 codegen 从协议 schema 生成类型。

### 13.3 OpenCode JS/TS SDK 不能直接复用

应对：

- 首选 `opencode acp`；
- native fallback 优先走 OpenCode server HTTP API；
- 最后再走 CLI fallback。

### 13.4 ACP 协议和 provider 支持仍在变化

应对：

- adapter 分层；
- capability detection；
- provider-specific fallback；
- route policy 不依赖某个 provider 的非标准能力。

### 13.5 Windows native 复杂

应对：

- 第一阶段明确 WSL 优先；
- native Windows 标记为 best-effort；
- daemon 内部统一 path 和 shell abstraction。

## 14. 初始实现建议

第一阶段目录可扩展为：

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
```

实现顺序：

```text
1. daemon skeleton + config + event log
2. workspace detection + git state
3. ACP client abstraction
4. provider registry
5. one provider adapter
6. TUI shell
7. route decision records
8. handoff summary pipeline
9. quota/health observer
```

## 15. 已确认决策

已确认：

```text
Use Rust as Baize core language.
Use ratatui for first TUI.
Use Tauri for future desktop shell.
Use TOML for configuration.
Keep TypeScript only for optional frontend/prototype tooling.
```
