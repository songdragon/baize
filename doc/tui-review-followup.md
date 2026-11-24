# TUI 双重 Review 核验与修复

日期：2026-11-25。审阅基线：`bde63840dd83cf5e0d2481b614709c73ec057575`，相对 `main` 的 `fe0a2ca`，即 [PR #9](https://github.com/songdragon/baize/pull/9) 原始改动。

通过 computer use 操作 Antigravity（会话 Code Review For TUI Changes）和 ZCode（会话 基于 HEAD bde6384 收敛 Review Comments），分别请求只读审阅。两边确认了相同 HEAD 与范围，未修改代码；ZCode 明确报告使用静态阅读、未运行测试。以下复现、取舍、修复与测试由 Codex 完成；这些应用内意见没有作为 GitHub review 发布。

## 取舍记录

| 来源 | 意见 | 核验及处理 |
|---|---|---|
| Antigravity 1 | 新会话未清理 `execution_uncertain` 会死锁 | 未采纳。唯一生产调用入口 `run_command(New)` 已先阻止不确定状态进入新会话；原报告的快捷键复现路径不可达。保留保护并补回归。 |
| Antigravity 2；ZCode 待核实 | 收到 canceled 事件就应解除保护 | 未采纳。daemon 的 cancel handler 立即写入 `completed(status=canceled)`，此时 provider 可以继续运行。执行返回后才产生 `session.agent.ignored` / 失败等记录；这些终态可以解除保护。提前清理会允许并发修改工作区。 |
| Antigravity 3 | 首次创建 session 失败后草稿从缓存消失 | 已复现并修复。未收到 Started 的失败/断线恢复原输入；保留用户随后编辑的文本和附件；已有 session 的不确定结果继续保护，不自动重发。 |
| Antigravity 4 | 会话 ID 从 None 变为 Some 后丢弃权限响应 | 已复现并修复。权限响应按权限 ID 更新；会话列表按 workspace 校验；会话绑定操作仍拒绝过期响应。没有照搬“所有结果都取消身份校验”的建议。 |
| Antigravity 5 | 不确定结果的 F5 提示永远不可达 | 已复现并修复。后台操作与不确定结果分别判断，保留草稿并给出 F5 路径。 |
| Antigravity 待核实 | 已提交草稿残留在内存 | 原报告的正常切回路径会消费缓存，不能按描述复现；进一步复现到“打开另一会话失败留下缓存副本，再提交”的真实问题。提交时消费当前 key，取得 session ID 时迁移恢复记录，完成后清理。 |
| ZCode 1 | 已提交输入重新成为普通未发送草稿 | 已复现并修复。草稿文件 v2 分开保存 `drafts` 与 `submissions`；后者只进入显式恢复入口。重放历史后可通过 Ctrl-K 恢复，不自动填入 composer。 |
| ZCode 2 | 小终端权限内容不可见却可以批准 | 已用 80×18 TestBackend 复现理由被裁切。目标、风险、命令和理由前置；审批至少需要 36×18，缩小后重新审阅。长内容用 PgUp/PgDn，确认不重置已浏览位置。 |
| ZCode 3 | 第二客户端静默覆盖同 workspace 草稿 | 已复现并修复。使用独立锁文件串行化缓存写入，并比较当前磁盘快照；冲突时保留文件与当前输入，显示未保存提示。没有加入并发草稿合并。空闲客户端也校验保存状态。 |
| ZCode 4 | 保存错误遮住所有操作提示 | 已复现并修复。操作反馈与保存错误各占一行；取消等待、F5、请求失败等提示保持可见。 |
| ZCode 待核实 | 有过程回合时焦点键可能被吞 | 完整读取 `process_key`，不支持的键返回 false。原焦点测试增加真实回合后仍通过，不改实现。 |
| ZCode 待核实／低优先级 | 长按审批键、保存去抖与退避 | 尚无真实物理长按复现或延时测量。没有用两次模拟 Press 冒充长按，也没有据此改变确认交互或增加后台写入架构。保留为后续实测项。 |
| ZCode 低优先级 | AGENTS 仍称草稿仅在内存 | 同步实际持久化及显式恢复边界。 |

## 兼容与恢复

- v2 的未发送草稿按 workspace/session 自动恢复文本、光标、附件路径；在途或不确定提交单独保存，通过 Ctrl-K 的 Recover 命令显式恢复。
- v1 无法区分两种内容，升级时全部保留为待核对输入，需要显式恢复。读取不修改旧文件；下一次有保存变动时写入 v2。未知版本、损坏文件不覆盖。
- 保存冲突时，先复制当前未保存输入，再关闭／重开该客户端读取最新内容。另一个客户端的已保存内容不被覆盖。
- 取消仍不等于停止 provider；F5 只有读到实际执行结束的记录后才解除不确定保护。

## 验证

新增 18 项单测，TUI 170 → 188；工作区 390 项通过，含 7 项预览的 all-targets 共 397 项通过。fmt、clippy `-D warnings`、release 构建通过。

| 主题 | 主要新增测试 |
|---|---|
| 首次失败与附件 | `first_submission_failure_before_started_keeps_a_restartable_draft`、`setup_failure_restores_only_an_empty_editor_and_never_retries_a_prompt`、`a_new_editor_draft_and_a_failed_first_submission_both_survive_restart` |
| 草稿生命周期 | `a_cached_new_conversation_draft_is_consumed_when_its_submission_gets_a_session`、`running_submission_without_a_session_id_requires_explicit_draft_recovery`、`loading_completed_history_keeps_a_recovered_submission_out_of_the_composer`、`explicit_recovery_does_not_replace_an_attachment_only_draft` |
| 存储 | `legacy_drafts_require_review_and_unknown_formats_are_preserved`、`a_second_client_cannot_silently_overwrite_another_clients_saved_draft`、`save_lock_contention_is_nonblocking_and_external_changes_invalidate_saved_status`、`invalid_attachment_paths_fail_serialization_without_panicking` |
| 权限／作用域 | `permission_results_are_applied_after_a_first_prompt_creates_its_session`、`session_lists_are_scoped_to_workspace_while_the_current_session_can_change`、`permission_decisions_require_a_readable_terminal_and_review_again_after_resize`、`permission_command_and_reason_are_visible_in_a_short_terminal` |
| 不确定结果 | `failure_after_started_keeps_execution_uncertain_and_does_not_restore_for_resend`、`an_uncertain_result_with_no_worker_explains_how_to_reconcile_without_resubmitting`、`cancellation_markers_do_not_unlock_new_sessions_until_execution_finishes` |

修复前先观察到 4 项回归失败，再观察到 ZCode 对应的 5 项回归失败；其中保存错误与 notice 同显更新的是原有测试。取消保护测试在修复前即通过，用于反证不应提前解锁。

发布版在隔离 POSIX PTY 中通过 10 项检查：中文多行保存、SIGKILL 后恢复、反向焦点、权限审阅、取消审阅、已保存退出提示、重复退出键保护、Esc 继续、终端恢复，以及 v1 显式恢复并写入 v2。测试不发送 provider prompt、不确认真实取消或审批，不停止用户的 daemon。

本地证据：`target/audit/tui-review/after/` 的 12–14 捕获分别展示 80×18 权限内容、保存错误与操作反馈同显、已提交输入不自动回填；它们来自实际 Ratatui renderer。`target/audit/tui-review/pty-v2/` 保存本轮 PTY 结果。图片与原始终端记录未提交到仓库。

```sh
cargo test --workspace
cargo test --workspace --all-targets
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --release -p baize-cli
python3 scripts/tui_interaction_smoke.py
```

本轮只验证当前 macOS 环境；并发草稿合并、provider 进程终止及全平台发布不在这些验证中。
