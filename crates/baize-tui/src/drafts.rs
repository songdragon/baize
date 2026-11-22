use super::*;
use std::fs::{self, OpenOptions};

type Draft = (String, usize, Vec<PathBuf>);
type DraftMap = HashMap<String, Draft>;

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

pub(super) fn current_draft(state: &TuiState) -> Option<Draft> {
    if !state.input.is_empty() || !state.image_paths.is_empty() {
        Some((
            state.input.clone(),
            state.ui.cursor,
            state.image_paths.clone(),
        ))
    } else if state.ui.execution_uncertain || state.activity_status.starts_with("running") {
        // Keep an uncertain submission recoverable, but never automatically send it.
        state
            .ui
            .last_submission
            .as_ref()
            .map(|last| (last.prompt.clone(), usize::MAX, last.image_paths.clone()))
    } else {
        None
    }
}

fn snapshot(state: &TuiState) -> Result<Value> {
    let mut drafts = state.ui.drafts.clone();
    let key = state.session_id.clone().unwrap_or_default();
    if let Some(draft) = current_draft(state) {
        drafts.insert(key, draft);
    } else {
        drafts.remove(&key);
    }
    drafts.retain(|_, (text, _, images)| !text.is_empty() || !images.is_empty());
    serde_json::to_value(drafts).context("serialize client drafts")
}

fn read_document(path: &Path) -> Result<Value> {
    let data = match fs::read(path) {
        Ok(bytes) => serde_json::from_slice::<Value>(&bytes).context("parse draft file")?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            json!({"version":1,"workspaces":{}})
        }
        Err(error) => return Err(error).context("read draft file"),
    };
    anyhow::ensure!(
        data["version"] == 1 && data["workspaces"].is_object(),
        "unsupported or malformed draft file"
    );
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
            .unwrap_or_else(|| json!({}));
        let drafts: DraftMap = serde_json::from_value(value).context("decode workspace drafts")?;
        let count = drafts.len();
        state.ui.drafts = drafts;
        restore_draft(state);
        state.ui.drafts_persisted = true;
        if count > 0 {
            state.ui.notice = Some("Local drafts recovered. Review recorded history before sending; Ctrl-L opens the latest session.".into());
        }
        Ok(Self {
            path,
            workspace,
            previous: snapshot(state)?,
        })
    }

    pub(super) fn checkpoint(&mut self, state: &mut TuiState) {
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
        let current = snapshot(state)?;
        if current == self.previous {
            return Ok(());
        }
        // Reload before replacing only this workspace so other workspaces survive.
        let mut document = read_document(&self.path)?;
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
    fn session_drafts_and_uncertain_submissions_are_retained_without_automatic_retry() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("drafts.json");
        let mut state = TuiState::default();
        let mut store = DraftStore::open(path.clone(), "one".into(), &mut state).unwrap();
        state.session_id = Some("task_1".into());
        state.input = "draft".into();
        save_draft(&mut state);
        state.session_id = Some("task_2".into());
        state.input.clear();
        state.ui.execution_uncertain = true;
        state.ui.last_submission = Some(PendingPromptSubmission {
            prompt: "possibly submitted".into(),
            provider_id: "codex".into(),
            image_paths: vec![],
            workspace_id: None,
            session_id: state.session_id.clone(),
        });
        store.checkpoint(&mut state);
        let mut restored = TuiState::default();
        let mut reloaded = DraftStore::open(path.clone(), "one".into(), &mut restored).unwrap();
        assert!(restored.input.is_empty());
        restored.session_id = Some("task_2".into());
        restore_draft(&mut restored);
        assert_eq!(restored.input, "possibly submitted");
        restored.input.clear();
        restored.activity_status = "idle".into();
        restored.ui.execution_uncertain = false;
        reloaded.checkpoint(&mut restored);
        let mut final_state = TuiState::default();
        DraftStore::open(path, "one".into(), &mut final_state).unwrap();
        assert!(!final_state.ui.drafts.contains_key("task_2"));
        assert_eq!(final_state.ui.drafts["task_1"].0, "draft");
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
        assert!(!text.contains("Ordinary status notice"));
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
}
