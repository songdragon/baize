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
        .filter_map(|line| line.get(3..).map(str::trim))
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
