use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceStatus {
    pub root: PathBuf,
    pub git_root: Option<PathBuf>,
    pub branch: Option<String>,
    pub dirty: bool,
    pub changed_files: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffHunk {
    pub file_path: String,
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub header: String,
    pub lines: Vec<String>,
}

pub fn inspect(path: impl AsRef<Path>) -> Result<WorkspaceStatus> {
    let root = path.as_ref().canonicalize().with_context(|| {
        format!(
            "failed to canonicalize workspace path {}",
            path.as_ref().display()
        )
    })?;
    let git_root = git_output(&root, &["rev-parse", "--show-toplevel"])
        .ok()
        .map(PathBuf::from);
    let branch = git_output(&root, &["branch", "--show-current"]).ok();
    let porcelain = git_output(&root, &["status", "--porcelain"]).unwrap_or_default();
    let changed_files = porcelain
        .lines()
        .filter_map(porcelain_path)
        .filter(|line| !line.is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>();

    Ok(WorkspaceStatus {
        root,
        git_root,
        branch,
        dirty: !changed_files.is_empty(),
        changed_files,
    })
}

pub fn diff_hunks(path: impl AsRef<Path>) -> Result<Vec<DiffHunk>> {
    let root = path.as_ref().canonicalize().with_context(|| {
        format!(
            "failed to canonicalize workspace path {}",
            path.as_ref().display()
        )
    })?;
    let raw = git_output(&root, &["diff", "--no-ext-diff", "--unified=3"])?;
    Ok(parse_diff_hunks(&raw))
}

fn git_output(cwd: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .with_context(|| format!("failed to run git {}", args.join(" ")))?;

    if !output.status.success() {
        anyhow::bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn porcelain_path(line: &str) -> Option<&str> {
    line.get(2..).map(str::trim).filter(|path| !path.is_empty())
}

fn parse_diff_hunks(raw: &str) -> Vec<DiffHunk> {
    let mut hunks = Vec::new();
    let mut current_file = None::<String>;
    let mut current_hunk = None::<DiffHunk>;

    for line in raw.lines() {
        if let Some(path) = line.strip_prefix("+++ b/") {
            current_file = Some(path.to_string());
            continue;
        }
        if line.starts_with("@@ ") {
            if let Some(hunk) = current_hunk.take() {
                hunks.push(hunk);
            }
            if let (Some(file_path), Some((old_start, old_lines, new_start, new_lines))) =
                (current_file.clone(), parse_hunk_header(line))
            {
                current_hunk = Some(DiffHunk {
                    file_path,
                    old_start,
                    old_lines,
                    new_start,
                    new_lines,
                    header: line.to_string(),
                    lines: Vec::new(),
                });
            }
            continue;
        }
        if let Some(hunk) = current_hunk.as_mut() {
            hunk.lines.push(line.to_string());
        }
    }

    if let Some(hunk) = current_hunk {
        hunks.push(hunk);
    }

    hunks
}

fn parse_hunk_header(line: &str) -> Option<(u32, u32, u32, u32)> {
    let mut parts = line.split_whitespace();
    if parts.next()? != "@@" {
        return None;
    }
    let old_part = parts.next()?;
    let new_part = parts.next()?;
    Some((parse_range(old_part)?, parse_range(new_part)?)).map(|(old, new)| {
        let (old_start, old_lines) = old;
        let (new_start, new_lines) = new;
        (old_start, old_lines, new_start, new_lines)
    })
}

fn parse_range(part: &str) -> Option<(u32, u32)> {
    let trimmed = part.trim_start_matches(['-', '+']);
    let (start, lines) = trimmed.split_once(',').unwrap_or((trimmed, "1"));
    Some((start.parse().ok()?, lines.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn inspect_plain_directory_without_git() {
        let temp = tempfile::tempdir().expect("temp dir");
        let status = inspect(temp.path()).expect("inspect should work");

        assert_eq!(
            status.root,
            temp.path().canonicalize().expect("canonical path")
        );
        assert!(status.git_root.is_none());
        assert!(status.branch.is_none());
        assert!(!status.dirty);
        assert!(status.changed_files.is_empty());
    }

    #[test]
    fn inspect_clean_git_repository() {
        let temp = tempfile::tempdir().expect("temp dir");
        run_git(temp.path(), &["init", "-b", "main"]);

        let status = inspect(temp.path()).expect("inspect should work");

        assert_eq!(
            status.root,
            temp.path().canonicalize().expect("canonical path")
        );
        assert_eq!(
            status.git_root,
            Some(temp.path().canonicalize().expect("canonical path"))
        );
        assert_eq!(status.branch.as_deref(), Some("main"));
        assert!(!status.dirty);
        assert!(status.changed_files.is_empty());
    }

    #[test]
    fn inspect_git_repository_with_changed_files() {
        let temp = tempfile::tempdir().expect("temp dir");
        run_git(temp.path(), &["init", "-b", "main"]);
        fs::write(temp.path().join("tracked.txt"), "before\n").expect("write tracked file");
        run_git(temp.path(), &["add", "tracked.txt"]);
        run_git(temp.path(), &["commit", "-m", "initial"]);

        fs::write(temp.path().join("tracked.txt"), "after\n").expect("modify tracked file");
        fs::write(temp.path().join("new.txt"), "new\n").expect("write new file");

        let status = inspect(temp.path()).expect("inspect should work");

        assert!(status.dirty);
        assert_eq!(status.changed_files, vec!["tracked.txt", "new.txt"]);
    }

    #[test]
    fn parses_porcelain_paths_without_dropping_first_character() {
        assert_eq!(porcelain_path(" M tracked.txt"), Some("tracked.txt"));
        assert_eq!(porcelain_path("M  staged.txt"), Some("staged.txt"));
        assert_eq!(porcelain_path("?? new.txt"), Some("new.txt"));
    }

    #[test]
    fn parses_git_diff_hunks() {
        let raw = r#"diff --git a/src/lib.rs b/src/lib.rs
index 1111111..2222222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,2 +1,3 @@
 old
-before
+after
+new
"#;

        let hunks = parse_diff_hunks(raw);

        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].file_path, "src/lib.rs");
        assert_eq!(hunks[0].old_start, 1);
        assert_eq!(hunks[0].old_lines, 2);
        assert_eq!(hunks[0].new_start, 1);
        assert_eq!(hunks[0].new_lines, 3);
        assert_eq!(hunks[0].lines, vec![" old", "-before", "+after", "+new"]);
    }

    #[test]
    fn extracts_diff_hunks_from_git_repository() {
        let temp = tempfile::tempdir().expect("temp dir");
        run_git(temp.path(), &["init", "-b", "main"]);
        fs::write(temp.path().join("tracked.txt"), "before\nsame\n").expect("write tracked file");
        run_git(temp.path(), &["add", "tracked.txt"]);
        run_git(temp.path(), &["commit", "-m", "initial"]);

        fs::write(temp.path().join("tracked.txt"), "after\nsame\n").expect("modify tracked file");

        let hunks = diff_hunks(temp.path()).expect("diff hunks");

        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].file_path, "tracked.txt");
        assert!(hunks[0].lines.iter().any(|line| line == "-before"));
        assert!(hunks[0].lines.iter().any(|line| line == "+after"));
    }

    fn run_git(cwd: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .env("GIT_AUTHOR_NAME", "Baize Test")
            .env("GIT_AUTHOR_EMAIL", "baize@example.invalid")
            .env("GIT_COMMITTER_NAME", "Baize Test")
            .env("GIT_COMMITTER_EMAIL", "baize@example.invalid")
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
