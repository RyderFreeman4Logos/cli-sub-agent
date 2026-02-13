// -- ensure_git_init tests ------------------------------------------------

#[test]
fn test_ensure_git_init_creates_repo() {
    let dir = tempdir().unwrap();
    let todos = dir.path().join("fresh-todos");

    assert!(!todos.exists());

    ensure_git_init(&todos).unwrap();

    assert!(todos.join(".git").exists(), ".git must exist after init");
    assert!(
        todos.join(".gitignore").exists(),
        ".gitignore must be created"
    );
    let gitignore = fs::read_to_string(todos.join(".gitignore")).unwrap();
    assert!(
        gitignore.contains(".lock"),
        ".gitignore must contain .lock rule"
    );
}

#[test]
fn test_ensure_git_init_idempotent() {
    let dir = tempdir().unwrap();
    let todos = dir.path();

    ensure_git_init(todos).unwrap();
    // Calling again should be a no-op, not an error
    ensure_git_init(todos).unwrap();

    assert!(todos.join(".git").exists());
}

#[test]
fn test_ensure_git_init_sets_user_config() {
    let dir = tempdir().unwrap();
    let todos = dir.path().join("cfg-test");

    ensure_git_init(&todos).unwrap();

    let email_output = Command::new("git")
        .args(["config", "user.email"])
        .current_dir(&todos)
        .output()
        .unwrap();
    let email = String::from_utf8_lossy(&email_output.stdout)
        .trim()
        .to_string();
    assert_eq!(email, "csa@localhost");
}

// -- history tests --------------------------------------------------------

#[test]
fn test_history_shows_plan_commits() {
    let dir = tempdir().unwrap();
    let todos = dir.path();
    let ts = "20260101T000000";

    setup_todos_dir(todos, ts);
    save(todos, ts, "first save").unwrap();

    fs::write(todos.join(ts).join("TODO.md"), "# Version 2\n").unwrap();
    save(todos, ts, "second save").unwrap();

    let log = history(todos, ts).unwrap();
    assert!(
        log.contains("second save"),
        "history should contain second commit message, got: {log}"
    );
    assert!(
        log.contains("first save"),
        "history should contain first commit message, got: {log}"
    );
}

#[test]
fn test_history_no_git_repo_errors() {
    let dir = tempdir().unwrap();
    let todos = dir.path();
    let result = history(todos, "20260101T000000");
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("No git repository"));
}

#[test]
fn test_history_invalid_timestamp_errors() {
    let dir = tempdir().unwrap();
    let todos = dir.path();
    ensure_git_init(todos).unwrap();

    let result = history(todos, "../etc");
    assert!(result.is_err());
}

// -- list_versions tests --------------------------------------------------

#[test]
fn test_list_versions_tracks_todo_md() {
    let dir = tempdir().unwrap();
    let todos = dir.path();
    let ts = "20260101T000000";

    setup_todos_dir(todos, ts);
    save(todos, ts, "v1").unwrap();

    fs::write(todos.join(ts).join("TODO.md"), "# v2\n").unwrap();
    save(todos, ts, "v2").unwrap();

    let versions = list_versions(todos, ts).unwrap();
    assert_eq!(
        versions.len(),
        2,
        "should have 2 committed versions of TODO.md"
    );
    assert!(versions[0].len() >= 7, "should be full hash");
}

#[test]
fn test_list_versions_empty_when_no_git() {
    let dir = tempdir().unwrap();
    let todos = dir.path();
    let versions = list_versions(todos, "20260101T000000").unwrap();
    assert!(versions.is_empty());
}

#[test]
fn test_list_versions_metadata_only_commit_excluded() {
    let dir = tempdir().unwrap();
    let todos = dir.path();
    let ts = "20260101T000000";

    setup_todos_dir(todos, ts);
    save(todos, ts, "initial").unwrap();

    fs::write(
        todos.join(ts).join("metadata.toml"),
        "title = \"changed\"\nstatus = \"approved\"\n",
    )
    .unwrap();
    let meta_path = format!("{}/metadata.toml", ts);
    save_file(todos, ts, &meta_path, "metadata update").unwrap();

    let versions = list_versions(todos, ts).unwrap();
    assert_eq!(
        versions.len(),
        1,
        "metadata-only commit should not appear in TODO.md versions"
    );
}

