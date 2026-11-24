# 可靠的跨 Agent 连续会话

2026-11-25，跟踪 [issue #10](https://github.com/songdragon/baize/issues/10)。在现有 CLI adapters、daemon 和 TUI 中承接同一段工作，不涉及 provider Desktop 集成或共享配置同步。

## 日常操作

先开始或打开一个会话，在输入框提交以下本地命令。这些命令由 Baize 处理，不调用模型，也不切换执行者。

```text
/remember api 保持现有公开函数签名，只使用标准库
/remember language 用户可见回答使用中文
/requirements
/remember language 代码注释使用英文，最终回答使用中文
/forget language
```

相同 key 表示替换；撤销保留修订号和来源，不删除审计依据。长期要求需要显式保存，Baize 不从 agent 建议中自动推断用户承诺。原始对话仍按用户/assistant、provider 和事件来源承接。原会话目标保留为背景，后续用户修正和当前有效要求优先。

Tab 选择下一轮执行者，Enter 仍使用原 Baize session。也可 Ctrl-H 准备交接、Ctrl-Y 接受。接受只选择执行者，不发送草稿、不推进回执。Ctrl-H 和 F5 都使用请求时的实际草稿；Context 面板展示来源、有效/撤销要求、预算省略和缺口。实际发送会重新检查当前输入与 workspace。

## 接续与失败恢复

- 每个成功的原生执行引用都有回执：原生 ID、已确认事件位置、尚未完整交付的 assistant 来源、要求修订号和执行入口身份。
- 原生 ID 与入口/cwd 身份相符、回执仍能在历史中定位时，补齐其尚未知晓的对话。旧 ID 没有回执时，不假设它了解已有历史。
- 每轮重新读取 workspace、HEAD、改变的文件及包含 staged changes 的 Git hunk。旧测试信号只代表历史记录，不是当前代码已经通过验证。
- 较早有效要求和未交付用户请求不按“最后 8 条”裁掉；无法放入预算时拒绝执行，TUI 保留未提交输入。可调整 `context.provider_token_budgets` 或另开聚焦会话。预算是保守估算，不是 provider tokenizer 的精确计数。
- assistant 输出能容纳时保留全文；否则标明摘录/省略，来源留在 pending 集合中。预算不足时仍可能无法补齐全文，不能把回执解释成完整语义理解证明。
- 预览、handoff 接受、失败、权限拒绝和未知结果均不推进回执；源 agent 不需要在线，也不需要额外调用它生成摘要。
- 同一 session 的运行租约阻止多客户端并发发送。会话状态和证据在 SQLite 中原子提交；HTTP future 中断、daemon 重启以及旧记录中未配对的 dispatch 都保留未知标记。

先确认原生执行已经停止并检查部分修改，再按界面给出的准确 ID 确认核对：

```text
/reconcile evt_<界面显示的准确ID>
```

该命令不终止 provider、不重发 prompt、不推进回执。尚在运行的回合不能这样解除保护。F5 先核对已有结果；缺少确认依据时不会自动把未知当作完成。

原生引用失效时，检查修改后显式清除该 provider 的引用：

```text
/reset-provider codex
```

下一次用户发送才启动新的原生会话并携带 Baize 历史；没有隐藏重试。其他 provider 的引用和主对话保留。取消整个 Baize session 仍沿用既有语义，不等于停止 provider。

Antigravity headless 可能因无法询问命令权限而自动拒绝工具，即使退出码为 0、terminal status 为 `SUCCESS`。带有 `denied_actions` 的结果现在报告 `PermissionDenied`，不作为成功回合。用户需要通过实际原生入口配置适用权限；Baize 本次没有实现原生审批桥接。

## API 与兼容

- `GET /sessions/:id/requirements`：查询有效、撤销、历史 handoff 来源及执行回执。
- `POST /sessions/:id/requirements`：显式用户命令，例如 `{"action":"remember","key":"api","text":"保持公开签名"}`、`{"action":"forget","key":"api"}`。
- `POST /sessions/:id/context-preview`：`{"provider_id":"codex","prompt":"待提交的请求"}`，与实际发送共用上下文构建路径；不写回执。原 GET 接口保留。
- `POST /sessions/:id/prompt`：上述 `/remember`、`/forget`、`/requirements`、`/reconcile`、`/reset-provider` 也可作为本地命令使用。
- 新客户端先核对 daemon 的 `runtime.capabilities.session_continuity`；旧 daemon 不会收到本地命令或普通 prompt。升级后需先重新启动 daemon。
- 旧 session 缺少 continuity 字段时使用默认值；旧 handoff 中用户明确提供的约束按来源导入，可撤销且不反复复活。
- 缺少原生引用或重置引用后使用完整上下文。没有导入私有 transcript、迁移未完成工具进程，或声称 CLI 与 Desktop 能力等价。

## 验证证据

自动化使用 fake executor、临时数据库/Git workspace 和 Ratatui TestBackend，不消耗模型额度。覆盖长对话、约束替换/撤销、旧数据导入、原生引用不匹配、pending stream 重组、预算拒绝、回执原子回滚、双客户端、重启、未 dispatch 的租约、失效引用、预览与 handoff 不确认交付，以及 exit 0 权限拒绝。

真实验收入口需要显式启用：

```sh
cargo build --release -p baize-daemon --example continuity_daemon
python3 scripts/continuity_acceptance.py --allow-real-providers \
  --daemon target/release/examples/continuity_daemon \
  --output target/audit/continuity/<新的目录名>
```

它使用独立 workspace/data/端口和现有 `allow_project` 策略，允许 agent 在验收项目内编辑和运行测试，不读取/修改用户 Baize 配置，不操作旧 daemon。该例子不是默认 daemon 配置，也不在 `cargo test` 中执行真实 provider。

最终 release 构建证据：`target/audit/continuity/real-20261125-04/`（此前 `real-20261125-03` 的加强验收也通过）。Codex CLI 0.171.0、Antigravity CLI 1.5.14，共 3 个真实回合、12 项检查通过：

1. Codex 创建 `normalize_label` 和测试，记录原生 ID。
2. 更新保存的前缀与错误信息要求，Antigravity 合并连续空白、补齐类型检查和测试；在交回前独立验证其实现，确认对话口令未写入文件。
3. 增加外部 `acceptance_cases.json`，重启测试 daemon；Codex 恢复原 ID、读取新文件、保留 Antigravity 修改并增加测试，中文回答中正确复述只存在于历史对话中的口令。

`result.json`、三轮 `*-preview.json` / `*-response.json` / `*-events.json`、各阶段测试及独立校验结果保留在该目录。三轮 workspace 测试通过，SENTINEL 未改变；最终 8 项 unittest 通过。TUI renderer 可重放实际证据生成三种尺寸的 cell captures，不再调用模型。

前一轮 `real-20261125-02` 复核发现 Antigravity 权限拒绝，初始脚本仅检查终态与最终产物，导致假阳性。该轮 `result.json` 已标为无效并保留原始日志，相关误判已补回归测试。含此排查和最终 release 复验，共调用真实模型 9 次；`real-20261125-01` 在本地代理配置处失败，调用次数为 0。

最终自动化验证：`cargo test --workspace` 422 项、`cargo test --workspace --all-targets` 429 项全部通过，相对 main 新增 32 项；fmt check、clippy `-D warnings` 和 release 构建通过。

发布版 TUI 另通过 10 项 POSIX PTY 交互回归（隔离草稿目录，不发送 provider prompt）。旧 daemon 的提交前拒绝保护也通过真实 CLI 检查，未创建/发送会话。相应结果为 `target/audit/continuity/release-pty/result.json` 与 `target/audit/continuity/upgrade-guard.json`。
