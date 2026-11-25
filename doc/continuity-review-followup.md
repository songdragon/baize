# 连续会话原生应用审阅跟进

日期：2026-11-25。PR：[#11](https://github.com/songdragon/baize/pull/11)。审阅范围固定为 `13d4051..25a2dd9`，本文件记录随后修复，不把测试替身当作真实 provider 验收。

## 审阅入口与状态

- Antigravity：通过 computer use 创建 `Baize PR #11 Code Review` 会话，模型 Gemini 3.8 Flash Medium，会话 ID `7e23d3e0-24fb-428f-843a-b1190af16497`。已读取最终六条意见。要求只读代码、不给真实 provider 发测试请求、不修改或提交文件。
- ZCode：通过 computer use 创建独立 Baize PR #11 审阅任务，模型 GLM-5.4，最高档，保留“变更前确认”。只对已显示的只读命令给予单次允许。用户解锁 Mac 后继续，已读取 `基于静态证据输出最终 Review Comments` 的最终报告：4 条 P2、1 条 P3。范围仍固定 `25a2dd9`；TUI 仅 grep 级核对，测试和验收脚本未读，未由 ZCode 运行测试。两份报告均为静态审阅，不代表原生 provider 能力验收。

## Antigravity 意见的独立复核

| 意见 | 复核与处置 | 验证 |
| --- | --- | --- |
| P1：取消或结果提交冲突时清除 dispatched，使未知执行失去保护 | 采纳问题，细化方案：未知结果保留租约；有结束证据的结果与忽略事件原子保存并释放租约。取消也在同一存储锁及事务内更新，避免旧快照覆盖新状态。不能把所有取消都当作未知，也不能盲目清除保护。 | 取消先于 Timeout、worker error、已知完成结果的回归；重启及准确 ID 核对后再次重启。 |
| P1：dispatch 前内部错误缺少 not_submitted | 采纳。确定尚未发送的加载、上下文与路由错误明确返回该标记。未采纳“没有 attempt ID 就不进入未知状态”的建议：网络中断可能丢失 ID，不能据此安全重发；现有 F5 可核对，不能概括为永久锁死。 | 缺失 workspace 返回 500 + not_submitted，零 executor 请求，释放尚未 dispatch 的租约。 |
| P2：/requirements 查询写行、事件且被运行租约阻止 | 采纳。daemon 提供纯只读视图；TUI 使用 GET 查询，执行中也可查看，不覆盖 provider 提交记录或新草稿。 | idle/active 两种状态前后会话、时间戳和事件完全相同；TUI 查询及繁忙输入保护。 |
| P2：Antigravity terminal error 解析影响所有 provider | 采纳。解析仅适用于 Antigravity。合成同名 JSON 验证函数隔离契约，不声称 Codex 正常输出必然具有该格式。 | 三 provider 的合成退出码 0 结果：仅 Antigravity 报拒绝。 |
| P2：active_provider_id 为 None 时不能使用显式目标 | 采纳为旧数据兼容修复。当前创建接口通常会填 active provider；租约应记录已经解析好的本次执行者，并修正路由的 previous-provider 来源。 | 旧 session 无 active provider，指定 Antigravity 可正常执行。 |
| P2：入口身份变化时预览可发送，实际发送却要求重置 | 采纳。预览和发送均阻止入口身份不匹配，并提示 /reset-provider；不再误报为预算不足。身份范围仍是实现中的命令、传输、prompt 入口及项目根目录，不扩张为所有模型或策略设置。 | preview.blocked、重置提示及发送 409 + not_submitted。 |

## Double check 补充修复

- 本地命令返回嵌套 session.continuity 时同步未知执行标记，避免核对成功后仍禁用操作；显示要求来源。
- Context 区分未发送预览与最近提交的上下文及结果；取消响应同步其他客户端启动的租约。
- /unimage 等本地附件操作可以在未知执行期间使用，允许移除后续草稿图片后进行核对，不解除执行保护。
- 旧版裸 session.agent.ignored 缺少结束证据，不能在升级恢复时释放租约；显式 uncertain 或 Timeout 优先于数字退出码。保留未知保护并补迁移回归。
- 连续会话能力升级为协议 2，新 CLI/TUI 拒绝向旧 daemon 提交，避免只升级客户端后继续使用存在取消竞争问题的服务端。

## ZCode 意见的独立复核

| 意见 | 复核与处置 | 验证 |
| --- | --- | --- |
| P2：resume 返回不同原生 ID，却为新 ID 写入增量回执 | 采纳。成功结果仍保存新 ID，但增量输入不能证明它知道旧原生历史；保留上一份已确认回执，不为新 ID 推进位置。下一轮因 ID 不匹配补齐完整上下文并提示覆盖未验证，成功后再更新回执。未验证真实 CLI 是否会静默换 ID，不据此扩大为已复现的 provider 故障。 | fake executor 的 c1 → c2：旧历史未在增量中发送，回执不推进；第三轮补齐历史后确认 c2。 |
| P2：refresh_token 等字段名使用户历史整行脱敏，却显示完整包含 | 采纳。仅出现字段名不再视作凭据，赋值/认证信息仍脱敏；历史实际脱敏时明确警告、标记 truncated，并保留 pending，不能当作完整交付。未采纳直接关闭用户历史脱敏的建议。 | 普通中文字段名原话完整交付；user/assistant 的 API_KEY 赋值都脱敏、保留安全行并维持 pending；JSON 预览继续脱敏。 |
| P2：工作区清单及撤销全文占满强制上下文 | 部分采纳。改动文件清单最多 32 个、单路径受现有长度限制，保留省略数量和明确警告；撤销要求保留 key/修订/来源及撤销标记，正文仍保存在数据库，不再重复放入 prompt。未采纳“强制段超限也截断发送”：有效要求和必须承接的用户消息仍完整保留，超预算拒绝发送。此阻塞可通过配置、要求替换或聚焦新会话恢复，不等于永久锁死。 | 2,000 个变更文件在小预算下可发送并显示 +1968 more；长撤销正文不阻塞，恢复为 active 时同预算仍拒绝。 |
| P2：Draft 约束提前生效、重复导入后难以撤销 | 采纳。只导入 Accepted；预览背景也不把 Draft 约束列为有效要求。按首尾去空白后的文本与已有要求去重，保留首个来源和 key；撤销后的重复交接不会复活。既有已导入记录保留审计，不自动删除旧约束。 | 三份重复 Draft 不生效；两份接受后只有一条要求；撤销再接受第三份仍只有已撤销的原条目；零 executor 调用。 |
| P3：dispatch 前失败缺少 not_submitted | 已由 `28e79dc` 修复，不重复修改。 | 既有缺失 workspace 回归保持通过，确认零 executor 调用及租约释放。 |

ZCode 的未确认候选也做了定向检查：stream normalizer 的累计值只写入内部 last_output，返回事件仍保留 delta；未据此修改流拼接。TUI 实际读超时为 120 + 5 秒，网络/收尾超时仍按未知结果核对，未证明需要新的超时策略。handoff 产物写入 data_dir/artifacts/handoffs，预览保留现有脱敏。其他 Antigravity 拒绝形态缺少真实输出证据，没有以模糊字符串匹配扩大失败判定。长会话无法容纳所有必要历史、旧执行错误保留未知保护均维持已记录的产品边界。

## 验证

Antigravity 阶段（`28e79dc`）：审阅前基线 422 项，本阶段新增 15 项，10 个回归测试在修复对应实现之前观察到失败。ZCode 阶段另新增 6 项，六项均在修复前观察到失败，修复后通过。使用 fake executor、临时 SQLite/Git、TestBackend 和合成命令输出，自动化未调用真实模型。

- `cargo test --workspace`：443 通过，0 失败。
- `cargo test --workspace --all-targets`：450 通过，0 失败。
- `cargo fmt --all -- --check`：通过。
- `cargo clippy --workspace --all-targets -- -D warnings`：通过，0 警告。
- `cargo build --release -p baize-cli`：通过。
- Antigravity 阶段发布版 TUI 的 10 项 POSIX PTY 交互回归通过，覆盖草稿崩溃恢复、中文多行粘贴、退出/取消确认、重复退出快捷键保护及终端恢复；零 provider prompt。结果：`target/audit/continuity/review-25a2dd9/pty/result.json`。ZCode 阶段修改 daemon 上下文，不重复声称该结果属于新提交；该检查读取现有 daemon 元数据，不作为新协议端到端验收。

本机日志：Antigravity 阶段为 `/tmp/baize-continuity-review-final-{tests,all-targets,fmt,clippy,release}.log`；ZCode 阶段为 `/tmp/baize-zcode-review-final-{tests,all-targets,fmt,clippy,release}.log`。原生应用的代码审阅使用模型；它们与自动化测试中不调用 provider 的约束分别记录。

两轮审阅修复合并后，已于 2026-11-25 在 main 合并提交 `3aad6b7` 重跑 Codex → Antigravity → Codex：3 次真实调用、12 项检查全部通过，三阶段 fixture 测试为 7 / 8 / 9 项；原始证据为 `target/audit/continuity/real-20261125-05/`。源提交、版本、原生引用和摘要见 [验收结果](acceptance/continuity-20261125.json)，说明见 [连续会话文档](reliable-agent-continuity.md)。旧 daemon 未停止或替换。
