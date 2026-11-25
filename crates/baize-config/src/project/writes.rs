use super::*;
use std::io::Write;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConfigChange {
    pub backup_id: String,
    pub status: String,
    pub verification: String,
}

#[derive(Serialize, Deserialize)]
struct Backup {
    version: u32,
    root: PathBuf,
    original: Option<String>,
    original_mode: Option<u32>,
    applied_digest: String,
}

/// Apply exactly the reviewed plan. Never writes global provider configuration.
pub fn apply(
    root: &Path,
    plan_id: &str,
    replace: &[String],
    backup_root: &Path,
) -> Result<ConfigChange> {
    let root = root.canonicalize()?;
    let _lock = lock(&root)?;
    let prepared = prepare(&root, replace)?;
    ensure!(
        prepared.plan.plan_id == plan_id,
        "Project configuration changed; review a fresh plan"
    );
    ensure!(
        prepared.plan.conflicts.is_empty(),
        "Configuration plan is blocked; review conflicts"
    );
    ensure!(
        !prepared.plan.changes.is_empty(),
        "Configuration already matches; nothing to apply"
    );
    let id = Uuid::new_v4().to_string();
    let backup = Backup {
        version: 1,
        root: root.clone(),
        original: prepared.original.clone(),
        original_mode: file_mode(&root.join(TARGET))?,
        applied_digest: digest(prepared.desired.as_bytes()),
    };
    let path = backup_path(backup_root, &root, &id)?;
    private_dir(path.parent().context("Backup directory unavailable")?)?;
    atomic_write(&path, &serde_json::to_vec(&backup)?, Some(0o600))?;
    safe_target(&root)?;
    ensure!(
        prepare(&root, replace)?.plan.plan_id == plan_id,
        "Project configuration changed during backup; no target written"
    );
    private_dir(&root.join(".codex"))?;
    atomic_write(&root.join(TARGET), prepared.desired.as_bytes(), Some(0o600))
        .with_context(|| format!("Apply failed; backup {id} is available for checked recovery"))?;
    Ok(ConfigChange {
        backup_id: id,
        status: "applied".into(),
        verification: "configured_only_not_runtime_verified".into(),
    })
}

/// Restore only if the current target still equals the applied file (or original).
pub fn restore(root: &Path, backup_id: &str, backup_root: &Path) -> Result<ConfigChange> {
    let root = root.canonicalize()?;
    let _lock = lock(&root)?;
    let path = backup_path(backup_root, &root, backup_id)?;
    let backup: Backup =
        serde_json::from_str(&read_with_limit(&path, MAX_BYTES * 8)?.context("Backup not found")?)
            .map_err(|_| anyhow::anyhow!("Invalid configuration backup"))?;
    ensure!(
        backup.version == 1 && backup.root == root,
        "Backup belongs to another workspace or version"
    );
    let current = read_optional(&root.join(TARGET))?;
    if current == backup.original && file_mode(&root.join(TARGET))? == backup.original_mode {
        return Ok(ConfigChange {
            backup_id: backup_id.into(),
            status: "already_restored".into(),
            verification: "configured_only_not_runtime_verified".into(),
        });
    }
    ensure!(
        current
            .as_ref()
            .is_some_and(|text| digest(text.as_bytes()) == backup.applied_digest),
        "Codex configuration changed since apply; refusing to overwrite later edits"
    );
    #[cfg(unix)]
    ensure!(
        file_mode(&root.join(TARGET))? == Some(0o600),
        "Codex configuration permissions changed since apply"
    );
    match backup.original {
        Some(text) => atomic_write(&root.join(TARGET), text.as_bytes(), backup.original_mode)?,
        None => {
            fs::remove_file(root.join(TARGET))?;
            sync_dir(&root.join(".codex"))?;
        }
    }
    Ok(ConfigChange {
        backup_id: backup_id.into(),
        status: "restored".into(),
        verification: "configured_only_not_runtime_verified".into(),
    })
}

/// Recover receipt IDs after client disconnects without returning original config values.
pub fn history(root: &Path, backup_root: &Path) -> Result<Vec<ConfigChange>> {
    let root = root.canonicalize()?;
    safe_target(&root)?;
    let parent = backup_path(backup_root, &root, &Uuid::nil().to_string())?
        .parent()
        .context("Missing backup directory")?
        .to_path_buf();
    let entries = match fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(error) => return Err(error.into()),
    };
    let current = read_optional(&root.join(TARGET))?;
    let mut result = vec![];
    for entry in entries {
        ensure!(
            result.len() < 1000,
            "Too many configuration backups to list at once"
        );
        let path = entry?.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let id = path
            .file_stem()
            .and_then(|value| value.to_str())
            .context("Invalid backup filename")?;
        let expected = backup_path(backup_root, &root, id)?;
        ensure!(
            path == expected && !fs::symlink_metadata(&path)?.file_type().is_symlink(),
            "Invalid backup file"
        );
        let backup: Backup = serde_json::from_str(
            &read_with_limit(&path, MAX_BYTES * 8)?.context("Backup disappeared")?,
        )
        .map_err(|_| anyhow::anyhow!("Unreadable configuration backup {id}"))?;
        ensure!(
            backup.version == 1 && backup.root == root,
            "Backup workspace/version mismatch"
        );
        let status = if current == backup.original {
            "restored"
        } else if current
            .as_ref()
            .is_some_and(|text| digest(text.as_bytes()) == backup.applied_digest)
        {
            "applied"
        } else {
            "later_changes_present"
        };
        result.push(ConfigChange {
            backup_id: id.into(),
            status: status.into(),
            verification: "configured_only_not_runtime_verified".into(),
        });
    }
    result.sort_by(|a, b| a.backup_id.cmp(&b.backup_id));
    Ok(result)
}

fn lock(root: &Path) -> Result<fs::File> {
    safe_target(root)?;
    private_dir(&root.join(".baize"))?;
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(root.join(".baize/project-config.lock"))?;
    file.try_lock()
        .map_err(|_| anyhow::anyhow!("Another project configuration change is in progress"))?;
    safe_target(root)?;
    Ok(file)
}

fn backup_path(backup_root: &Path, root: &Path, id: &str) -> Result<PathBuf> {
    ensure!(
        Uuid::parse_str(id).is_ok() && id.len() == 36,
        "Invalid backup ID"
    );
    Ok(backup_root
        .join(digest(root.to_string_lossy().as_bytes()))
        .join(format!("{id}.json")))
}

fn private_dir(path: &Path) -> Result<()> {
    if path.is_dir() {
        return Ok(());
    }
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn atomic_write(path: &Path, bytes: &[u8], mode: Option<u32>) -> Result<()> {
    let parent = path.parent().context("Target directory unavailable")?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file()
            .set_permissions(fs::Permissions::from_mode(mode.unwrap_or(0o600)))?;
    }
    #[cfg(not(unix))]
    let _ = mode;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|error| error.error)?;
    sync_dir(parent)
}

fn sync_dir(path: &Path) -> Result<()> {
    #[cfg(unix)]
    fs::File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuration_lock_is_exclusive_and_released_without_deleting_it() {
        let root = tempfile::tempdir().unwrap();
        let held = lock(root.path()).unwrap();
        assert!(lock(root.path()).is_err());
        drop(held);
        assert!(lock(root.path()).is_ok());
    }
}
