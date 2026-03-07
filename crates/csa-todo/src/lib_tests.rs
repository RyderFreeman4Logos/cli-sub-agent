use super::*;
use tempfile::tempdir;

#[test]
fn test_todo_status_roundtrip() {
    for status in [
        TodoStatus::Draft,
        TodoStatus::Debating,
        TodoStatus::Approved,
        TodoStatus::Implementing,
        TodoStatus::Done,
    ] {
        let s = status.to_string();
        let parsed: TodoStatus = s.parse().unwrap();
        assert_eq!(parsed, status);
    }
}

#[test]
fn test_todo_status_invalid() {
    let result: Result<TodoStatus> = "invalid".parse();
    assert!(result.is_err());
}

#[test]
fn test_create_plan() {
    let dir = tempdir().unwrap();
    let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

    let plan = manager.create("Test Plan", Some("feat/test")).unwrap();

    assert_eq!(plan.metadata.title, "Test Plan");
    assert_eq!(plan.metadata.branch.as_deref(), Some("feat/test"));
    assert_eq!(plan.metadata.status, TodoStatus::Draft);
    assert!(plan.metadata.sessions.is_empty());
    assert!(plan.todo_dir.exists());
    assert!(plan.metadata_path().exists());
    assert!(plan.todo_md_path().exists());
}

#[test]
fn test_create_plan_no_branch() {
    let dir = tempdir().unwrap();
    let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

    let plan = manager.create("No Branch", None).unwrap();
    assert!(plan.metadata.branch.is_none());
}

#[test]
fn test_load_plan() {
    let dir = tempdir().unwrap();
    let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

    let created = manager.create("Load Test", None).unwrap();
    let loaded = manager.load(&created.timestamp).unwrap();

    assert_eq!(loaded.metadata.title, "Load Test");
    assert_eq!(loaded.metadata.status, TodoStatus::Draft);
    assert_eq!(loaded.timestamp, created.timestamp);
}

#[test]
fn test_load_nonexistent() {
    let dir = tempdir().unwrap();
    let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

    let result = manager.load("99991231T235959");
    assert!(result.is_err());
}

#[test]
fn test_list_empty() {
    let dir = tempdir().unwrap();
    let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

    let plans = manager.list().unwrap();
    assert!(plans.is_empty());
}

#[test]
fn test_list_nonexistent_dir() {
    let manager = TodoManager::with_base_dir(PathBuf::from("/nonexistent/todos"));
    let plans = manager.list().unwrap();
    assert!(plans.is_empty());
}

#[test]
fn test_list_multiple() {
    let dir = tempdir().unwrap();
    let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

    manager.create("Plan A", None).unwrap();
    std::thread::sleep(std::time::Duration::from_secs(1));
    manager.create("Plan B", None).unwrap();

    let plans = manager.list().unwrap();
    assert_eq!(plans.len(), 2);
    assert_eq!(plans[0].metadata.title, "Plan B");
    assert_eq!(plans[1].metadata.title, "Plan A");
}

#[test]
fn test_list_skips_hidden_dirs() {
    let dir = tempdir().unwrap();
    let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

    manager.create("Visible", None).unwrap();
    std::fs::create_dir_all(dir.path().join(".git")).unwrap();

    let plans = manager.list().unwrap();
    assert_eq!(plans.len(), 1);
}

#[test]
fn test_find_by_branch() {
    let dir = tempdir().unwrap();
    let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

    manager.create("Plan A", Some("feat/alpha")).unwrap();
    manager.create("Plan B", Some("feat/beta")).unwrap();
    manager.create("Plan C", Some("feat/alpha")).unwrap();

    let found = manager.find_by_branch("feat/alpha").unwrap();
    assert_eq!(found.len(), 2);
    assert!(
        found
            .iter()
            .all(|p| p.metadata.branch.as_deref() == Some("feat/alpha"))
    );
}

