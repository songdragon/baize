use super::*;
use std::fs::{self, OpenOptions};

pub(super) struct DraftStore {
    path: PathBuf,
    workspace: String,
    previous: Value,
}

pub(super) fn default_draft_path() -> PathBuf {
    std::env::var_os("BAIZE_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::data_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("baize")
        })
        .join("tui-drafts.json")
}

fn read_document(path: &Path) -> Result<Value> {
    let mut data = match fs::read(path) {
        Ok(bytes) => serde_json::from_slice::<Value>(&bytes).context("parse draft file")?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            json!({"version":2,"workspaces":{}})
        }
        Err(error) => return Err(error).context("read draft file"),
    };
    anyhow::ensure!(
        matches!(data["version"].as_u64(), Some(1 | 2)) && data["workspaces"].is_object(),
        "unsupported or malformed draft file"
    );
    if data["version"] == 1 {
        // V1 mixed unsent input and possibly executed submissions. Preserve
        // every entry, but require explicit recovery instead of guessing its kind.
        if let Some(workspaces) = data["workspaces"].as_object_mut() {
            for value in workspaces.values_mut() {
                *value = json!({"drafts":{}, "submissions":value.clone()});
            }
        }
        data["version"] = json!(2);
    }
    Ok(data)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("draft path has no parent")?;
    fs::create_dir_all(parent).context("create draft directory")?;
    let temporary = parent.join(format!(".tui-drafts-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .context("create temporary draft file")?;
        file.write_all(bytes).context("write client drafts")?;
        file.sync_all().context("flush client drafts")?;
        fs::rename(&temporary, path).context("replace client draft file")?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

impl DraftStore {
    pub(super) fn open(path: PathBuf, workspace: String, state: &mut TuiState) -> Result<Self> {
        let document = read_document(&path)?;
        let value = document["workspaces"]
            .get(&workspace)
            .cloned()
            .unwrap_or_else(|| json!({"drafts":{}, "submissions":{}}));
        let drafts: DraftMap =
            serde_json::from_value(value["drafts"].clone()).context("decode workspace drafts")?;
        let recovery: DraftMap = serde_json::from_value(value["submissions"].clone())
            .context("decode recoverable submissions")?;
        let count = drafts.len();
        let recovery_count = recovery.len();
        state.ui.drafts = drafts;
        state.ui.recovery_drafts = recovery;
        restore_draft(state);
        state.ui.drafts_persisted = true;
        if recovery_count > 0 {
            state.ui.notice = Some("Stored submissions or legacy drafts need Ctrl-K recovery after checking history. Ctrl-L opens the latest session.".into());
        } else if count > 0 {
            state.ui.notice = Some("Local drafts recovered. Review recorded history before sending; Ctrl-L opens the latest session.".into());
        }
        Ok(Self {
            path,
            workspace,
            previous: value,
        })
    }

    pub(super) fn checkpoint(&mut self, state: &mut TuiState) {
        remember_recovery_draft(state);
        match self.save(state) {
            Ok(()) => {
                state.ui.drafts_persisted = true;
                state.ui.draft_error = None;
            }
            Err(error) => {
                state.ui.drafts_persisted = false;
                state.ui.draft_error = Some(format!("Drafts not saved: {error:#}"));
            }
        }
    }

    fn save(&mut self, state: &TuiState) -> Result<()> {
        let current = draft_snapshot(state)?;
        let _guard = if current == self.previous {
            None // Idle checks read the atomically replaced file without taking a writer lock.
        } else {
            let parent = self.path.parent().context("draft path has no parent")?;
            fs::create_dir_all(parent).context("create draft directory")?;
            let mut options = OpenOptions::new();
            options.read(true).write(true).create(true).truncate(false);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            // Lock a separate inode and never wait on another client in the UI thread.
            let guard = options
                .open(self.path.with_extension("lock"))
                .context("open draft cache lock")?;
            guard
                .try_lock()
                .map_err(|error| anyhow!("draft cache busy or unavailable: {error}"))?;
            Some(guard)
        };
        let mut document = read_document(&self.path)?;
        let on_disk = document["workspaces"]
            .get(&self.workspace)
            .cloned()
            .unwrap_or_else(|| json!({"drafts":{}, "submissions":{}}));
        anyhow::ensure!(
            on_disk == self.previous,
            "drafts changed in another client; copy your input before restarting this client"
        );
        if current == self.previous {
            return Ok(());
        }
        // Serialize read/modify/write across workspaces and detect stale writers
        // within a workspace. This deliberately does not merge concurrent edits.
        document["workspaces"][&self.workspace] = current.clone();
        write_atomic(&self.path, &serde_json::to_vec_pretty(&document)?)?;
        self.previous = current;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_drafts_require_review_and_unknown_formats_are_preserved() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("drafts.json");
        let legacy = json!({"version":1,"workspaces":{"one":{"": ["legacy input",2,["old.png"]]}}});
        let original = serde_json::to_vec(&legacy).unwrap();
        fs::write(&path, &original).unwrap();
        let mut state = TuiState::default();
        let mut store = DraftStore::open(path.clone(), "one".into(), &mut state).unwrap();
        assert!(state.input.is_empty());
        assert_eq!(
            state.ui.last_submission.as_ref().unwrap().prompt,
            "legacy input"
        );
        assert_eq!(fs::read(&path).unwrap(), original);
        state.input = "new unsent draft".into();
        store.checkpoint(&mut state);
        assert_eq!(read_document(&path).unwrap()["version"], 2);
        let mut restored = TuiState::default();
        DraftStore::open(path.clone(), "one".into(), &mut restored).unwrap();
        assert_eq!(restored.input, "new unsent draft");
        assert_eq!(
            restored.ui.last_submission.as_ref().unwrap().prompt,
            "legacy input"
        );
        let future = br#"{"version":99,"workspaces":{}}"#;
        fs::write(&path, future).unwrap();
        assert!(DraftStore::open(path.clone(), "one".into(), &mut TuiState::default()).is_err());
        assert_eq!(fs::read(path).unwrap(), future);
    }

    #[test]
    fn save_lock_contention_is_nonblocking_and_external_changes_invalidate_saved_status() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("drafts.json");
        let mut state = TuiState::default();
        let mut store = DraftStore::open(path.clone(), "one".into(), &mut state).unwrap();
        let guard = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path.with_extension("lock"))
            .unwrap();
        guard.lock().unwrap();
        state.input = "local draft".into();
        store.checkpoint(&mut state);
        assert!(!state.ui.drafts_persisted && !path.exists());
        drop(guard);
        store.checkpoint(&mut state);
        assert!(state.ui.drafts_persisted);
        let mut changed = read_document(&path).unwrap();
        changed["workspaces"]["one"]["drafts"][""][0] = json!("external draft");
        fs::write(&path, serde_json::to_vec(&changed).unwrap()).unwrap();
        store.checkpoint(&mut state); // No local editing since the last successful save.
        assert!(!state.ui.drafts_persisted);
        assert!(state
            .ui
            .draft_error
            .as_deref()
            .unwrap()
            .contains("another client"));
        assert_eq!(
            read_document(&path).unwrap()["workspaces"]["one"]["drafts"][""][0],
            "external draft"
        );
    }

    #[test]
    fn drafts_survive_restart_with_unicode_cursor_attachments_and_workspace_isolation() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("drafts.json");
        let mut state = TuiState::default();
        let mut store = DraftStore::open(path.clone(), "one".into(), &mut state).unwrap();
        state.input = "草稿 👩‍💻".into();
        state.ui.cursor = 3;
        state.image_paths = vec![PathBuf::from("图片.png")];
        store.checkpoint(&mut state);
        assert!(state.ui.drafts_persisted && state.ui.draft_error.is_none());
        let mut other = TuiState::default();
        let mut second = DraftStore::open(path.clone(), "two".into(), &mut other).unwrap();
        assert!(other.input.is_empty());
        other.input = "other workspace".into();
        second.checkpoint(&mut other);
        let mut restored = TuiState::default();
        DraftStore::open(path.clone(), "one".into(), &mut restored).unwrap();
        assert_eq!(restored.input, "草稿 👩‍💻");
        assert_eq!(restored.ui.cursor, 3);
        assert_eq!(restored.image_paths, [PathBuf::from("图片.png")]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn corruption_and_write_failure_are_visible_and_never_replace_old_data() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("drafts.json");
        fs::write(&path, b"broken").unwrap();
        let mut state = TuiState {
            input: "keep current".into(),
            ..TuiState::default()
        };
        assert!(DraftStore::open(path.clone(), "one".into(), &mut state).is_err());
        assert_eq!(state.input, "keep current");
        assert_eq!(fs::read(&path).unwrap(), b"broken");
        fs::remove_file(&path).unwrap();
        let mut store =
            DraftStore::open(path.clone(), "one".into(), &mut TuiState::default()).unwrap();
        fs::create_dir(&path).unwrap();
        store.checkpoint(&mut state);
        assert!(state
            .ui
            .draft_error
            .as_deref()
            .unwrap()
            .contains("Drafts not saved"));
        assert!(!state.ui.drafts_persisted && path.is_dir());
        state.ui.notice = Some("Ordinary status notice".into());
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(112, 36)).unwrap();
        terminal.draw(|frame| render(frame, &state)).unwrap();
        let text = format!("{:?}", terminal.backend().buffer());
        assert!(text.contains("Drafts not saved"));
        assert!(text.contains("Ordinary status notice"));
    }

    #[test]
    fn saved_draft_exit_dialog_reports_recovery_instead_of_loss() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = TuiState::default();
        let mut store =
            DraftStore::open(temp.path().join("drafts.json"), "one".into(), &mut state).unwrap();
        state.input = "ready for tomorrow".into();
        store.checkpoint(&mut state);
        open_overlay(&mut state, Overlay::Quit);
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(112, 36)).unwrap();
        terminal.draw(|frame| render(frame, &state)).unwrap();
        let text = format!("{:?}", terminal.backend().buffer());
        assert!(text.contains("Drafts are saved locally"));
        assert!(!text.contains("will be lost"));
    }

    #[test]
    fn a_second_client_cannot_silently_overwrite_another_clients_saved_draft() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("drafts.json");
        let (mut first, mut second) = (TuiState::default(), TuiState::default());
        let mut a = DraftStore::open(path.clone(), "one".into(), &mut first).unwrap();
        let mut b = DraftStore::open(path.clone(), "one".into(), &mut second).unwrap();
        first.input = "draft A".into();
        a.checkpoint(&mut first);
        second.input = "draft B".into();
        b.checkpoint(&mut second);
        assert!(!second.ui.drafts_persisted);
        assert!(second
            .ui
            .draft_error
            .as_deref()
            .unwrap()
            .contains("another client"));
        assert_eq!(second.input, "draft B");
        let mut restored = TuiState::default();
        DraftStore::open(path, "one".into(), &mut restored).unwrap();
        assert_eq!(restored.input, "draft A");
        a.checkpoint(&mut first);
        assert!(first.ui.drafts_persisted);
    }
}
