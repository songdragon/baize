use super::*;
use serde_json::json;

fn fixture() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join(".agents")).unwrap();
    fs::write(root.path().join(SOURCE),json!({"mcpServers":{"fixture":{"command":"python3","args":["mcp.py"],"env":{"TOKEN":"private-test-value"},"disabledTools":["erase"]}}}).to_string()).unwrap();
    root
}

#[test]
fn previews_shared_sources_without_writes_or_secret_values() {
    let root = fixture();
    fs::write(root.path().join("AGENTS.md"), "Shared project instruction").unwrap();
    fs::create_dir_all(root.path().join(".agents/skills/fixture")).unwrap();
    fs::write(
        root.path().join(".agents/skills/fixture/SKILL.md"),
        "---\nname: fixture\ndescription: fixture\n---\nshared source",
    )
    .unwrap();
    let preview = plan(root.path(), &[]).unwrap();
    assert_eq!(preview.status, "ready");
    assert_eq!(preview.shared_instructions, ["AGENTS.md"]);
    assert_eq!(preview.shared_skills, [".agents/skills/fixture/SKILL.md"]);
    assert!(!serde_json::to_string(&preview)
        .unwrap()
        .contains("private-test-value"));
    assert!(!root.path().join(".codex").exists());
    assert!(!root.path().join(".baize").exists());
    assert_eq!(
        preview,
        serde_json::from_str(&serde_json::to_string(&preview).unwrap()).unwrap()
    );
}

#[test]
fn applies_and_restores_exact_native_bytes_preserving_unrelated_settings() {
    let root = fixture();
    let backups = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join(".codex")).unwrap();
    let original =
        "# Keep my comment\nmodel = 'native-model'\n[mcp_servers.other]\ncommand = 'other'\n";
    fs::write(root.path().join(TARGET), original).unwrap();
    let preview = plan(root.path(), &[]).unwrap();
    assert_eq!(preview.native_only_servers, ["other"]);
    let changed = apply(root.path(), &preview.plan_id, &[], backups.path()).unwrap();
    let receipts = history(root.path(), backups.path()).unwrap();
    assert_eq!(receipts[0].backup_id, changed.backup_id);
    assert_eq!(receipts[0].status, "applied");
    assert!(!serde_json::to_string(&receipts)
        .unwrap()
        .contains("private-test-value"));
    let current = fs::read_to_string(root.path().join(TARGET)).unwrap();
    assert!(current.contains("# Keep my comment\nmodel = 'native-model'"));
    let parsed = current.parse::<toml::Value>().unwrap();
    assert_eq!(
        parsed["mcp_servers"]["other"]["command"].as_str(),
        Some("other")
    );
    assert_eq!(
        parsed["mcp_servers"]["fixture"]["env"]["TOKEN"].as_str(),
        Some("private-test-value")
    );
    assert_eq!(plan(root.path(), &[]).unwrap().status, "in_sync");
    assert_eq!(
        changed,
        serde_json::from_str(&serde_json::to_string(&changed).unwrap()).unwrap()
    );
    assert_eq!(
        restore(root.path(), &changed.backup_id, backups.path())
            .unwrap()
            .status,
        "restored"
    );
    assert_eq!(
        fs::read_to_string(root.path().join(TARGET)).unwrap(),
        original
    );
    assert_eq!(
        restore(root.path(), &changed.backup_id, backups.path())
            .unwrap()
            .status,
        "already_restored"
    );
    assert_eq!(
        history(root.path(), backups.path()).unwrap()[0].status,
        "restored"
    );
}

#[test]
fn history_is_empty_before_apply_and_rejects_corrupt_receipts() {
    let root = fixture();
    let backups = tempfile::tempdir().unwrap();
    assert!(history(root.path(), backups.path()).unwrap().is_empty());
    let preview = plan(root.path(), &[]).unwrap();
    let change = apply(root.path(), &preview.plan_id, &[], backups.path()).unwrap();
    let directory = backups.path().join(digest(
        root.path()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .as_bytes(),
    ));
    fs::write(
        directory.join(format!("{}.json", change.backup_id)),
        "not-json",
    )
    .unwrap();
    assert!(history(root.path(), backups.path()).is_err());
}

#[test]
fn restoring_a_new_file_removes_only_that_file_and_preserves_later_edits() {
    let root = fixture();
    let backups = tempfile::tempdir().unwrap();
    let preview = plan(root.path(), &[]).unwrap();
    let changed = apply(root.path(), &preview.plan_id, &[], backups.path()).unwrap();
    fs::write(root.path().join(".codex/keep.txt"), "keep").unwrap();
    fs::write(root.path().join(TARGET), "# a later user edit\n").unwrap();
    assert!(restore(root.path(), &changed.backup_id, backups.path())
        .unwrap_err()
        .to_string()
        .contains("later edits"));
    assert_eq!(
        fs::read_to_string(root.path().join(TARGET)).unwrap(),
        "# a later user edit\n"
    );
    let desired = prepare(root.path(), &[]).unwrap().desired;
    fs::write(root.path().join(TARGET), desired).unwrap();
    // A different valid file is still not the original applied file.
    assert!(restore(root.path(), &changed.backup_id, backups.path()).is_err());
}

