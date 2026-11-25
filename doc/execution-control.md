# 执行停止与原生审批

2026-11-25，跟踪 [停止闭环 #12](https://github.com/songdragon/baize/issues/12)、[原生审批 #13](https://github.com/songdragon/baize/issues/13)。实现提交 `9705353`，基于 main `57fa83a`，仍通过 daemon 的 HTTP/SSE 驱动薄 TUI。

## 用户操作

- Ctrl-X 先审阅目标回合，Enter 请求停止，Esc 保持执行。停止不会删除主对话、撤销文件修改或自动重发请求。关闭 TUI 仍不会取消执行。
- 界面区分 `stopping`、`stopped`、`unconfirmed`。只有确认结束后才能继续发送或接受交接；未确认时先检查原生执行与 workspace，再按准确 ID 使用 `/reconcile`。仍有存活 worker 时不能核对解除保护。
- 原生审批到达时实时更新 Permissions 面板，保留正在编辑的草稿和焦点。先审阅命令、目录、原因、执行来源及作用范围，再 Ctrl-A 批准一次或 Ctrl-D 拒绝。批准只排队交付本次决定，不代表命令已经执行或任务已经完成。
- 过期、已决定、停止中、断连或属于其他回合的请求返回冲突；不会把旧批准应用到新请求。Providers 面板显示实际入口及原生审批是否接通。

## 入口与边界

| 入口 | 审批 | 停止与确认 |
| --- | --- | --- |
| Codex 0.171.0，`codex app-server --listen stdio://` | 命令和普通文件修改的单次请求；不批准 session-wide 或持久规则 | 发送 `turn/interrupt`，等待同一 thread/turn 的 `turn/completed`；仅中断 ACK 不算停止证明 |
| Antigravity 1.5.16，`agy --print` | 未接通，print/stdin 协议拒绝审批控制消息 | 有界停止 Baize 启动的本地进程组；原生后台执行没有确认时保留 unknown |
| OpenCode CLI | 未接通，原生往返未验收 | 本地进程组控制，原生后台状态仍需核对 |

Codex 服务沿用其原生 prompt、项目配置、模型和工具加载，不提供统一系统提示词替换。Baize 的 `ask` 映射为原生 `untrusted` 审批策略、workspace-write 沙箱；`allow_project` 为 workspace-write / never，`deny` 为 read-only / never。不会改写全局配置或忽略 provider 的受管要求。

目录根权限扩张（`grantRoot`）、强制人工检查点、stdin 专用审批、额外权限申请、表单/elicitation 等未支持交互不会被转换成普通单次批准，会明确报告需使用原生入口。本次没有实现 ACP 生命周期、Desktop 自动化、共享配置同步或所有原生工具的等价接入。

Antigravity 的本机兼容检查仅提交一条 `control_response`，退出码 2，原生报告 `num_turns=0` 且 usage 全为 0；没有提交用户 prompt。该限制与 [官方 headless 文档](https://antigravity.google/docs/cli/headless.md) 的 Unsupported messages 相符。未采用自动点击或跳过全部权限来伪造审批闭环。

## 生命周期与持久化

- 控制句柄按唯一 attempt ID 注册，由实际 worker 持有。客户端断开不取消 worker；已取得的原生中断完成证据仍原子保存，且不推进成功回执。
- 停止请求先持久化，再发送给精确句柄；审批解析同时检查落库的停止状态，覆盖“已落库、信号未送达”的竞态。
- 原生决定先与审计事件在同一事务提交，再对 worker 可见。持久化失败不能释放批准；同一 callback 只能决定一次。
- worker 结束或 daemon 重启后，相关 Pending 原生请求变为 Expired。查询不截断为第一页；旧手动 Baize 记录不被误改。
- CLI 在自己的进程组运行，TERM 后有界升级为 KILL。输出读取不会因子进程继承管道无限等待；stdin 背压也有界。清理本地进程不冒充原生停止证明。
- 中断确认等待最多 5 秒；无法取得终态时保留未知保护，不把已发送中断或本地进程退出当作确认。累计原生审批等待预算为 10 分钟，TUI 读取预算与 daemon 公布值配合；超时后仍按实际证据判断结果。

## API 与升级

- `POST /sessions/:id/execution/cancel`：停止当前回合，保留会话；没有执行时明确返回错误，已确认停止时重复请求幂等。
- 原 `POST /sessions/:id/cancel`：保留取消整个 session 的语义，并向活跃控制句柄发送停止请求。已有客户端行为不被静默改成继续会话。
- `GET /permissions` / `POST /permissions/:id/approve|deny`：原生记录包含 attempt/provider/native request 的绑定和原始请求详情；旧无绑定记录仍只是 Baize 记录。
- `GET /runtime/status`：连续会话协议 3、execution_control、各 provider 的 native_approvals、实际入口和审批等待预算。
- session 的 continuity.last_stop 保存回合、provider、状态及说明。旧记录缺少字段时使用默认值。

新 TUI/CLI 在发送前要求协议 3，需使用同版本 daemon。Codex 旧执行入口回执身份改变时，发送会被阻止并保留草稿；检查 workspace 后显式 `/reset-provider codex`，下一次发送才带 Baize 历史启动新的原生引用。不会隐藏重试或导入私有 transcript。

## 验证与重现

单测使用协议 fixture、临时进程/SQLite 和 TestBackend，不调用模型。覆盖单次批准/拒绝、过期/错误绑定、停止与完成竞争、HTTP 断连、原生终态缺失、stdin 背压、子进程持有管道、忽略 TERM、重启后过期及原会话交接。

```sh
cargo test --workspace
cargo test --workspace --all-targets
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
python3 -m unittest discover -s scripts -p test_execution_control_acceptance.py -v
cargo build --release -p baize-daemon --example continuity_daemon
python3 scripts/execution_control_acceptance.py --allow-real-providers \
  --daemon target/release/examples/continuity_daemon \
  --output target/audit/execution-control/<新的目录名>
```

真实脚本只在隔离 workspace/data/端口运行，使用 `ask`；仅可批准固定 worker.py 的精确命令，目录/模式不符、shell 注入、其他原生交互或 fixture 被修改都会拒绝验收。模型调用不属于自动化测试。

最终实现的原生审批/停止验收为 `target/audit/execution-control/real-20261125-02/`：5 次真实调用、14 项检查全部通过，批准/拒绝/长任务停止后保持原 Baize session，实际工具进程退出，B 的新对话传回恢复原 ID 的 Codex。开发中同一流程另运行 5 次（01）；它不替代最终构建的 02 证据。

最终构建还运行原有的真实编码接续脚本，证据为 `target/audit/execution-control/continuity-20261125-01/`：3 次真实调用、12 项检查全部通过，三个阶段分别通过 8、9、10 项 fixture 测试；原生引用跨 daemon 重启保持，返回 Codex 消费外部修改的验收文件。

自动化为 473 项 workspace、481 项 all-targets、2 项 Python 验收脚本单测全部通过，fmt、clippy（拒绝 warning）和 release 构建通过。发布版 PTY 的 10 项交互回归通过且未发送模型请求。完整检查、阶段测试数及二进制摘要统一记录在 [验收摘要](acceptance/execution-control-20261125.json)；原始 provider 数据只保存在本地忽略的 `target/audit`。原有已发布功能的历史记录保留，不冒充本次证据。