// -- show_version tests ---------------------------------------------------

#[test]
fn test_show_version_retrieves_content() {
    let dir = tempdir().unwrap();
    let todos = dir.path();
    let ts = "20260101T000000";

    setup_todos_dir(todos, ts);
    save(todos, ts, "v1").unwrap();

    fs::write(todos.join(ts).join("TODO.md"), "# Version Two\n").unwrap();
    save(todos, ts, "v2").unwrap();

    let v1_content = show_version(todos, ts, 1).unwrap();
    assert!(
        v1_content.contains("Version Two"),
        "version 1 should be latest, got: {v1_content}"
    );

    let v2_content = show_version(todos, ts, 2).unwrap();
    assert!(
        v2_content.contains("# Test"),
        "version 2 should be original, got: {v2_content}"
    );
}

#[test]
fn test_show_version_zero_errors() {
    let dir = tempdir().unwrap();
    let todos = dir.path();
    let ts = "20260101T000000";

    setup_todos_dir(todos, ts);
    save(todos, ts, "v1").unwrap();

    let result = show_version(todos, ts, 0);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Version 0"));
}

#[test]
fn test_show_version_out_of_range_errors() {
    let dir = tempdir().unwrap();
    let todos = dir.path();
    let ts = "20260101T000000";

    setup_todos_dir(todos, ts);
    save(todos, ts, "v1").unwrap();

    let result = show_version(todos, ts, 99);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("does not exist"));
}

#[test]
fn test_show_version_no_commits_errors() {
    let dir = tempdir().unwrap();
    let todos = dir.path();
    let ts = "20260101T000000";

    setup_todos_dir(todos, ts);

    let result = show_version(todos, ts, 1);
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("No committed versions"));
}

// -- diff tests -----------------------------------------------------------

#[test]
fn test_diff_against_last_commit() {
    let dir = tempdir().unwrap();
    let todos = dir.path();
    let ts = "20260101T000000";

    setup_todos_dir(todos, ts);
    save(todos, ts, "initial").unwrap();

    fs::write(todos.join(ts).join("TODO.md"), "# Changed content\n").unwrap();

    let diff_output = diff(todos, ts, None).unwrap();
    assert!(
        diff_output.contains("Changed content"),
        "diff should show working copy change, got: {diff_output}"
    );
}

#[test]
fn test_diff_clean_working_copy_defaults_to_last_two_versions() {
    let dir = tempdir().unwrap();
    let todos = dir.path();
    let ts = "20260101T000000";

    setup_todos_dir(todos, ts);
    fs::write(todos.join(ts).join("TODO.md"), "# Version 1\n").unwrap();
    save(todos, ts, "v1").unwrap();

    fs::write(todos.join(ts).join("TODO.md"), "# Version 2\n").unwrap();
    save(todos, ts, "v2").unwrap();

    let diff_output = diff(todos, ts, None).unwrap();
    assert!(
        diff_output.contains("-# Version 1"),
        "clean default diff should include previous version, got: {diff_output}"
    );
    assert!(
        diff_output.contains("+# Version 2"),
        "clean default diff should include latest version, got: {diff_output}"
    );
}

#[test]
fn test_diff_clean_working_copy_with_single_version_shows_initial_content() {
    let dir = tempdir().unwrap();
    let todos = dir.path();
    let ts = "20260101T000000";

    setup_todos_dir(todos, ts);
    fs::write(todos.join(ts).join("TODO.md"), "# Initial save\n").unwrap();
    save(todos, ts, "v1").unwrap();

    let diff_output = diff(todos, ts, None).unwrap();
    assert!(
        diff_output.contains("--- /dev/null"),
        "single-version clean diff should render as new file, got: {diff_output}"
    );
    assert!(
        diff_output.contains("+# Initial save"),
        "single-version clean diff should include initial content, got: {diff_output}"
    );
}