#[test]
fn fresh_target_can_be_removed_by_checked_restore() {
    let root = fixture();
    let backups = tempfile::tempdir().unwrap();
    let preview = plan(root.path(), &[]).unwrap();
    let change = apply(root.path(), &preview.plan_id, &[], backups.path()).unwrap();
    restore(root.path(), &change.backup_id, backups.path()).unwrap();
    assert!(!root.path().join(TARGET).exists());
    assert!(root.path().join(SOURCE).exists());
    assert!(restore(root.path(), "../../backup", backups.path()).is_err());
    assert!(restore(
        root.path(),
        &uuid::Uuid::new_v4().to_string(),
        backups.path()
    )
    .is_err());
}

#[test]
fn changed_source_target_or_replacement_choices_invalidate_the_plan() {
    let root = fixture();
    let backups = tempfile::tempdir().unwrap();
    let old = plan(root.path(), &[]).unwrap();
    fs::write(root.path().join(SOURCE), "{\"mcpServers\":{}}").unwrap();
    assert!(apply(root.path(), &old.plan_id, &[], backups.path())
        .unwrap_err()
        .to_string()
        .contains("fresh plan"));
    assert!(!root.path().join(TARGET).exists());
    assert!(plan(root.path(), &["nonexistent".into()]).is_err());
    let preview = plan(root.path(), &[]).unwrap();
    assert!(apply(root.path(), &preview.plan_id, &[], backups.path()).is_err());
}

#[test]
fn conflicting_names_require_explicit_replacement_and_keep_other_servers() {
    let root = fixture();
    let backups = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join(".codex")).unwrap();
    fs::write(
        root.path().join(TARGET),
        "[mcp_servers.fixture]\ncommand='native'\n",
    )
    .unwrap();
    let preview = plan(root.path(), &[]).unwrap();
    assert_eq!(preview.status, "blocked");
    assert!(apply(root.path(), &preview.plan_id, &[], backups.path()).is_err());
    let replace = vec!["fixture".into()];
    let preview = plan(root.path(), &replace).unwrap();
    assert_eq!(preview.status, "ready");
    assert!(apply(root.path(), &preview.plan_id, &[], backups.path()).is_err());
    apply(root.path(), &preview.plan_id, &replace, backups.path()).unwrap();
    assert_eq!(plan(root.path(), &[]).unwrap().status, "in_sync");
}

#[test]
fn replacement_preserves_native_tool_approval_rules() {
    let root = fixture();
    let backups = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join(".codex")).unwrap();
    fs::write(root.path().join(TARGET),"[mcp_servers.fixture]\ncommand='native'\ndefault_tools_approval_mode='prompt'\ndisabled_tools=['erase']\n[mcp_servers.fixture.tools.erase]\napproval_mode='prompt'\n").unwrap();
    let replace = vec!["fixture".into()];
    let preview = plan(root.path(), &replace).unwrap();
    apply(root.path(), &preview.plan_id, &replace, backups.path()).unwrap();
    let native = fs::read_to_string(root.path().join(TARGET))
        .unwrap()
        .parse::<toml::Value>()
        .unwrap();
    assert_eq!(
        native["mcp_servers"]["fixture"]
            .get("default_tools_approval_mode")
            .and_then(toml::Value::as_str),
        Some("prompt")
    );
    assert_eq!(
        native["mcp_servers"]["fixture"]["tools"]["erase"]["approval_mode"].as_str(),
        Some("prompt")
    );
}

#[test]
fn replacement_cannot_relax_native_denylists_or_reuse_unknown_auth_settings() {
    for fields in [
        "disabled_tools=['erase','wipe']",
        "env_vars=['PRIVATE_NATIVE']",
    ] {
        let root = fixture();
        fs::create_dir(root.path().join(".codex")).unwrap();
        fs::write(
            root.path().join(TARGET),
            format!("[mcp_servers.fixture]\ncommand='native'\n{fields}\n"),
        )
        .unwrap();
        assert_eq!(
            plan(root.path(), &["fixture".into()]).unwrap().status,
            "blocked"
        );
    }
}

