// -- find_by_branch additional tests --------------------------------------

#[test]
fn test_find_by_branch_excludes_no_branch_plans() {
    let dir = tempdir().unwrap();
    let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

    manager.create("With branch", Some("feat/x")).unwrap();
    manager.create("No branch", None).unwrap();

    let found = manager.find_by_branch("feat/x").unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].metadata.title, "With branch");
}

#[test]
fn test_find_by_branch_empty_todos_dir() {
    let dir = tempdir().unwrap();
    let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

    let found = manager.find_by_branch("any-branch").unwrap();
    assert!(found.is_empty());
}

// -- find_by_status additional tests --------------------------------------

#[test]
fn test_find_by_status_multiple_matches() {
    let dir = tempdir().unwrap();
    let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

    let p1 = manager.create("Plan 1", None).unwrap();
    let p2 = manager.create("Plan 2", None).unwrap();
    manager.create("Plan 3", None).unwrap();

    manager
        .update_status(&p1.timestamp, TodoStatus::Implementing)
        .unwrap();
    manager
        .update_status(&p2.timestamp, TodoStatus::Implementing)
        .unwrap();

    let implementing = manager.find_by_status(TodoStatus::Implementing).unwrap();
    assert_eq!(implementing.len(), 2);

    let drafts = manager.find_by_status(TodoStatus::Draft).unwrap();
    assert_eq!(drafts.len(), 1);
    assert_eq!(drafts[0].metadata.title, "Plan 3");
}

#[test]
fn test_find_by_status_no_match() {
    let dir = tempdir().unwrap();
    let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

    manager.create("Draft plan", None).unwrap();

    let done = manager.find_by_status(TodoStatus::Done).unwrap();
    assert!(done.is_empty());
}

// -- multi-plan listing tests ---------------------------------------------

#[test]
fn test_list_sorted_newest_first() {
    let dir = tempdir().unwrap();
    let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

    for (ts, title) in [
        ("20250101T000000", "Oldest"),
        ("20250601T000000", "Middle"),
        ("20260101T000000", "Newest"),
    ] {
        let plan_dir = dir.path().join(ts);
        std::fs::create_dir_all(&plan_dir).unwrap();
        let now = chrono::Utc::now();
        let metadata = TodoMetadata {
            branch: None,
            status: TodoStatus::Draft,
            title: title.to_string(),
            sessions: Vec::new(),
            created_at: now,
            updated_at: now,
        };
        let content = toml::to_string_pretty(&metadata).unwrap();
        std::fs::write(plan_dir.join("metadata.toml"), content).unwrap();
        std::fs::write(plan_dir.join("TODO.md"), format!("# {title}\n")).unwrap();
    }

    let plans = manager.list().unwrap();
    assert_eq!(plans.len(), 3);
    assert_eq!(plans[0].metadata.title, "Newest");
    assert_eq!(plans[1].metadata.title, "Middle");
    assert_eq!(plans[2].metadata.title, "Oldest");
}

#[test]
fn test_list_skips_dirs_without_metadata() {
    let dir = tempdir().unwrap();
    let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

    manager.create("Valid", None).unwrap();

    let orphan_dir = dir.path().join("20250101T000000");
    std::fs::create_dir_all(&orphan_dir).unwrap();
    std::fs::write(orphan_dir.join("TODO.md"), "# Orphan\n").unwrap();

    let plans = manager.list().unwrap();
    assert_eq!(
        plans.len(),
        1,
        "orphan dir without metadata should be skipped"
    );
    assert_eq!(plans[0].metadata.title, "Valid");
}

// -- link_session additional tests ----------------------------------------

#[test]
fn test_link_session_to_nonexistent_plan_errors() {
    let dir = tempdir().unwrap();
    let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

    let result = manager.link_session("99991231T235959", "session-1");
    assert!(result.is_err());
}

#[test]
fn test_link_session_preserves_other_metadata() {
    let dir = tempdir().unwrap();
    let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

    let plan = manager.create("Test", Some("feat/branch")).unwrap();
    manager
        .update_status(&plan.timestamp, TodoStatus::Approved)
        .unwrap();

    manager.link_session(&plan.timestamp, "sess-1").unwrap();

    let reloaded = manager.load(&plan.timestamp).unwrap();
    assert_eq!(
        reloaded.metadata.status,
        TodoStatus::Approved,
        "status should be preserved after link_session"
    );
    assert_eq!(
        reloaded.metadata.branch.as_deref(),
        Some("feat/branch"),
        "branch should be preserved after link_session"
    );
    assert_eq!(reloaded.metadata.title, "Test");
}

// -- validate_timestamp boundary tests ------------------------------------

#[test]
fn test_validate_timestamp_valid_formats() {
    assert!(validate_timestamp("20260211T023000").is_ok());
    assert!(validate_timestamp("20260211T023000-1").is_ok());
    assert!(validate_timestamp("20260211T023000-99").is_ok());
}

#[test]
fn test_validate_timestamp_rejects_invalid() {
    assert!(validate_timestamp("").is_err());
    assert!(validate_timestamp("../etc").is_err());
    assert!(validate_timestamp("a/b").is_err());
    assert!(validate_timestamp("a\\b").is_err());
    assert!(validate_timestamp("2026abc").is_err());
    assert!(validate_timestamp("2026 01").is_err());
}

// -- TodoStatus parsing boundary ------------------------------------------

#[test]
fn test_todo_status_case_insensitive() {
    assert_eq!("DRAFT".parse::<TodoStatus>().unwrap(), TodoStatus::Draft);
    assert_eq!("Draft".parse::<TodoStatus>().unwrap(), TodoStatus::Draft);
    assert_eq!(
        "IMPLEMENTING".parse::<TodoStatus>().unwrap(),
        TodoStatus::Implementing
    );
}

// -- atomic_write / create rollback boundary ------------------------------

#[test]
fn test_create_generates_todo_md_with_title() {
    let dir = tempdir().unwrap();
    let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

    let plan = manager.create("My Feature", None).unwrap();

    let content = std::fs::read_to_string(plan.todo_md_path()).unwrap();
    assert!(
        content.contains("My Feature"),
        "TODO.md should contain the plan title"
    );
    assert!(
        content.contains("## Goal"),
        "TODO.md should have Goal section"
    );
    assert!(
        content.contains("## Tasks"),
        "TODO.md should have Tasks section"
    );
}

#[test]
fn test_create_collision_suffix() {
    let dir = tempdir().unwrap();
    let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

    let plan_a = manager.create("A", None).unwrap();
    let plan_b = manager.create("B", None).unwrap();

    assert_ne!(plan_a.timestamp, plan_b.timestamp);
    assert_eq!(plan_a.metadata.title, "A");
    assert_eq!(plan_b.metadata.title, "B");
}
