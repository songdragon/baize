# Baize 终端工作台设计

更新：2026-11-25。面向持续开发会话的日常工作台；实现记录与验证结果在本文末尾维护。研究依据见 [ui-research.md](ui-research.md)。

## 工作模型

用户的工作对象是一个 Baize conversation，与当前 workspace 关联。provider 是可以更换的执行者。主对话保留用户输入、回答来源和执行顺序，内部交接不能伪装成用户消息。TUI 只通过 daemon 操作领域状态。

宽终端可显示三栏：会话列表、主对话、按需检查面板。中等宽度保留对话与检查面板，窄终端按焦点显示列表或检查内容。所有终端尺寸必须安全退化，不能产生越界光标。界面不要求图标字体。

## 连续会话与用户要求

`/remember <key> <text>` 保存或替换明确要求，`/forget <key>` 撤销，`/requirements` 查看来源。它们是 Baize 本地命令，不调用 agent。Tab 或 Ctrl-H / Ctrl-Y 在原 session 选择执行者；F5 和交接预览使用请求时的实际草稿，显示来源、预算缺口和未交付状态。发送时重新构建。

结果未知时先确认 provider 停止并检查修改，再 `/reconcile <准确的turn-id>` 显式确认核对；该命令不重放请求。失效原生引用可 `/reset-provider <provider>` 清除，下一次明确发送才开始新的下层会话。当前实现、原生权限边界及真实 A → B → A 验收见 [连续会话说明](reliable-agent-continuity.md)。

## 交互

| 目标 | 操作 | 结果 |
|---|---|---|
| 开始讨论、反馈、编码 | Enter | 在同一会话提交输入，阻止并发重复提交 |
| 多行编辑 | Alt-Enter / Shift-Enter / Ctrl-J；方向键、Home/End、Delete/Backspace | 保留换行、缩进、中文与 emoji 字素；部分终端只能用 Alt-Enter 或 Ctrl-J |
| 会话查找 | Ctrl-O | 按目标、provider、状态或 ID 搜索；Enter 恢复完整事件对话 |
| 最新会话 | Ctrl-L | 恢复当前 workspace 已加载列表中的最新会话 |
| 新会话 | Ctrl-N | 原会话草稿暂存，清空原对话显示；执行中不切换 |
| 目标 provider | Tab | 下一次输入由目标执行者在原 Baize session 继续；头部区分 active 与 target，刷新按 provider ID 保留仍可用的选择 |
| 显式交接 | Ctrl-H → 检查 Handoff → Ctrl-Y | 先准备事实与摘要，再接受；保持 Baize session ID |
| 检查详情 | F5 / F7 / F8 | 刷新、轮换、显示详情：Session、Changes、Context、Handoff、Permissions、Providers |
| 执行过程 | F9；过程聚焦后 Space / ↑↓ / Enter / ←→ | 定位并展开最新回合；折叠、选择操作、查看完整参数与输出、切换历史回合；Esc 回到输入 |
| 焦点 | F6 / Shift-Tab / Esc | F6 正向、Shift-Tab 反向遍历输入、过程、会话、检查面板；Esc 关闭浮层或回到输入 |
| 权限 | Ctrl-P / ↑↓ / Ctrl-A / Ctrl-D | 隐藏权限先打开审阅；至少 36×18，PgUp/PgDn 浏览长内容；再次操作已审阅的同一目标才决定。刷新按 ID 保留选择，目标被替换后重新审阅；与 provider 自身权限机制区分 |
| 阅读历史 | PgUp / PgDn / 鼠标滚轮 | 对话回看不应被新输出强制拉到底部；检查面板可独立滚动 |
| 查找操作 | Ctrl-K / F1 | 搜索命令／显示帮助，避免只靠记忆快捷键 |
| 完成 | Ctrl-E | 用户检查产物后标记完成 |
| 停止当前执行 | Ctrl-X → Enter / Esc | 确认后请求停止当前回合，保留主对话与部分修改；区分停止中、已确认停止和未确认，确认停止后才可在原会话继续或交接 |
| 退出 | Ctrl-Q / Ctrl-C / 菜单 Quit → Enter | 所有状态都先确认，连续按退出键也不会直接关闭；Esc 留在客户端；daemon 与执行继续存活 |
| 图片 | Ctrl-V、/image path、/unimage | 使用已有附件能力，显示文件名，允许清除 |

会话草稿按 workspace/session 保存到本地数据目录 `tui-drafts.json`，包含文本、光标与附件路径；使用原子替换、Unix `0600` 权限。重启恢复新会话草稿，打开已有会话恢复其草稿；保存错误单列一行，与操作反馈同显，损坏文件不覆盖。退出框根据实际保存状态提示可恢复或可能丢失。v2 将未发送草稿与运行中／不确定提交分开保存，后者及旧格式输入需要通过 Ctrl-K 显式恢复，绝不自动填入输入框或发送。记录确认执行结束后清理恢复标记。缓存写入使用非阻塞锁与冲突检测，另一个客户端的内容不会被静默覆盖。数据路径和清理方式见 [quickstart](quickstart.md)。网络超时后不能自动重试 prompt、审批或交接接受等操作，因为服务端可能已经执行。

