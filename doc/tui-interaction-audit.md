# TUI 交互检查与修复

2026-11-22，基线 `fe0a2cae88de8acbc5c85ae377b31da67c778234`。本轮检查 Baize 日常编辑、会话恢复、执行过程阅读、provider 选择、权限处理、取消和退出恢复链路。目标是避免用户输入丢失、目标错配和不可见的快捷键副作用。

本页保留当轮验收快照。2026-11-25 的双重 review、草稿格式升级及修复取舍见 [后续核验](tui-review-followup.md)；当前操作说明见 [quickstart](quickstart.md)。

## 方法与范围

先采集基线，再修复、捕获相同状态并执行回归。离线截图来自真实 `render()` 与 Ratatui `CompletedFrame.buffer`，并非静态 UI 示意；故障和权限使用受控数据，不对真实会话发送取消或审批。修复后的 release 二进制另在 POSIX PTY 中连接已有本机 daemon，读取历史、强制结束客户端并重启；草稿目录使用临时目录，不读取或覆盖用户的客户端草稿。正常启动会注册当前工作区并探测 provider diagnostics。

本地证据位于 `target/audit/tui-interactions/{before,small-before,after,pty}/`。PNG 是真实渲染单元格的栅格化呈现；PTY 保留 `.ansi` 和 `result.json`。检查不调用真实 provider prompt，不证明所有终端字体、辅助技术或 Windows/Linux 行为。取消 provider 进程、并发客户端草稿合并不属于本轮修复。

## 按流程的结果

| 步骤 | 检查点 | 基线健康度与影响 | 修复后 | Issue |
|---|---|---|---|---|
| 1 | 输入、Unicode、多行粘贴、过程折叠与正文阅读 | 已有保护及回归；本轮检查未发现新的阻断问题 | 通过原有渲染、流式、滚动、去重测试与 PTY 输入检查 | — |
| 2 | 打开 / 重载历史会话 | P1：坏 events 导致新 session ID 搭配旧正文，草稿清空；加载期间编辑丢失 | 完整新状态验证成功才应用；提交切换前保存最新编辑，同会话重载保留输入、附件和光标 | [#2](https://github.com/songdragon/baize/issues/2) |
| 3 | 取消会话 | P1：Ctrl-X 直接提交持久取消 | 显示准确 session 目标与 provider 可能继续的提示；确认前零取消请求；Esc 保留任务，重复快捷键不能跳过 | [#3](https://github.com/songdragon/baize/issues/3) |
| 4 | 审阅与决定权限 | P1：Ctrl-D 可以拒绝隐藏请求；刷新可能改变选中目标 | 批准与拒绝统一先审阅；按 ID 保持目标，替换后重新审阅；空列表无写请求 | [#4](https://github.com/songdragon/baize/issues/4) |
| 5 | 手动选择 provider 后刷新 | P1：选中的 Antigravity 被重置为 Codex，下一回合改变执行者 | 按 ID 保留最新选择，列表重排也保持；被移除或不可用仍按原健康逻辑回退 | [#5](https://github.com/songdragon/baize/issues/5) |
| 6 | 退出 / 异常终止后继续编辑 | P1：只有内存草稿，退出保护无法覆盖崩溃 | 原子保存并恢复文本、光标、附件路径，按 workspace/session 分组；错误可见，不覆盖损坏文件；不自动重发不确定提交 | [#6](https://github.com/songdragon/baize/issues/6) |
| 7 | 键盘正反向遍历及窄屏 | P2：Shift-Tab 和 F6 同向 | F6 正向，Shift-Tab 反向；Inspector 按需显示；Esc 回到输入且保留草稿 | [#7](https://github.com/songdragon/baize/issues/7) |

| 8 | 缩小窗口后的退出 | P2：小于 36×10 时退出确认被 Resize 提示遮挡 | 紧凑界面优先显示 Enter / Esc、保存状态及继续运行边界；35×9 取消确认也显示目标 | [#8](https://github.com/songdragon/baize/issues/8) |

## 回归证据

新增 17 项 TUI 测试，工作区测试由 355 增至 372（TUI 153 → 170）；加上 7 项离线预览，`--all-targets` 为 379 项，0 失败。格式检查和 clippy 零警告通过，release 构建成功。

| 修复 | 主要新增测试 |
|---|---|
| #2 | `invalid_history_never_commits_a_partial_session_or_discards_its_draft`、`switching_saves_edits_made_after_request_started_and_same_session_reload_keeps_input` |
| #3 | `cancellation_requires_confirmation_and_escape_preserves_execution_and_draft`、`cancellation_rejects_missing_changed_or_canceled_targets_and_waits_for_worker`、`cancel_dialog_renders_target_and_scope_in_normal_and_narrow_terminals` |
| #4 | `both_permission_decisions_require_visible_review_and_refresh_preserves_identity` |
| #5 | `refresh_preserves_the_latest_selected_provider_by_id_across_reordering`、`refresh_still_falls_back_when_the_chosen_provider_is_unavailable` |
| #6 | `drafts_survive_restart_with_unicode_cursor_attachments_and_workspace_isolation`、`session_drafts_and_uncertain_submissions_are_retained_without_automatic_retry`、`corruption_and_write_failure_are_visible_and_never_replace_old_data`、`saved_draft_exit_dialog_reports_recovery_instead_of_loss` |
| #7 | `focus_shortcuts_traverse_both_directions_from_every_start_without_changing_drafts`、`reverse_focus_opens_the_inspector_on_narrow_screens_and_handles_an_empty_session` |
| #8 | `compact_exit_keeps_confirmation_visible_and_escape_returns_to_resize_guidance`、`compact_cancel_shows_target_and_actions_and_zero_sized_screens_remain_safe` |
| 捕获 | `interaction_audit::capture_current_interaction_review` |

完整验证命令：

```sh
cargo test --workspace
cargo test --workspace --all-targets
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --release -p baize-cli
python3 scripts/tui_interaction_smoke.py
```

重新采集渲染单元格（不需要 daemon）：

```sh
BAIZE_TUI_AUDIT_DIR="$PWD/target/audit/tui-interactions/after" \
  cargo test -p baize-tui interaction_audit::capture_current_interaction_review -- --exact
```

修复后同场景：03 保持 `task_original` 与原草稿；04 保留 `EDITED WHILE LOADING`；05 仍选择 Antigravity；02/06 显示正确 Inspector；08 显示取消确认；09/10 显示草稿恢复和可恢复退出提示。11 在 35×9 仍显示退出确认；07 为保存不可用时的降级提示，不代表正常保存成功的退出状态。

发布版 PTY 验证：中文多行粘贴及保存、SIGKILL 后恢复、反向焦点、权限审阅、取消审阅但不确认、已保存草稿退出提示、重复退出快捷键保护、Esc 继续、终端 raw mode 与备用屏恢复。客户端使用的临时草稿在测试结束后清理，真实 daemon 保持运行。
