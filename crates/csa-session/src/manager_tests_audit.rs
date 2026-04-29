#[test]
fn test_save_result_persists_manager_fields_sidecar() {
    let td = tempdir().unwrap();
    let state = create_session_in(td.path(), td.path(), None, None, Some("codex")).unwrap();
    let result = crate::result::SessionResult {
        status: "success".to_string(),
        exit_code: 0,
        summary: "Test completed".to_string(),
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: chrono::Utc::now(),
        completed_at: chrono::Utc::now(),
        events_count: 0,
        artifacts: vec![],
        peak_memory_mb: None,
        manager_fields: crate::result::SessionManagerFields {
            artifacts: Some(
                toml::toml! {
                    [repo_write_audit]
                    added = ["new.txt"]
                }
                .into(),
            ),
            ..Default::default()
        },
    };

    save_result_in(td.path(), &state.meta_session_id, &result, crate::SaveOptions::default())
        .unwrap();

    let loaded = load_result_in(td.path(), &state.meta_session_id)
        .unwrap()
        .unwrap();
    assert!(
        loaded
            .artifacts
            .iter()
            .any(|artifact| artifact.path == manager_result::CONTRACT_RESULT_ARTIFACT_PATH)
    );
    assert_eq!(
        loaded
            .manager_fields
            .artifacts
            .as_ref()
            .and_then(|value| value.get("repo_write_audit"))
            .and_then(|value| value.get("added"))
            .and_then(toml::Value::as_array)
            .and_then(|items| items.first())
            .and_then(toml::Value::as_str),
        Some("new.txt")
    );
}

#[test]
fn test_compute_repo_write_audit_detects_committed_and_uncommitted_changes() {
    let td = tempdir().unwrap();
    run_git(td.path(), &["init"]);
    run_git(td.path(), &["config", "user.email", "test@example.com"]);
    run_git(td.path(), &["config", "user.name", "Test User"]);
    std::fs::write(td.path().join("tracked.txt"), "baseline\n").unwrap();
    std::fs::write(td.path().join("deleted.txt"), "delete me\n").unwrap();
    std::fs::write(td.path().join("rename-old.txt"), "rename me\n").unwrap();
    run_git(
        td.path(),
        &["add", "tracked.txt", "deleted.txt", "rename-old.txt"],
    );
    run_git(td.path(), &["commit", "-m", "init"]);
    let pre_head = detect_git_head(td.path()).unwrap();

    std::fs::write(td.path().join("tracked.txt"), "committed mutation\n").unwrap();
    std::fs::write(td.path().join("new.txt"), "new file\n").unwrap();
    std::fs::remove_file(td.path().join("deleted.txt")).unwrap();
    run_git(td.path(), &["mv", "rename-old.txt", "rename-new.txt"]);
    run_git(
        td.path(),
        &["add", "tracked.txt", "new.txt", "deleted.txt", "rename-new.txt"],
    );
    run_git(td.path(), &["commit", "-m", "repo changes"]);

    std::fs::write(td.path().join("tracked.txt"), "uncommitted mutation\n").unwrap();

    let pre_porcelain = std::process::Command::new("git")
        .args(["status", "--porcelain=v1", "-z"])
        .current_dir(td.path())
        .output()
        .unwrap();
    let pre_porcelain = String::from_utf8(pre_porcelain.stdout).unwrap();

    let audit = compute_repo_write_audit(td.path(), &pre_head, Some(&pre_porcelain)).unwrap();
    assert_eq!(audit.added, vec![std::path::PathBuf::from("new.txt")]);
    assert_eq!(audit.modified, vec![std::path::PathBuf::from("tracked.txt")]);
    assert_eq!(audit.deleted, vec![std::path::PathBuf::from("deleted.txt")]);
    assert_eq!(
        audit.renamed,
        vec![(
            std::path::PathBuf::from("rename-old.txt"),
            std::path::PathBuf::from("rename-new.txt")
        )]
    );
}

#[test]
fn test_compute_repo_write_audit_clean_session_is_empty() {
    let td = tempdir().unwrap();
    run_git(td.path(), &["init"]);
    run_git(td.path(), &["config", "user.email", "test@example.com"]);
    run_git(td.path(), &["config", "user.name", "Test User"]);
    std::fs::write(td.path().join("tracked.txt"), "baseline\n").unwrap();
    run_git(td.path(), &["add", "tracked.txt"]);
    run_git(td.path(), &["commit", "-m", "init"]);

    let pre_head = detect_git_head(td.path()).unwrap();
    let pre_porcelain = std::process::Command::new("git")
        .args(["status", "--porcelain=v1", "-z"])
        .current_dir(td.path())
        .output()
        .unwrap();
    let pre_porcelain = String::from_utf8(pre_porcelain.stdout).unwrap();

    let audit = compute_repo_write_audit(td.path(), &pre_head, Some(&pre_porcelain)).unwrap();
    assert!(audit.is_empty());
}

