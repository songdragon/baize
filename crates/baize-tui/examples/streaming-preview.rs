//! Offline, interactive review of folded process output. Never contacts a daemon.
#[path = "streaming_preview/model.rs"]
mod model;
#[path = "streaming_preview/view.rs"]
mod view;

use anyhow::{bail, Context, Result};
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use model::Preview;
use ratatui::{backend::CrosstermBackend, backend::TestBackend, Terminal};
use std::{io, path::Path, time::Duration};

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [] => interactive(),
        [flag, directory] if flag == "--capture" => capture(Path::new(directory)),
        _ => bail!("usage: streaming-preview [--capture DIRECTORY]"),
    }
}

fn interactive() -> Result<()> {
    enable_raw_mode()?;
    if let Err(error) = execute!(io::stdout(), EnterAlternateScreen) {
        disable_raw_mode()?;
        return Err(error.into());
    }
    let result = run_terminal();
    let restore_raw = disable_raw_mode();
    let restore_screen = execute!(io::stdout(), LeaveAlternateScreen);
    result?;
    restore_raw?;
    restore_screen?;
    Ok(())
}

fn run_terminal() -> Result<()> {
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let mut preview = Preview::default();
    loop {
        preview.tick();
        terminal.draw(|frame| view::render(frame, &preview))?;
        if !event::poll(Duration::from_millis(80))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        if matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
            || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
        {
            return Ok(());
        }
        match key.code {
            KeyCode::Char(' ') => preview.expanded = !preview.expanded,
            KeyCode::Enter if preview.expanded => preview.details = !preview.details,
            KeyCode::Up if preview.expanded => preview.select(-1),
            KeyCode::Down if preview.expanded => preview.select(1),
            KeyCode::PageUp => preview.scroll = preview.scroll.saturating_add(5),
            KeyCode::PageDown => preview.scroll = preview.scroll.saturating_sub(5),
            KeyCode::Home => preview.scroll = usize::MAX,
            KeyCode::End => preview.scroll = 0,
            KeyCode::Char('1') => preview = Preview::snapshot(3, false),
            KeyCode::Char('2') => preview = Preview::snapshot(10, false),
            KeyCode::Char('3') => preview = Preview::snapshot(10, true),
            KeyCode::Char('r') => preview = Preview::default(),
            KeyCode::Char('p') => preview.paused = !preview.paused,
            _ => {}
        }
    }
}

fn capture(directory: &Path) -> Result<()> {
    std::fs::create_dir_all(directory).context("create preview capture directory")?;
    for (name, stage, expanded, details) in [
        ("running", 3, false, false),
        ("answering", 6, false, false),
        ("completed", 10, false, false),
        ("expanded", 10, true, false),
        ("detail", 10, true, true),
    ] {
        let mut preview = Preview::snapshot(stage, expanded);
        preview.selected = 2;
        preview.details = details;
        let height = if details { 44 } else { 38 };
        let mut terminal = Terminal::new(TestBackend::new(112, height))?;
        terminal.draw(|frame| view::render(frame, &preview))?;
        let buffer = terminal.backend().buffer();
        let cells: Vec<_> = buffer
            .content
            .iter()
            .map(|cell| {
                serde_json::json!({
                    "symbol": cell.symbol(), "fg": format!("{:?}", cell.fg),
                    "bg": format!("{:?}", cell.bg),
                    "bold": cell.modifier.contains(ratatui::style::Modifier::BOLD),
                })
            })
            .collect();
        let contents = serde_json::to_vec(&serde_json::json!({
            "width": 112, "height": height, "cells": cells,
            "source": "Ratatui TestBackend: same renderer as the interactive terminal example",
        }))?;
        std::fs::write(directory.join(format!("{name}.json")), contents)?;
    }
    println!(
        "Captured five real Ratatui frames in {}",
        directory.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_exports_actual_terminal_cells_and_rejects_file_as_directory() {
        let path = std::env::temp_dir().join(format!("baize-fold-preview-{}", std::process::id()));
        capture(&path).unwrap();
        let frame: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path.join("completed.json")).unwrap()).unwrap();
        assert_eq!(frame["cells"].as_array().unwrap().len(), 112 * 38);
        assert!(capture(&path.join("completed.json")).is_err());
        std::fs::remove_dir_all(path).unwrap();
    }
}