#[test]
fn test_find_by_branch_none() {
    let dir = tempdir().unwrap();
    let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

    manager.create("Plan", Some("feat/other")).unwrap();

    let found = manager.find_by_branch("feat/missing").unwrap();
    assert!(found.is_empty());
}

#[test]
fn test_find_by_status() {
    let dir = tempdir().unwrap();
    let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

    let plan = manager.create("Plan", None).unwrap();
    manager
        .update_status(&plan.timestamp, TodoStatus::Approved)
        .unwrap();
    manager.create("Draft Plan", None).unwrap();

    let approved = manager.find_by_status(TodoStatus::Approved).unwrap();
    assert_eq!(approved.len(), 1);
    assert_eq!(approved[0].metadata.title, "Plan");
}

#[test]
fn test_update_status() {
    let dir = tempdir().unwrap();
    let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

    let plan = manager.create("Test", None).unwrap();
    assert_eq!(plan.metadata.status, TodoStatus::Draft);

    let updated = manager
        .update_status(&plan.timestamp, TodoStatus::Implementing)
        .unwrap();
    assert_eq!(updated.metadata.status, TodoStatus::Implementing);

    let reloaded = manager.load(&plan.timestamp).unwrap();
    assert_eq!(reloaded.metadata.status, TodoStatus::Implementing);
}

#[test]
fn test_update_status_nonexistent() {
    let dir = tempdir().unwrap();
    let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

    let result = manager.update_status("99991231T235959", TodoStatus::Done);
    assert!(result.is_err());
}

#[test]
fn test_link_session() {
    let dir = tempdir().unwrap();
    let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

    let plan = manager.create("Test", None).unwrap();

    manager.link_session(&plan.timestamp, "01ABCDEF").unwrap();
    manager.link_session(&plan.timestamp, "01GHIJKL").unwrap();
    manager.link_session(&plan.timestamp, "01ABCDEF").unwrap();

    let reloaded = manager.load(&plan.timestamp).unwrap();
    assert_eq!(reloaded.metadata.sessions.len(), 2);
    assert!(reloaded.metadata.sessions.contains(&"01ABCDEF".to_string()));
    assert!(reloaded.metadata.sessions.contains(&"01GHIJKL".to_string()));
}

#[test]
fn test_write_todo_md() {
    let dir = tempdir().unwrap();
    let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

    let plan = manager.create("Test", None).unwrap();

    let new_content = "# Updated\n\nNew content here.\n";
    manager.write_todo_md(&plan.timestamp, new_content).unwrap();

    let read_back = std::fs::read_to_string(plan.todo_md_path()).unwrap();
    assert_eq!(read_back, new_content);
}

#[test]
fn test_metadata_serialization() {
    let metadata = TodoMetadata {
        branch: Some("feat/test".to_string()),
        status: TodoStatus::Draft,
        title: "Test".to_string(),
        sessions: vec!["01ABC".to_string()],
        language: None,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };

    let toml_str = toml::to_string_pretty(&metadata).unwrap();
    let deserialized: TodoMetadata = toml::from_str(&toml_str).unwrap();

    assert_eq!(deserialized.title, "Test");
    assert_eq!(deserialized.status, TodoStatus::Draft);
    assert_eq!(deserialized.branch.as_deref(), Some("feat/test"));
    assert_eq!(deserialized.sessions, vec!["01ABC"]);
}

#[test]
fn test_load_path_traversal_dotdot() {
    let dir = tempdir().unwrap();
    let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

    let result = manager.load("../etc/passwd");
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("path traversal"));
}

#[test]
fn test_load_path_traversal_absolute() {
    let dir = tempdir().unwrap();
    let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

    let result = manager.load("/tmp/evil");
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("Invalid timestamp"),
        "absolute path should be rejected"
    );
}

#[test]
fn test_load_path_traversal_slash() {
    let dir = tempdir().unwrap();
    let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

    let result = manager.load("a/b");
    assert!(result.is_err());
}

#[test]
fn test_load_empty_timestamp() {
    let dir = tempdir().unwrap();
    let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

    let result = manager.load("");
    assert!(result.is_err());
}

