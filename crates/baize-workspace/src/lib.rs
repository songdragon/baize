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