#[test]
fn test_pre_existing_dirty_file_not_attributed_to_session() {
    let td = tempdir().unwrap();
    run_git(td.path(), &["init"]);
    run_git(td.path(), &["config", "user.email", "test@example.com"]);
    run_git(td.path(), &["config", "user.name", "Test User"]);
    std::fs::write(td.path().join("src.txt"), "baseline\n").unwrap();
    run_git(td.path(), &["add", "src.txt"]);
    run_git(td.path(), &["commit", "-m", "init"]);

    let pre_head = detect_git_head(td.path()).unwrap();
    std::fs::write(td.path().join("src.txt"), "dirty before session\n").unwrap();
    let pre_porcelain = std::process::Command::new("git")
        .args(["status", "--porcelain=v1", "-z"])
        .current_dir(td.path())
        .output()
        .unwrap();
    let pre_porcelain = String::from_utf8(pre_porcelain.stdout).unwrap();

    let audit = compute_repo_write_audit(td.path(), &pre_head, Some(&pre_porcelain)).unwrap();
    assert!(audit.is_empty());
}

#[test]
fn test_pre_existing_dirty_plus_session_add() {
    let td = tempdir().unwrap();
    run_git(td.path(), &["init"]);
    run_git(td.path(), &["config", "user.email", "test@example.com"]);
    run_git(td.path(), &["config", "user.name", "Test User"]);
    std::fs::write(td.path().join("src.txt"), "baseline\n").unwrap();
    run_git(td.path(), &["add", "src.txt"]);
    run_git(td.path(), &["commit", "-m", "init"]);

    let pre_head = detect_git_head(td.path()).unwrap();
    std::fs::write(td.path().join("src.txt"), "dirty before session\n").unwrap();
    let pre_porcelain = std::process::Command::new("git")
        .args(["status", "--porcelain=v1", "-z"])
        .current_dir(td.path())
        .output()
        .unwrap();
    let pre_porcelain = String::from_utf8(pre_porcelain.stdout).unwrap();

    std::fs::write(td.path().join("output-new.md"), "created during session\n").unwrap();
    run_git(td.path(), &["add", "output-new.md"]);
    let audit = compute_repo_write_audit(td.path(), &pre_head, Some(&pre_porcelain)).unwrap();
    assert_eq!(audit.added, vec![std::path::PathBuf::from("output-new.md")]);
    assert!(audit.modified.is_empty());
    assert!(audit.deleted.is_empty());
    assert!(audit.renamed.is_empty());
}

#[test]
fn test_session_further_modifies_pre_existing_dirty_file_is_conservatively_ignored() {
    let td = tempdir().unwrap();
    run_git(td.path(), &["init"]);
    run_git(td.path(), &["config", "user.email", "test@example.com"]);
    run_git(td.path(), &["config", "user.name", "Test User"]);
    std::fs::write(td.path().join("src.txt"), "baseline\n").unwrap();
    run_git(td.path(), &["add", "src.txt"]);
    run_git(td.path(), &["commit", "-m", "init"]);

    let pre_head = detect_git_head(td.path()).unwrap();
    std::fs::write(td.path().join("src.txt"), "dirty before session\n").unwrap();
    let pre_porcelain = std::process::Command::new("git")
        .args(["status", "--porcelain=v1", "-z"])
        .current_dir(td.path())
        .output()
        .unwrap();
    let pre_porcelain = String::from_utf8(pre_porcelain.stdout).unwrap();

    std::fs::write(td.path().join("src.txt"), "dirty and changed again\n").unwrap();
    let audit = compute_repo_write_audit(td.path(), &pre_head, Some(&pre_porcelain)).unwrap();
    assert!(audit.is_empty());
}

#[test]
fn test_write_audit_warning_artifact_persists_session_local_warning() {
    let td = tempdir().unwrap();
    let session_dir = td.path().join("session");
    std::fs::create_dir_all(&session_dir).unwrap();
    let artifact = write_audit_warning_artifact(
        &session_dir,
        &RepoWriteAudit {
            added: vec![std::path::PathBuf::from("output/summary.md")],
            modified: vec![std::path::PathBuf::from("src/lib.rs")],
            deleted: vec![std::path::PathBuf::from("src/old.rs")],
            renamed: vec![(
                std::path::PathBuf::from("src/a.rs"),
                std::path::PathBuf::from("src/b.rs")
            )],
        },
    )
    .unwrap()
    .expect("artifact path");

    assert_eq!(artifact, session_dir.join("output/audit-warnings.md"));
    let contents = std::fs::read_to_string(&artifact).unwrap();
    assert!(contents.contains("output/summary.md"));
    assert!(contents.contains("src/lib.rs"));
    assert!(contents.contains("src/old.rs"));
    assert!(contents.contains("src/a.rs"));
    assert!(contents.contains("src/b.rs"));
}
