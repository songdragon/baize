use std::time::{Duration, Instant};

pub(super) const TOOLS: [(&str, &str, &str); 5] = [
    ("读取", "README.md", "88 行 · 5.0 KB"),
    ("读取", "Cargo.toml", "44 行 · 1.2 KB"),
    ("读取", "doc/vision.md", "115 行 · 7.4 KB"),
    ("读取", "doc/mvp-implementation-plan.md", "433 行 · 20.9 KB"),
    ("整理", "项目架构与当前实现边界", "已完成"),
];

// Chunks represent fixture response events, not a locally generated typing animation.
pub(super) const ANSWER: [&str; 14] = [
    "Baize 是一个让你在多个 coding agent 之间切换、并保留同一段开发会话的工具。",
    "你可以先让 Codex 分析，再切换到 Antigravity 继续处理，而不用从头交代背景。",
    "",
    "它解决什么问题",
    "• 连续对话：保留近期交流、当前工作区事实和仍然有效的约束。",
    "• 切换 agent：通过统一会话编排 CLI，记录执行事件和交接信息。",
    "• 本地掌控：数据存放在本地 SQLite，TUI 通过 daemon 操作会话。",
    "",
    "项目如何组成",
    "TUI / CLI → daemon → provider adapters → Codex / Antigravity / OpenCode",
    "daemon 负责会话、路由、权限和事件持久化；TUI 负责交互与结果展示。",
    "",
    "目前已实现 CLI 路径；Provider Desktop 执行和完整的跨 agent 能力等价仍待验证。",
    "它的目标，是让 agent 可以更换，而你的开发上下文能够延续。",
];

pub(super) struct Preview {
    pub stage: usize,
    pub expanded: bool,
    pub details: bool,
    pub selected: usize,
    pub scroll: usize,
    pub paused: bool,
    pub failed: bool,
    updated: Instant,
}

impl Default for Preview {
    fn default() -> Self {
        Self {
            stage: 0,
            expanded: false,
            details: false,
            selected: 0,
            scroll: 0,
            paused: false,
            failed: false,
            updated: Instant::now(),
        }
    }
}

impl Preview {
    pub(super) fn snapshot(stage: usize, expanded: bool) -> Self {
        Self {
            stage: stage.min(10),
            expanded,
            paused: true,
            ..Self::default()
        }
    }

    pub(super) fn tick(&mut self) {
        if !self.paused && self.stage < 10 && self.updated.elapsed() >= Duration::from_millis(850) {
            self.stage += 1;
            self.updated = Instant::now();
        }
    }

    pub(super) fn select(&mut self, delta: isize) {
        self.selected = self.selected.saturating_add_signed(delta).min(4);
        self.details = false;
    }

    pub(super) fn response_lines(&self) -> &'static [&'static str] {
        let count = match self.stage {
            0..=4 => 0,
            5 => 2,
            6 => 7,
            7 => 11,
            8..=9 => 13,
            _ => ANSWER.len(),
        };
        &ANSWER[..count]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_arrives_in_fixture_chunks_and_selection_stays_in_bounds() {
        assert!(Preview::snapshot(3, false).response_lines().is_empty());
        assert_eq!(Preview::snapshot(5, false).response_lines().len(), 2);
        assert_eq!(Preview::snapshot(99, false).response_lines(), ANSWER);
        let mut preview = Preview::default();
        preview.select(-1);
        assert_eq!(preview.selected, 0);
        preview.select(99);
        assert_eq!(preview.selected, 4);
    }

    #[test]
    fn playback_advances_but_paused_and_completed_states_are_stable() {
        let mut preview = Preview {
            updated: Instant::now() - Duration::from_secs(2),
            ..Preview::default()
        };
        preview.tick();
        assert_eq!(preview.stage, 1);
        preview.paused = true;
        preview.updated -= Duration::from_secs(2);
        preview.tick();
        assert_eq!(preview.stage, 1);
        preview.stage = 10;
        preview.paused = false;
        preview.tick();
        assert_eq!(preview.stage, 10);
    }
}
