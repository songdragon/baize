# Baize 终端工作台设计

更新：2026-11-22。面向持续开发会话的日常工作台；实现记录与验证结果在本文末尾维护。研究依据见 [ui-research.md](ui-research.md)。

## 工作模型

用户的工作对象是一个 Baize conversation，与当前 workspace 关联。provider 是可以更换的执行者。主对话保留用户输入、回答来源和执行顺序，内部交接不能伪装成用户消息。TUI 只通过 daemon 操作领域状态。

宽终端可显示三栏：会话列表、主对话、按需检查面板。中等宽度保留对话与检查面板，窄终端按焦点显示列表或检查内容。所有终端尺寸必须安全退化，不能产生越界光标。界面不要求图标字体。

## 交互

| 目标 | 操作 | 结果 |
|---|---|---|
| 开始讨论、反馈、编码 | Enter | 在同一会话提交输入，阻止并发重复提交 |
| 多行编辑 | Alt-Enter / Shift-Enter / Ctrl-J；方向键、Home/End、Delete/Backspace | 保留换行、缩进、中文与 emoji 字素；部分终端只能用 Alt-Enter 或 Ctrl-J |
| 会话查找 | Ctrl-O | 按目标、provider、状态或 ID 搜索；Enter 恢复完整事件对话 |
| 最新会话 | Ctrl-L | 恢复当前 workspace 已加载列表中的最新会话 |
| 新会话 | Ctrl-N | 原会话草稿暂存，清空原对话显示；执行中不切换 |
| 目标 provider | Tab | 下一次输入由目标执行者在原 Baize session 继续；头部区分 active 与 target |
| 显式交接 | Ctrl-H → 检查 Handoff → Ctrl-Y | 先准备事实与摘要，再接受；保持 Baize session ID |
| 检查详情 | F5 / F7 / F8 | 刷新、轮换、显示详情：Session、Changes、Context、Handoff、Permissions、Providers |
| 执行过程 | F9；过程聚焦后 Space / ↑↓ / Enter / ←→ | 定位并展开最新回合；折叠、选择操作、查看完整参数与输出、切换历史回合；Esc 回到输入 |
| 焦点 | F6 / Shift-Tab / Esc | 输入、过程、会话、检查面板间切换；Esc 关闭浮层或回到输入 |
| 权限 | Ctrl-P / ↑↓ / Ctrl-A / Ctrl-D | 查看完整命令、风险、理由、所属会话后决定；与 provider 自身权限机制区分 |
| 阅读历史 | PgUp / PgDn / 鼠标滚轮 | 对话回看不应被新输出强制拉到底部；检查面板可独立滚动 |
| 查找操作 | Ctrl-K / F1 | 搜索命令／显示帮助，避免只靠记忆快捷键 |
| 完成 | Ctrl-E | 用户检查产物后标记完成 |
| 取消 | Ctrl-X | 取消 Baize 会话；目前 daemon 不保证终止 provider 进程，必须明确提示并核对 workspace |
| 退出 | Ctrl-Q / Ctrl-C | 有草稿或后台操作时确认；daemon 与执行继续存活 |
| 图片 | Ctrl-V、/image path、/unimage | 使用已有附件能力，显示文件名，允许清除 |

会话草稿目前只在客户端进程内暂存，退出前会提示未发送内容丢失。草稿恢复不会自动发送。网络超时后不能自动重试 prompt、审批或交接接受等操作，因为服务端可能已经执行。

## 数据与可靠性

- 当前目录定位 workspace，复用已注册的项目，不在 UI 线程执行 HTTP。
- 会话列表限定该 workspace；恢复时读取事件记录，按顺序显示用户和各执行者输出，并用 event ID 去重。
- 同 session 后续回合保留已经见过的 event ID，避免请求完结后再取历史导致重复回答。
- 会话切换或新建时清理消息合并状态、旧交接和上下文；草稿按 session ID 关联。
- 过程默认折叠成一行，执行中补充当前工具摘要；工具参数、输出和原始事件在操作详情中查看。失败／诊断提示折叠时仍可见。
- 最后回答使用正常正文，支持标题、粗体与代码，不再带日志前缀或引用竖线。收到工具操作后，将之前未标注为最终回答的中间说明归入过程；优先使用 provider 的最终结果。
- 到达即更新，不做本地逐字播放；订阅就绪后提交，断流补齐和最终去重见 [流式输出设计](cli-streaming-output.md)。实时与历史回放使用同一展示模型；展开长回答上方的过程时自动定位。
- 提交、会话加载、检查、权限和交接通过工作线程执行。界面继续响应编辑、帮助、滚动和退出。
- 请求响应必须与原 session 对应，迟到响应不得绑定到新会话。
- Git 变更和上下文是最近一次读取的快照，明确刷新入口。上下文预览使用占位请求，不声称包含未发送草稿。
- 不把 provider health、CLI 存在或未知 quota 画成已验证的原生 Desktop 能力。
- 终端 raw mode、鼠标捕获、paste mode、备用屏在错误或 panic 时恢复。

## 验收场景

1. 首次进入 → 工作区会话列表 → 输入 → 输出 → 继续反馈，全程只有一个 session。
2. 退出后恢复 session，能看到过去的用户输入、原 provider 回复及后续 provider 回复，顺序和来源正确。
3. A → B → A，界面始终显示同 session，区分当前与下次执行者，并可检查 daemon 组合的上下文。
4. 中英文、组合字符、emoji、中间插入／删除、多行粘贴，光标与显示一致。
5. 浏览历史时新事件到达，仍能阅读当前位置；回到底部后跟随新输出。
6. 请求失败、流断开、服务端结果不确定时保留会话和可恢复输入，不自动重复操作。
7. 审批、交接和会话切换遇到执行中状态时有明确反馈，不错用另一会话的结果。
8. 小屏、空列表、长会话、长 diff、长权限命令、帮助和搜索无匹配均可操作。
9. 使用 Fake executor、TestBackend、隔离 daemon 数据目录验证，不消耗真实 provider 配额。

## 当前验证记录

2026-11-22 本地验证通过：

- `cargo test --workspace`：352 个测试通过，其中 TUI 150 个；`--all-targets` 额外包含 7 个离线预览测试，总计 359 个，0 失败。
- `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets -- -D warnings` 通过。
- `cargo build -p baize-cli` 构建成功。
- POSIX PTY + 本地假 daemon 验证启动、命令面板、多行中文粘贴、草稿退出确认、新会话草稿保留及终端模式恢复。7 次 fixture 请求，0 次 provider prompt。可在 daemon 端口空闲时运行 `python3 scripts/tui_smoke.py` 重现。
- TestBackend 覆盖宽窄屏、极小屏、空搜索、六种检查视图、浮层、Unicode 光标，以及超过 65,535 行的会话。
- 事件回放、分页、同会话跨回合去重、过期响应、未确认执行保护、HTTP 分块 Unicode 和 SSE 超时碎片有回归测试。
- 正式 release TUI 在独立 PTY 中恢复已有真实 Antigravity 会话，验证 F9 定位过程、Enter 查看原始详情、草稿退出保护和终端模式恢复。另回放该会话的 381 条已记录事件，包含 4 次工具操作与最终正文；未提交新 provider 请求。

以上验证不消耗真实 provider 配额，不构成真实供应商入口、Windows/Linux 全平台发布或所有跨产品原生工具能力的验收。取消不保证终止 provider 进程、草稿仅保存在当前客户端内存等边界按本文明确呈现。
