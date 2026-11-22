# CLI 真实流式过程输出

更新：2026-11-22。跟踪：[Issue #1](https://github.com/songdragon/baize/issues/1)。开发基线为 `e61853b`；基线源码在开发前已全部提交。

## 用户可见行为

CLI 有输出时，TUI 在下一次界面刷新显示可获得的内容，不等待整个请求结束，也不做本地逐字播放：

- agent 正文按原消息合并增量与快照；最终全文不会再次追加成重复回答；
- 执行过程默认折叠，运行时显示当前工具摘要；最终回答独立显示为正文，不带日志前缀或引用边框；
- F9 定位并展开最新过程，Space 折叠，↑↓ 选择操作，Enter 查看完整参数、输出和原始事件，←→ 切换回合，Esc 返回输入；输入区的空格保留编辑行为；
- stderr 诊断和失败提示在折叠时仍可见，完整内容保留在详情中；
- 工具按原消息 ID 合并更新，不把开始和完成算成两次操作；原始结构化数据与来源继续保存在 daemon 事件中；
- 历史恢复使用相同的消息合并规则，不把诊断当成 agent 回答。

两个管道各自保持输出顺序；跨 stdout/stderr 的全局顺序以读取到达顺序为准，不宣称能重建操作系统未提供的严格时间顺序。控制序列不能作为终端指令执行。

## 当前 provider 路径与证据

| Provider | 调用与已适配的过程 |
|---|---|
| Codex | `codex exec --json`：JSONL 消息及命令、文件变更、工具状态；原生消息 ID 对应快照／增量。来源：[官方非交互文档](https://developers.openai.com/codex/noninteractive/)、本机 `codex exec --help`。 |
| OpenCode | `opencode run --format json`：文本 part 与工具 state，保留 part ID、参数和输出。来源：[官方命令文档](https://opencode.ai/docs/cli/commands/)、仓库已有协议样例。 |
| Antigravity | `agy --print <PROMPT> --output-format stream-json`：适配官方 `event=init/step_update/result`，按 step index 合并 `text_delta`，显示 `tool_info` 和终态响应。来源：[官方 Headless 文档](https://antigravity.google/docs/cli/headless.md)、本机 `agy --help`。 |

Antigravity 的协议不是仅以 `type` 命名的通用 JSON。此次针对其 `step_update`、`text_delta`、`tool_info`、`result.response` 单独解析，并更新结构化输出能力检测。没有通过本次改动声称它的 ACP、MCP 或 Desktop 全部等效接入。

能展示多少正文分片取决于 CLI 自身的 flush 和事件粒度。CLI 只提供完整消息时，Baize 立即展示该消息和已提供的执行过程，不伪造 token 流。本文不承诺完整 PTY 交互、私有思维链或未公开的内部工具状态。

## 实现

1. 适配器并行读取 stdout/stderr 字节片段，普通文本无需换行即可发送；UTF-8 尾部碎片暂存至字符完整，JSON 帧暂存至可解析。
2. `AgentStreamMetadata` 保存 `channel`、`mode`（`message/delta/snapshot`）和 `item_id`。旧事件不含该字段时仍可读取。
3. daemon 记录并广播 `session.agent.output`、`session.agent.diagnostic`、`session.agent.tool_call`、`session.agent.status`。
4. TUI 等待 SSE 订阅和初始历史读取成功后，才发起 prompt。订阅失败时恢复草稿，明确说明请求未提交。
5. 断流后重新订阅并先读取事件历史；重叠事件按 ID 去重。广播积压会发出 `daemon.resync_required`，触发同样的补齐流程。重连不重发 prompt。
6. 请求返回后停止并等待流读取线程，排空其消息，再读取最终事件历史，补齐尾部后显示结果。
7. 上下文组装先合并分片再选择近期消息，诊断不当成 assistant 回复；上下文／交接读取不再只取前 500／200 条事件。
8. TUI 按回合维护过程、工具、诊断与回答。工具事件到来时，前面的未标注最终用途的文本成为过程说明；明确的 `final_answer` 消息保留为正文。Antigravity `result.response` 和可用的终态全文优先用于最终回答，兼容已经保存的历史事件。

需要重新启动 daemon 才会使用新的适配器及事件模型。只重开 TUI 会继续连接原 daemon。重启前应先确认当前 provider 回合已经结束；会话记录保存在原数据目录。

折叠与正文展示是 TUI 客户端更新，兼容 `e02e94c` 的 daemon；已使用该流式服务时，直接启动新的 TUI 即可。

## 验证

全工作区 355 个测试通过，其中适配器 64、daemon 76、TUI 153；包含离线示例的 `--all-targets` 共 362 个。首次流式接入增加 26 个测试；折叠展示新增 12 个正式 TUI 测试及 7 个示例测试，退出保护另外新增 3 个回归测试。自动测试使用假进程、协议样例或 Fake executor，不发送真实模型请求。

- 假 CLI 退出前的无换行 stdout、stderr，以及拆分中文、emoji、大量诊断和尾部 JSON；
- 官方 Antigravity step_update/tool/result 格式、Codex/OpenCode 消息和工具状态；
- SSE 订阅就绪、HTTP 拒绝、取消、超时、断线历史补齐、重复事件、积压通知和尾部事件；
- tower 验证 prompt 尚未完成时，stdout/stderr/工具事件已在 SSE 和 SQLite 中可见；
- TestBackend 验证正文到达即显示、增量与快照去重、工具完整输出、来源区分和历史一致；
- 订阅失败不执行 prompt，草稿保留；最终历史和已显示 stderr 不重复；
- 超过 500 条流事件后的最新快照仍进入上下文。
- 原生最终结果、过程说明归类、工具合并、折叠失败提示、正文格式、多回合重复工具 ID、历史／断线重叠去重和长回答后的过程定位；
- 用用户已有的 381 条 Antigravity 事件做离线渲染，并在正式 release TUI 中恢复同一记录、展开过程和详情、检查终端恢复；不提交新 prompt。

验证命令：

```sh
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --release --locked -p baize-cli
```

真实供应商端到端调用仍需使用已登录的对应 CLI 进行人工测试，不能以这些无配额测试宣称所有版本和所有原生工具已经验收。
