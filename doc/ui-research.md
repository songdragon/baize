# Rust UI 调研与 Baize 选型

核对日期：2026-11-21。依据引用讨论「查找优秀的 Rust UI 项目」及本次访问的官方站点和项目仓库。以下适配判断是 Baize 的工程判断，不是各框架的性能排名。产品约束以 [vision.md](vision.md)、[spec.md](spec.md) 为准。

## 决策

当前交付 Ratatui + Crossterm 的终端工作台，复用 Rust daemon 的 HTTP/SSE API。框架具备布局和渲染基础，产品质量主要取决于会话恢复、可靠交互、可读输出和失败处理。当前保留 lockfile 中的 Ratatui 0.29；上游页面已有后续版本信息，升级应另行验证兼容性，不能把“最新版本”当作本次重写的前提。

Baize 自身的 Desktop UI 从“默认 Tauri”调整为候选评估：优先验证 GPUI + GPUI Kit 的开发工具体验，同时保留 Iced、Slint、egui、Tauri。没有引入桌面运行时，也没有把这项研究当作 provider Desktop 执行能力已经接入。

## TUI 参考

| 官方来源 | 可参考的能力或模式 | Baize 的采用方式 |
|---|---|---|
| [Ratatui](https://ratatui.rs/) / [作品集](https://www.ratatui.rs/showcase/apps/) | Rust 终端布局、组件、测试后端 | 继续作为 TUI 基础；状态、事件处理、渲染分离 |
| [Yazi](https://github.com/sxyazi/yazi) | 异步终端文件管理器；列表与预览分栏 | 宽终端显示会话列表、主对话及按需检查面板；窄终端按焦点切换 |
| [Television](https://github.com/alexpasmantier/television) | 搜索、选择、预览的键盘流程 | 会话搜索和命令面板采用相同的过滤、选择、确认习惯；本次采用子串匹配，不宣称模糊排序 |
| [GitUI](https://github.com/gitui-org/gitui) | 面向 Git 的终端工作流 | 变更面板展示当前 Git 文件和 diff，明确最后刷新快照的时效 |
| [Presenterm](https://github.com/mfontanini/presenterm) | Markdown 终端排版与主题 | 参考内容层级、留白和颜色；本次不引入完整演示或 Markdown 渲染引擎 |
| [TachyonFX](https://github.com/ratatui/tachyonfx) | Ratatui 的效果与动画扩展 | 保留为可选项，当前不增加动效依赖；优先保证真实流事件和输入响应 |

上述项目用于设计研究，没有复制其源码。核心信息不能仅通过颜色表达，状态文本和键盘提示必须保留。不要求 Nerd Font。

## Desktop 候选

| 官方来源 | 路线 | Baize 评估重点 | 本次状态 |
|---|---|---|---|
| [GPUI](https://gpui.rs/) + [GPUI Kit](https://gpui-kit.com/) | Rust GPU UI；Kit 提供组件、停靠面板与数据展示能力 | 信息密度、长会话虚拟列表、输入法、辅助功能、macOS/Linux/Windows 分发 | 优先原型候选，尚无 Baize 原型验证 |
| [Iced](https://iced.rs/) | Rust 声明式 GUI，状态与消息驱动 | 聊天输入、布局、主题、长期维护与跨平台交互 | 保留候选 |
| [Slint](https://slint.dev/) | 独立声明式 UI 语言与 Rust 逻辑 | 自定义视觉、设计工具、动态内容及发布许可条件 | 保留候选，采用前核查具体许可 |
| [egui](https://github.com/emilk/egui) | 即时模式 Rust GUI | 调试与证据检查效率；聊天阅读、文本编辑体验需实际验证 | 工程工具方向候选 |
| [Tauri 2](https://v2.tauri.app/) | Rust shell + 系统 WebView | Web 文本和组件生态、sidecar 生命周期、各系统 WebView 差异 | Web 前端路线候选 |

GPUI Kit 的当前站点同时介绍 gpui-component、gpui-base、gpui-shell 等层次；旧名称不能用来推断兼容 API。候选选型不依赖 star 数、宣传语或未经测量的性能对比。

桌面评估必须运行相同的会话恢复、A → B → A、草稿、权限、错误与大输出场景，并核对平台、许可证和可访问性。视觉相似不能证明执行入口能力等效。

## 执行环境边界

TUI 和未来 Baize Desktop 都只是 daemon 客户端。它们不能凭自身 UI 框架获得 Codex、Antigravity 等产品 Desktop 的内置 prompts、MCP、skills、computer/browser use。provider 入口能力仍按 [provider-context-research.md](provider-context-research.md) 的证据与未知项报告；本次 UI 核对没有重验全部 provider。

设计与可执行验收见 [tui-design.md](tui-design.md)。
