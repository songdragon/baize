# 跨 Agent 项目配置

跟踪 [issue #15](https://github.com/songdragon/baize/issues/15)。本次支持项目根目录的 Codex app-server 与 Antigravity CLI，不涉及 Desktop、ACP、全局配置迁移或所有原生能力的等价接入。

## 一份源文件

```text
<project>/
  AGENTS.md                         # 两个入口原生发现的共同项目指令
  .agents/skills/<name>/SKILL.md     # 两个入口复用的单份 skill
  .agents/mcp_config.json           # MCP 定义源；Antigravity 原生读取
  .codex/config.toml                # 经审阅应用后的 Codex 项目映射
```

Baize 不复制 AGENTS.md 或 skill 目录，也不替换原生 prompt。已有 skill 若需复用，应保留 canonical 目录并使用相对 symlink。界面只列出当前项目根的文件，不宣称包含父目录、用户、插件或受管配置的完整生效清单。发现 SKILL.md 不代表其 frontmatter 已通过原生校验或该 skill 已启用。Codex 的 AGENTS.override.md、原生 skill 开关及其他规则仍可能改变实际行为。

Codex 原生信任是独立前置条件：未受信任的项目可能忽略 `.codex/config.toml`。请在项目中打开原生 Codex 审阅其信任决定；Baize 不自动授予信任。`in_sync` 只表示配置映射就位，不表示 MCP 已连接、工具已执行或两个入口能力相同。

## 审阅、应用与恢复

先使用同版本 daemon/TUI；CLI 和 TUI 会核对 `runtime.capabilities.project_config >= 1`，不支持时明确要求重启 daemon，不发送配置请求。

```sh
baize project-config --path /path/to/project plan

# 把输出中的精确 plan_id 放在这里；计划过期时重新审阅
baize project-config --path /path/to/project apply <plan_id>

# 原生同名定义不一致时，先明确选择替换，再审阅新计划
baize project-config --path /path/to/project plan --replace server_name
baize project-config --path /path/to/project apply <new_plan_id> --replace server_name

# 重启或丢失响应后仍可找回 backup_id
baize project-config --path /path/to/project history
baize project-config --path /path/to/project restore <backup_id>
```

不指定 `--path` 时使用当前目录注册的项目根。所有 CLI 写操作都通过 daemon，TUI 通过 Ctrl-K → **Review shared project configuration** 打开同一份只读计划，保留未发送草稿。TUI 不执行终端命令或直接修改 provider 文件。

计划绑定项目、MCP 源文件、原 Codex 配置及权限和完整替换选项。源或目标改变后，旧计划不能应用。同名差异默认阻止写入；指定 `--replace` 只允许计划中明确的替换，不解除原生策略保护。

工作区中任何运行或未确认的回合都会阻止应用和恢复；只读审阅和历史查询仍可用。配置写操作互斥，进程结束后文件锁由系统释放，不需要删除遗留锁来解锁。备份先持久化，再原子替换目标；恢复会检查目标仍等于已应用版本及其权限，拒绝覆盖后续编辑。原文件不存在时，恢复只删除本次生成的 Codex 文件，保留共同源和其他文件。

备份位于 daemon 数据目录的 `artifacts/project-config/<project-hash>/`，原生配置值不会通过计划、历史列表或事件返回。Unix 备份和生成文件为 0600；恢复保留原文件内容与原权限。源文件中的认证值不会被转换成公开审阅文本；认证应继续使用 provider 原生机制，不在共享项目文件中提交凭据。

## MCP 映射范围

项目 `.agents/mcp_config.json` 的根为 `mcpServers`。例如以下模板需要替换成真实本地 MCP 服务，不会安装服务或自动执行安装命令：

```json
{
  "mcpServers": {
    "project_tools": {
      "command": "/absolute/path/to/mcp-server",
      "args": [],
      "disabledTools": ["delete_all"]
    }
  }
}
```

| 源字段 | Codex 项目字段 |
| --- | --- |
| command / args / env / cwd | command / args / env / cwd；默认 cwd 为项目根 |
| serverUrl 或 url / headers | url / http_headers；Streamable HTTP |
| disabled | enabled 的反值 |
| enabledTools / disabledTools | enabled_tools / disabled_tools |
| timeoutSeconds | 同时设置 startup_timeout_sec 和 tool_timeout_sec |

仅接受该公共子集；不展开环境变量模板或 shell 表达式。HTTP 与 stdio 字段不能混用。超时限 1..3600 秒；原生无限等待、默认超时差异和空 enabledTools 不被猜测转换。重复 server 名、未知字段、额外认证、每工具 eager/background 等无法等价映射的字段会阻止整份计划应用，不会部分写入。

原生 model、sandbox、审批策略、插件等无关设置及其他 MCP server 保留。映射不会降低现有工具审批策略、放宽原生工具 allow/deny 列表或重新启用被原生禁用的 server。匹配的连接保留原生认证扩展；涉及这些扩展的连接替换需要先在原生配置中明确迁移。传输类型变化也不会自动迁移认证。

这是审阅后合并，不是自动删除同步：从源文件移除 server 不会删除 Codex 定义，界面会把它列为保留的 Codex-only server。需要移除时使用原生配置操作或受检查的备份恢复。

## API

- `GET /workspaces/:id/project-config`：只读计划，不创建备份、不启动 provider。
- `POST /workspaces/:id/project-config`：`action` 为 `plan`、`apply`、`history` 或 `restore`；`apply` 需 plan_id 和与审阅一致的 replace 列表，`restore` 需 backup_id。
- 文件计划与恢复只证明配置文件状态；`verification` 始终为 `configured_only_not_runtime_verified`。真实调用证据独立记录。

## 验证

自动化用临时目录、SQLite、HTTP ServiceExt、TestBackend 和本地 MCP 协议 fixture，不调用模型。真实验收需显式开启，使用已受信任仓库的专用隔离 worktree、独立数据与端口；不会写用户全局信任或配置。

```sh
cargo test --workspace
cargo test --workspace --all-targets
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
python3 -m unittest discover -s scripts -p 'test_*acceptance.py' -v
cargo build --release -p baize-daemon --example continuity_daemon
python3 scripts/shared_config_acceptance.py --allow-real-providers \
  --daemon target/release/examples/continuity_daemon \
  --output target/audit/shared-config/<new-directory>
```

脚本先用官方 config/read 与 skills/list 做不提交 turn 的原生预检，再要求 Codex → Antigravity → Codex 各遵守共同指令、使用同份 skill 并调用只读 MCP。MCP 现场生成的 nonce 不在 prompt 或工作区文件中，实际调用日志和原生工具事件共同核验；返回 Codex 还须恢复原 native ID。最后检查备份可发现、恢复受保护且源文件未被修改。

2026-11-26 的最终原生验收为 `target/audit/shared-config/real-20261126-01/`：3 轮真实调用、19 项检查全部通过，实际 MCP 调用恰为 3 次；逐条复核最终回复均以共同项目指令规定的标记开头。3 个模型回合期间工作区与 canonical 源未被改动；最后的恢复只移除生成的 Codex 配置。用户全局 provider 配置摘要前后一致，生产 daemon 未重启。

最终自动化为 497 项 workspace、505 项 all-targets、4 项 Python 测试通过，fmt、clippy `-D warnings`、release 构建通过。最终 CLI 的 10 项 PTY 交互回归通过且没有提交模型请求。原生 daemon 对应实现提交 `ba57aa8`；后续 Rust 改动只补 TUI 的旧 daemon 兼容保护与回归，不改变已验收的原生执行路径。摘要见 [验收记录](acceptance/shared-project-config-20261126.json)，原始记录保存在本地忽略的 target/audit。

## 依据

2026-11-26 核对本机 Codex 0.171.0 schema 与配置层、Antigravity 1.5.17 CLI。主要文档：[Codex MCP](https://developers.openai.com/codex/mcp)、[Codex skills](https://developers.openai.com/codex/skills)、[Antigravity rules](https://antigravity.google/docs/rules.md)、[skills](https://antigravity.google/docs/skills.md)、[MCP](https://antigravity.google/docs/mcp.md)。本机独立未受信任 fixture 的配置层被禁用；同仓库隔离 worktree 的配置层可加载。这些检查没有提交模型回合。