## 数据与可靠性

- 当前目录定位 workspace，复用已注册的项目，不在 UI 线程执行 HTTP。
- 会话列表限定该 workspace；恢复时读取事件记录，按顺序显示用户和各执行者输出，并用 event ID 去重。
- 同 session 后续回合保留已经见过的 event ID，避免请求完结后再取历史导致重复回答。
- 会话切换先验证 session/events 并构建完整新状态，成功才整体应用；失败保留原会话、正文、草稿及附件。应用异步结果前保存最新编辑，不能用请求发起时的旧草稿覆盖等待期间的修改。
- 会话切换或新建时清理消息合并状态、旧交接和上下文；草稿按 session ID 关联。
- 过程默认折叠成一行，执行中补充当前工具摘要；工具参数、输出和原始事件在操作详情中查看。失败／诊断提示折叠时仍可见。
- 最后回答使用正常正文，支持标题、粗体与代码，不再带日志前缀或引用竖线。收到工具操作后，将之前未标注为最终回答的中间说明归入过程；优先使用 provider 的最终结果。
- 到达即更新，不做本地逐字播放；订阅就绪后提交，断流补齐和最终去重见 [流式输出设计](cli-streaming-output.md)。实时与历史回放使用同一展示模型；展开长回答上方的过程时自动定位。
- 提交、会话加载、检查、权限和交接通过工作线程执行。界面继续响应编辑、帮助、滚动和退出。
- 请求响应必须与原 session 对应，迟到响应不得绑定到新会话。
- Git 变更和上下文是最近一次读取的快照，明确刷新入口。上下文预览使用占位请求，不声称包含未发送草稿。
- 不把 provider health、CLI 存在或未知 quota 画成已验证的原生 Desktop 能力。
- Providers 显示 daemon 报告的实际入口及原生审批支持。Codex 使用 app-server stdio 接收单次命令/文件审批与中断完成证据；Antigravity print 不支持审批控制往返，停止本地 CLI 后仍可能保持未确认。审批事件实时更新面板但不抢走输入焦点，命令、目录及作用范围可滚动审阅，过期请求不能再次批准。详见 [执行控制](execution-control.md)。
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

2026-11-25 本地验证通过：

- `cargo test --workspace`：390 个测试通过，其中 TUI 188 个；`--all-targets` 额外包含 7 个离线预览测试，总计 397 个，0 失败。
- `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets -- -D warnings` 通过。
- `cargo build -p baize-cli` 构建成功。
- POSIX PTY + 本地假 daemon 验证启动、命令面板、多行中文粘贴、草稿退出确认、新会话草稿保留及终端模式恢复。7 次 fixture 请求，0 次 provider prompt。可在 daemon 端口空闲时运行 `python3 scripts/tui_smoke.py` 重现。
- TestBackend 覆盖宽窄屏、极小屏、空搜索、六种检查视图、浮层、Unicode 光标，以及超过 65,535 行的会话。
- 事件回放、分页、同会话跨回合去重、过期响应、未确认执行保护、HTTP 分块 Unicode 和 SSE 超时碎片有回归测试。
- 退出保护额外覆盖空闲、有草稿／附件、有后台任务、已有浮层和重复 Ctrl-C：所有退出入口统一确认，Esc 保留客户端状态，仅 Enter 确认退出。
- 正式 release TUI 在独立 PTY 中恢复已有真实 Antigravity 会话，验证 F9 定位过程、Enter 查看原始详情、草稿退出保护和终端模式恢复。另回放该会话的 381 条已记录事件，包含 4 次工具操作与最终正文；未提交新 provider 请求。
- 2026-11-22 交互检查建立 [#2–#8](https://github.com/songdragon/baize/issues?q=is%3Aissue+is%3Aopen)，新增 17 项测试，覆盖原子切换、危险操作审阅、provider 选择保持、本地草稿、反向焦点与极小终端确认。正式 release 在隔离 PTY 中强制终止并重启，验证多行中文草稿恢复及九项交互检查；运行 `python3 scripts/tui_interaction_smoke.py` 重现，未发送 provider prompt。逐项证据见 [交互审查](tui-interaction-audit.md)。

- 2026-11-25 通过 computer use 操作 Antigravity 与 ZCode 独立审阅，核验后修复八类问题，新增 18 项回归测试。发布版隔离 PTY 的 10 项检查包含旧格式显式恢复与 v2 写入；具体采纳、未采纳及待核实项见 [双重 review 记录](tui-review-followup.md)。

以上测试不消耗真实 provider 配额，不构成真实供应商入口、Windows/Linux 全平台发布或所有跨产品原生工具能力的验收。取消不保证终止 provider 进程；本地草稿用于单客户端恢复，没有多客户端并发合并或云同步。