#[test]
fn matching_connections_keep_native_authentication_and_default_fields_untouched() {
    for (source,native) in [
        (json!({"mcpServers":{"fixture":{"url":"https://example.invalid/mcp"}}}),"[mcp_servers.fixture]\nurl='https://example.invalid/mcp'\nbearer_token_env_var='NATIVE_ONLY_TOKEN'\n"),
        (json!({"mcpServers":{"fixture":{"command":"python3","args":["mcp.py"]}}}),"[mcp_servers.fixture]\ncommand='python3'\nargs=['mcp.py']\nenv_vars=['NATIVE_ONLY_TOKEN']\n")
    ] {
        let root=fixture();fs::create_dir(root.path().join(".codex")).unwrap();
        fs::write(root.path().join(SOURCE),source.to_string()).unwrap();fs::write(root.path().join(TARGET),native).unwrap();
        assert_eq!(plan(root.path(),&[]).unwrap().status,"in_sync");
        assert_eq!(fs::read_to_string(root.path().join(TARGET)).unwrap(),native);
    }
}

#[test]
fn maps_http_and_tool_filters_without_changing_approval_policy() {
    let mapped=mapping::codex_server(&json!({"serverUrl":"https://example.invalid/mcp","headers":{"Authorization":"private"},"disabled":true,"enabledTools":["read"],"disabledTools":["write"],"timeoutSeconds":30}),Path::new("/project")).unwrap();
    assert_eq!(mapped["enabled"].as_bool(), Some(false));
    assert_eq!(mapped["tool_timeout_sec"].as_integer(), Some(30));
    assert_eq!(mapped["startup_timeout_sec"].as_integer(), Some(30));
    assert_eq!(mapped["enabled_tools"][0].as_str(), Some("read"));
    assert!(mapped.get("approval_policy").is_none());
    for config in [
        json!({"command":"x","oauth":{}}),
        json!({"command":"x","serverUrl":"https://example.invalid"}),
        json!({"url":"https://example.invalid","env":{}}),
        json!({"command":"x","timeoutSeconds":-1}),
        json!({"command":"x","enabledTools":[]}),
        json!({"command":"x","disabled":"false"}),
        json!({"command":"x","args":[2]}),
        json!({"url":"https://token@example.invalid"}),
    ] {
        assert!(
            mapping::codex_server(&config, Path::new("/project")).is_err(),
            "{config}"
        );
    }
}

#[test]
fn unsupported_fields_block_the_whole_plan_without_partial_writes() {
    let root = fixture();
    let backups = tempfile::tempdir().unwrap();
    fs::write(root.path().join(SOURCE),json!({"mcpServers":{"ok":{"command":"x"},"unsupported":{"command":"x","tools":{"unsafe":{"background":"always"}}}}}).to_string()).unwrap();
    let preview = plan(root.path(), &[]).unwrap();
    assert_eq!(preview.status, "blocked");
    assert!(apply(root.path(), &preview.plan_id, &[], backups.path()).is_err());
    assert!(!root.path().join(TARGET).exists());
}

#[test]
fn invalid_inputs_and_failed_backup_do_not_touch_native_configuration() {
    let root = fixture();
    let backups = tempfile::NamedTempFile::new().unwrap();
    let preview = plan(root.path(), &[]).unwrap();
    assert!(apply(root.path(), &preview.plan_id, &[], backups.path()).is_err());
    assert!(!root.path().join(TARGET).exists());
    for source in [
        "{",
        "[]",
        "{}",
        "{\"mcpServers\":null}",
        "{\"mcpServers\":{\"bad/name\":{\"command\":\"x\"}}}",
        "{\"mcpServers\":{\"same\":{\"command\":\"x\"},\"same\":{\"command\":\"y\"}}}",
        "{\"mcpServers\":{},\"mcpServers\":{}}",
    ] {
        fs::write(root.path().join(SOURCE), source).unwrap();
        assert!(plan(root.path(), &[]).is_err());
    }
    fs::write(root.path().join(SOURCE), "x".repeat(MAX_BYTES as usize + 1)).unwrap();
    assert!(plan(root.path(), &[]).is_err());
}

#[cfg(unix)]
#[test]
fn symlinked_skills_are_reused_but_output_symlinks_are_rejected() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let root = fixture();
    let external = tempfile::tempdir().unwrap();
    fs::write(
        external.path().join("SKILL.md"),
        "---\nname: linked\ndescription: linked\n---\n",
    )
    .unwrap();
    fs::create_dir_all(root.path().join(".agents/skills")).unwrap();
    symlink(external.path(), root.path().join(".agents/skills/linked")).unwrap();
    assert_eq!(plan(root.path(), &[]).unwrap().shared_skills.len(), 1);
    symlink(external.path(), root.path().join(".codex")).unwrap();
    assert!(plan(root.path(), &[]).is_err());
    fs::remove_file(root.path().join(".codex")).unwrap();
    let backups = tempfile::tempdir().unwrap();
    let preview = plan(root.path(), &[]).unwrap();
    let change = apply(root.path(), &preview.plan_id, &[], backups.path()).unwrap();
    assert_eq!(
        fs::metadata(root.path().join(TARGET))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    fs::set_permissions(root.path().join(TARGET), fs::Permissions::from_mode(0o644)).unwrap();
    assert!(restore(root.path(), &change.backup_id, backups.path()).is_err());
}