#[test]
fn test_diff_uncommitted_file_shows_full_content() {
    let dir = tempdir().unwrap();
    let todos = dir.path();
    let ts = "20260101T000000";

    let plan_dir = todos.join(ts);
    fs::create_dir_all(&plan_dir).unwrap();
    fs::write(plan_dir.join("TODO.md"), "# Brand New\nLine 2\n").unwrap();
    ensure_git_init(todos).unwrap();

    let diff_output = diff(todos, ts, None).unwrap();
    assert!(
        diff_output.contains("+# Brand New"),
        "should show full file as new, got: {diff_output}"
    );
    assert!(
        diff_output.contains("+Line 2"),
        "should contain all lines, got: {diff_output}"
    );
}

#[test]
fn test_diff_nonexistent_file_returns_empty() {
    let dir = tempdir().unwrap();
    let todos = dir.path();
    let ts = "20260101T000000";

    ensure_git_init(todos).unwrap();

    let diff_output = diff(todos, ts, None).unwrap();
    assert!(
        diff_output.is_empty(),
        "diff of nonexistent file should be empty, got: {diff_output}"
    );
}

#[test]
fn test_diff_no_git_repo_errors() {
    let dir = tempdir().unwrap();
    let todos = dir.path();

    let result = diff(todos, "20260101T000000", None);
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("No git repository"));
}

// -- diff_versions tests --------------------------------------------------

#[test]
fn test_diff_versions_between_commits() {
    let dir = tempdir().unwrap();
    let todos = dir.path();
    let ts = "20260101T000000";

    setup_todos_dir(todos, ts);
    save(todos, ts, "v1").unwrap();

    fs::write(todos.join(ts).join("TODO.md"), "# Version 2\n").unwrap();
    save(todos, ts, "v2").unwrap();

    let diff_output = diff_versions(todos, ts, 2, 1).unwrap();
    assert!(
        diff_output.contains("Version 2"),
        "diff should reference v2 changes, got: {diff_output}"
    );
}

#[test]
fn test_diff_versions_same_version_empty() {
    let dir = tempdir().unwrap();
    let todos = dir.path();
    let ts = "20260101T000000";

    setup_todos_dir(todos, ts);
    save(todos, ts, "v1").unwrap();

    let diff_output = diff_versions(todos, ts, 1, 1).unwrap();
    assert!(
        diff_output.is_empty(),
        "diff of same version should be empty, got: {diff_output}"
    );
}

#[test]
fn test_diff_versions_out_of_range_errors() {
    let dir = tempdir().unwrap();
    let todos = dir.path();
    let ts = "20260101T000000";

    setup_todos_dir(todos, ts);
    save(todos, ts, "v1").unwrap();

    let result = diff_versions(todos, ts, 1, 99);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("does not exist"));
}

#[test]
fn test_diff_versions_zero_errors() {
    let dir = tempdir().unwrap();
    let todos = dir.path();
    let ts = "20260101T000000";

    setup_todos_dir(todos, ts);
    save(todos, ts, "v1").unwrap();

    let result = diff_versions(todos, ts, 0, 1);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("does not exist"));
}

#[test]
fn test_diff_versions_no_commits_errors() {
    let dir = tempdir().unwrap();
    let todos = dir.path();
    let ts = "20260101T000000";

    setup_todos_dir(todos, ts);

    let result = diff_versions(todos, ts, 1, 2);
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("No committed versions"));
}

// -- validate_revision tests ----------------------------------------------

#[test]
fn test_validate_revision_rejects_option_injection() {
    let result = validate_revision("--exec=evil");
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("must not start"));
}

#[test]
fn test_validate_revision_accepts_valid_hash() {
    assert!(validate_revision("abc123").is_ok());
    assert!(validate_revision("HEAD").is_ok());
    assert!(validate_revision("HEAD~1").is_ok());
}

// -- save with invalid timestamp ------------------------------------------

#[test]
fn test_save_rejects_path_traversal() {
    let dir = tempdir().unwrap();
    let todos = dir.path();

    let result = save(todos, "../evil", "bad");
    assert!(result.is_err());
}