#[test]
fn test_write_todo_md_updates_updated_at() {
    let dir = tempdir().unwrap();
    let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

    let plan = manager.create("Test", None).unwrap();
    let original_updated_at = plan.metadata.updated_at;

    std::thread::sleep(std::time::Duration::from_millis(10));

    manager
        .write_todo_md(&plan.timestamp, "# Updated content\n")
        .unwrap();

    let reloaded = manager.load(&plan.timestamp).unwrap();
    assert!(reloaded.metadata.updated_at > original_updated_at);
}

#[test]
fn test_latest() {
    let dir = tempdir().unwrap();
    let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

    manager.create("Plan A", None).unwrap();
    std::thread::sleep(std::time::Duration::from_secs(1));
    let plan_b = manager.create("Plan B", None).unwrap();

    let latest = manager.latest().unwrap();
    assert_eq!(latest.metadata.title, "Plan B");
    assert_eq!(latest.timestamp, plan_b.timestamp);
}

#[test]
fn test_latest_empty() {
    let dir = tempdir().unwrap();
    let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

    let result = manager.latest();
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("No TODO plans"));
}

#[test]
fn test_status_lifecycle() {
    let dir = tempdir().unwrap();
    let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

    let plan = manager.create("Lifecycle", None).unwrap();
    assert_eq!(plan.metadata.status, TodoStatus::Draft);

    let plan = manager
        .update_status(&plan.timestamp, TodoStatus::Debating)
        .unwrap();
    assert_eq!(plan.metadata.status, TodoStatus::Debating);

    let plan = manager
        .update_status(&plan.timestamp, TodoStatus::Approved)
        .unwrap();
    assert_eq!(plan.metadata.status, TodoStatus::Approved);

    let plan = manager
        .update_status(&plan.timestamp, TodoStatus::Implementing)
        .unwrap();
    assert_eq!(plan.metadata.status, TodoStatus::Implementing);

    let plan = manager
        .update_status(&plan.timestamp, TodoStatus::Done)
        .unwrap();
    assert_eq!(plan.metadata.status, TodoStatus::Done);
}

#[test]
fn test_create_with_language() {
    let dir = tempdir().unwrap();
    let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

    let plan = manager
        .create_with_language("Lang Plan", Some("feat/lang"), Some("Chinese (Simplified)"))
        .unwrap();

    assert_eq!(
        plan.metadata.language.as_deref(),
        Some("Chinese (Simplified)")
    );

    let reloaded = manager.load(&plan.timestamp).unwrap();
    assert_eq!(
        reloaded.metadata.language.as_deref(),
        Some("Chinese (Simplified)")
    );
}

#[test]
fn test_create_without_language() {
    let dir = tempdir().unwrap();
    let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

    let plan = manager.create("No Lang", None).unwrap();
    assert!(plan.metadata.language.is_none());

    let reloaded = manager.load(&plan.timestamp).unwrap();
    assert!(reloaded.metadata.language.is_none());
}

#[test]
fn test_language_field_serde_backward_compat() {
    // Verify that metadata without a language field deserializes correctly
    // (backward compatibility with existing plans).
    let toml_str = r#"
status = "draft"
title = "Old Plan"
sessions = []
created_at = "2025-01-01T00:00:00Z"
updated_at = "2025-01-01T00:00:00Z"
"#;
    let metadata: TodoMetadata = toml::from_str(toml_str).unwrap();
    assert!(metadata.language.is_none());
    assert_eq!(metadata.title, "Old Plan");
}

#[test]
fn test_language_field_skip_serializing_if_none() {
    let metadata = TodoMetadata {
        branch: None,
        status: TodoStatus::Draft,
        title: "Test".to_string(),
        sessions: Vec::new(),
        language: None,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };

    let toml_str = toml::to_string_pretty(&metadata).unwrap();
    assert!(
        !toml_str.contains("language"),
        "language = None should not appear in serialized TOML"
    );
}

include!("lib_ext_tests.rs");
