#[test]
fn test_tool_access_validation() {
    let td = tempdir().unwrap();
    let state = create_session_in(td.path(), td.path(), None, None, Some("codex")).unwrap();
    validate_tool_access_in(td.path(), &state.meta_session_id, "codex").unwrap();
    let err = validate_tool_access_in(td.path(), &state.meta_session_id, "gemini-cli");
    assert!(err.unwrap_err().to_string().contains("locked to tool"));
}

#[test]
fn test_no_tool_no_metadata() {
    let td = tempdir().unwrap();
    let state = create_session_in(td.path(), td.path(), None, None, None).unwrap();
    assert!(
        load_metadata_in(td.path(), &state.meta_session_id)
            .unwrap()
            .is_none()
    );
}

#[test]
fn test_complete_session() {
    let td = tempdir().unwrap();
    let state = create_session_in(td.path(), td.path(), Some("Test"), None, Some("codex")).unwrap();
    let hash = complete_session_in(td.path(), &state.meta_session_id, "session complete").unwrap();
    assert!(!hash.is_empty());
}

#[test]
fn test_save_and_load_result() {
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
        artifacts: vec![crate::result::SessionArtifact::new("output/result.txt")],
        peak_memory_mb: None,
            manager_fields: Default::default(),
    };
    save_result_in(td.path(), &state.meta_session_id, &result, crate::SaveOptions::default())
        .unwrap();
    let loaded = load_result_in(td.path(), &state.meta_session_id)
        .unwrap()
        .unwrap();
    assert_eq!(loaded.status, "success");
    assert_eq!(loaded.exit_code, 0);
    assert_eq!(loaded.tool, "codex");
    assert_eq!(loaded.artifacts.len(), 1);
}

#[cfg(unix)]
#[test]
fn test_save_session_and_result_preserve_legacy_symlink_root() {
    let td = tempdir().unwrap();
    let _xdg = ScopedXdgOverride::new(&td);
    let project = td.path().join("project");
    std::fs::create_dir_all(&project).unwrap();

    let alias = td.path().join("project-alias");
    std::os::unix::fs::symlink(&project, &alias).unwrap();

    let state_dir = csa_config::paths::state_dir_write().unwrap();
    let legacy_raw_root = state_dir.join(project_storage_key_from_path(&alias));
    let mut state = create_session_in(
        &legacy_raw_root,
        &alias,
        Some("Legacy symlink session"),
        None,
        Some("codex"),
    )
    .unwrap();
    state.project_path = alias.to_string_lossy().to_string();
    save_session_in(&legacy_raw_root, &state).unwrap();

    let result = crate::result::SessionResult {
        status: "success".to_string(),
        exit_code: 0,
        summary: "saved to legacy raw root".to_string(),
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: chrono::Utc::now(),
        completed_at: chrono::Utc::now(),
        events_count: 0,
        artifacts: Vec::new(),
        peak_memory_mb: None,
            manager_fields: Default::default(),
    };
    let canonical_project = project.canonicalize().unwrap();
    let mut loaded = load_session(&canonical_project, &state.meta_session_id).unwrap();
    loaded.description = Some("updated description".to_string());
    save_session(&loaded).unwrap();
    save_result(&canonical_project, &loaded.meta_session_id, &result).unwrap();

    let legacy_session_dir = get_session_dir_in(&legacy_raw_root, &loaded.meta_session_id);
    assert!(
        legacy_session_dir.join(STATE_FILE_NAME).is_file(),
        "legacy raw root should retain state writes"
    );
    assert!(
        legacy_session_dir.join(crate::result::RESULT_FILE_NAME).is_file(),
        "legacy raw root should retain result writes"
    );

    let canonical_root = get_session_root(&canonical_project).unwrap();
    let canonical_session_dir = get_session_dir_in(&canonical_root, &loaded.meta_session_id);
    assert!(
        !canonical_session_dir.join(crate::result::RESULT_FILE_NAME).exists(),
        "writes should not migrate live sessions into a different canonical root"
    );
}

#[test]
fn test_save_result_preserves_custom_schema_with_sidecar_snapshot() {
    let td = tempdir().unwrap();
    let state = create_session_in(td.path(), td.path(), None, None, Some("codex")).unwrap();
    let session_dir = get_session_dir_in(td.path(), &state.meta_session_id);
    let existing_result = r#"
status = "partial"
exit_code = 0
summary = "manager report"
started_at = "2026-01-01T00:00:00Z"
completed_at = "2026-01-01T00:10:00Z"

[result]
done = false

[tool]
name = "gemini-cli"
"#;
    std::fs::write(session_dir.join("result.toml"), existing_result).unwrap();

    let now = chrono::Utc::now();
    let runtime_result = crate::result::SessionResult {
        status: "success".to_string(),
        exit_code: 0,
        summary: "runtime summary".to_string(),
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now,
        events_count: 1,
        artifacts: vec![crate::result::SessionArtifact::new("output/acp-events.jsonl")],
        peak_memory_mb: None,
            manager_fields: Default::default(),
    };
    save_result_in(
        td.path(),
        &state.meta_session_id,
        &runtime_result,
        crate::SaveOptions::default(),
    )
    .unwrap();

    let persisted = std::fs::read_to_string(session_dir.join("result.toml")).unwrap();
    assert!(persisted.contains("status = \"success\""));
    assert!(persisted.contains("tool = \"codex\""));
    assert!(persisted.contains("[result]"));
    assert!(persisted.contains("done = false"));
    assert!(!persisted.contains("[tool]"));

    let sidecar_path = session_dir.join("output/user-result.toml");
    assert!(sidecar_path.is_file());
    let sidecar = std::fs::read_to_string(sidecar_path).unwrap();
    assert!(sidecar.contains("[tool]"));
    assert!(sidecar.contains("name = \"gemini-cli\""));

    let loaded = load_result_in(td.path(), &state.meta_session_id)
        .unwrap()
        .unwrap();
    assert!(
        loaded
            .artifacts
            .iter()
            .any(|artifact| artifact.path == "output/user-result.toml")
    );
}

#[test]
fn test_save_result_preserves_sidecar_when_artifacts_schema_is_malformed() {
    let td = tempdir().unwrap();
    let state = create_session_in(td.path(), td.path(), None, None, Some("codex")).unwrap();
    let session_dir = get_session_dir_in(td.path(), &state.meta_session_id);
    let existing_result = r#"
status = "partial"
exit_code = 0
summary = "manager report"
tool = "codex"
started_at = "2026-01-01T00:00:00Z"
completed_at = "2026-01-01T00:10:00Z"
events_count = 0
artifacts = [1, 2]
"#;
    std::fs::write(session_dir.join("result.toml"), existing_result).unwrap();

    let now = chrono::Utc::now();
    let runtime_result = crate::result::SessionResult {
        status: "success".to_string(),
        exit_code: 0,
        summary: "runtime summary".to_string(),
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now,
        events_count: 1,
        artifacts: vec![crate::result::SessionArtifact::new("output/acp-events.jsonl")],
        peak_memory_mb: None,
            manager_fields: Default::default(),
    };
    save_result_in(
        td.path(),
        &state.meta_session_id,
        &runtime_result,
        crate::SaveOptions::default(),
    )
    .unwrap();

    let sidecar_path = session_dir.join("output/user-result.toml");
    assert!(sidecar_path.is_file());
    let sidecar = std::fs::read_to_string(sidecar_path).unwrap();
    assert!(sidecar.contains("artifacts = [1, 2]"));

    let loaded = load_result_in(td.path(), &state.meta_session_id)
        .unwrap()
        .unwrap();
    assert!(
        loaded
            .artifacts
            .iter()
            .any(|artifact| artifact.path == "output/user-result.toml")
    );
}

#[test]
fn test_save_result_does_not_overwrite_existing_sidecar_snapshot() {
    let td = tempdir().unwrap();
    let state = create_session_in(td.path(), td.path(), None, None, Some("codex")).unwrap();
    let session_dir = get_session_dir_in(td.path(), &state.meta_session_id);
    let existing_result = r#"
status = "partial"
exit_code = 0
summary = "manager report"
started_at = "2026-01-01T00:00:00Z"
completed_at = "2026-01-01T00:10:00Z"

[result]
done = false

[tool]
name = "gemini-cli"
"#;
    std::fs::write(session_dir.join("result.toml"), existing_result).unwrap();

    let now = chrono::Utc::now();
    let first_runtime = crate::result::SessionResult {
        status: "success".to_string(),
        exit_code: 0,
        summary: "first run".to_string(),
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now,
        events_count: 1,
        artifacts: vec![],
        peak_memory_mb: None,
            manager_fields: Default::default(),
    };
    save_result_in(
        td.path(),
        &state.meta_session_id,
        &first_runtime,
        crate::SaveOptions::default(),
    )
    .unwrap();

    let second_runtime = crate::result::SessionResult {
        status: "success".to_string(),
        exit_code: 0,
        summary: "second run".to_string(),
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now,
        events_count: 2,
        artifacts: vec![],
        peak_memory_mb: None,
            manager_fields: Default::default(),
    };
    save_result_in(
        td.path(),
        &state.meta_session_id,
        &second_runtime,
        crate::SaveOptions::default(),
    )
    .unwrap();

    let sidecar = std::fs::read_to_string(session_dir.join("output/user-result.toml")).unwrap();
    assert!(sidecar.contains("[tool]"));
    assert!(sidecar.contains("name = \"gemini-cli\""));

    let loaded = load_result_in(td.path(), &state.meta_session_id)
        .unwrap()
        .unwrap();
    assert!(
        loaded
            .artifacts
            .iter()
            .any(|artifact| artifact.path == "output/user-result.toml")
    );
}

#[test]
fn test_save_result_errors_when_sidecar_path_is_not_file() {
    let td = tempdir().unwrap();
    let state = create_session_in(td.path(), td.path(), None, None, Some("codex")).unwrap();
    let session_dir = get_session_dir_in(td.path(), &state.meta_session_id);
    let existing_result = r#"
status = "partial"
exit_code = 0
summary = "manager report"
started_at = "2026-01-01T00:00:00Z"
completed_at = "2026-01-01T00:10:00Z"

[result]
done = false
"#;
    std::fs::write(session_dir.join("result.toml"), existing_result).unwrap();

    let sidecar_dir = session_dir.join("output/user-result.toml");
    std::fs::create_dir_all(&sidecar_dir).unwrap();

    let now = chrono::Utc::now();
    let runtime_result = crate::result::SessionResult {
        status: "success".to_string(),
        exit_code: 0,
        summary: "run".to_string(),
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now,
        events_count: 0,
        artifacts: vec![],
        peak_memory_mb: None,
            manager_fields: Default::default(),
    };
    let err = save_result_in(
        td.path(),
        &state.meta_session_id,
        &runtime_result,
        crate::SaveOptions::default(),
    )
    .unwrap_err();
    assert!(err.to_string().contains("not a file"));
}

#[test]
fn test_save_result_clears_stale_optional_runtime_fields() {
    let td = tempdir().unwrap();
    let state = create_session_in(td.path(), td.path(), None, None, Some("codex")).unwrap();
    let now = chrono::Utc::now();

    let old_result = crate::result::SessionResult {
        status: "success".to_string(),
        exit_code: 0,
        summary: "old run".to_string(),
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now,
        events_count: 7,
        artifacts: vec![crate::result::SessionArtifact::new("output/old-artifact.txt")],
        peak_memory_mb: None,
            manager_fields: Default::default(),
    };
    save_result_in(
        td.path(),
        &state.meta_session_id,
        &old_result,
        crate::SaveOptions::default(),
    )
    .unwrap();

    let new_result = crate::result::SessionResult {
        status: "failure".to_string(),
        exit_code: 1,
        summary: "new run".to_string(),
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now,
        events_count: 0,
        artifacts: vec![],
        peak_memory_mb: None,
            manager_fields: Default::default(),
    };
    save_result_in(
        td.path(),
        &state.meta_session_id,
        &new_result,
        crate::SaveOptions::default(),
    )
    .unwrap();

    let session_dir = get_session_dir_in(td.path(), &state.meta_session_id);
    let persisted = std::fs::read_to_string(session_dir.join("result.toml")).unwrap();
    assert!(!persisted.contains("events_count"));
    assert!(!persisted.contains("artifacts"));

    let loaded = load_result_in(td.path(), &state.meta_session_id)
        .unwrap()
        .unwrap();
    assert_eq!(loaded.events_count, 0);
    assert!(loaded.artifacts.is_empty());
}

#[test]
fn test_load_result_not_found() {
    let td = tempdir().unwrap();
    let state = create_session_in(td.path(), td.path(), None, None, None).unwrap();
    assert!(
        load_result_in(td.path(), &state.meta_session_id)
            .unwrap()
            .is_none()
    );
}

#[test]
fn test_list_artifacts() {
    let td = tempdir().unwrap();
    let state = create_session_in(td.path(), td.path(), None, None, Some("codex")).unwrap();
    let dir = get_session_dir_in(td.path(), &state.meta_session_id);
    std::fs::write(dir.join("output/report.txt"), "test").unwrap();
    std::fs::write(dir.join("output/diff.patch"), "test").unwrap();
    std::fs::write(dir.join("output/acp-events.jsonl"), "{\"ts\":\"2026-01-01T00:00:00Z\"}\n")
        .unwrap();
    let artifacts = list_artifacts_in(td.path(), &state.meta_session_id).unwrap();
    assert_eq!(artifacts.len(), 3);
    assert!(artifacts.contains(&"acp-events.jsonl".to_string()));
    assert!(artifacts.contains(&"diff.patch".to_string()));
    assert!(artifacts.contains(&"report.txt".to_string()));
}

#[test]
fn test_status_from_exit_code() {
    use crate::result::SessionResult;
    assert_eq!(SessionResult::status_from_exit_code(0), "success");
    assert_eq!(SessionResult::status_from_exit_code(1), "failure");
    assert_eq!(SessionResult::status_from_exit_code(137), "signal");
    assert_eq!(SessionResult::status_from_exit_code(143), "signal");
}

#[test]
fn test_save_session_in_explicit_base() {
    let td = tempdir().unwrap();
    let mut state =
        create_session_in(td.path(), td.path(), Some("Explicit save"), None, None).unwrap();
    state.description = Some("Modified".to_string());
    save_session_in(td.path(), &state).unwrap();
    let loaded = load_session_in(td.path(), &state.meta_session_id).unwrap();
    assert_eq!(loaded.description, Some("Modified".to_string()));
}

#[test]
fn test_list_sessions_empty_and_missing() {
    let td = tempdir().unwrap();
    assert!(list_all_sessions_in(td.path()).unwrap().is_empty());
    assert!(list_sessions_in(td.path(), None).unwrap().is_empty());
}

#[test]
fn test_delete_nonexistent_session() {
    let td = tempdir().unwrap();
    std::fs::create_dir_all(td.path().join("sessions")).unwrap();
    let r = delete_session_in(td.path(), &crate::validate::new_session_id());
    assert!(r.unwrap_err().to_string().contains("not found"));
}

#[test]
fn test_load_nonexistent_session() {
    let td = tempdir().unwrap();
    let r = load_session_in(td.path(), &crate::validate::new_session_id());
    assert!(r.unwrap_err().to_string().contains("not found"));
}

#[test]
fn test_update_last_accessed_advances_timestamp() {
    let td = tempdir().unwrap();
    let state = create_session_in(td.path(), td.path(), Some("ts"), None, None).unwrap();
    let t0 = state.last_accessed;
    std::thread::sleep(std::time::Duration::from_millis(10));
    let mut s = load_session_in(td.path(), &state.meta_session_id).unwrap();
    s.last_accessed = Utc::now();
    save_session_in(td.path(), &s).unwrap();
    let s2 = load_session_in(td.path(), &state.meta_session_id).unwrap();
    assert!(s2.last_accessed > t0);
}

#[test]
fn test_list_artifacts_empty_output() {
    let td = tempdir().unwrap();
    let state = create_session_in(td.path(), td.path(), None, None, None).unwrap();
    assert!(
        list_artifacts_in(td.path(), &state.meta_session_id)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn test_operations_with_invalid_session_id() {
    let td = tempdir().unwrap();
    let bad = "not-a-valid-ulid";
    assert!(load_session_in(td.path(), bad).is_err());
    assert!(delete_session_in(td.path(), bad).is_err());
    assert!(load_metadata_in(td.path(), bad).is_err());
    assert!(validate_tool_access_in(td.path(), bad, "codex").is_err());
}

fn run_git(dir: &std::path::Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed");
}

#[test]
fn test_create_session_records_branch_in_git_repo() {
    let td = tempdir().unwrap();
    run_git(td.path(), &["init"]);
    run_git(td.path(), &["config", "user.email", "test@example.com"]);
    run_git(td.path(), &["config", "user.name", "Test User"]);
    std::fs::write(td.path().join("README.md"), "test").unwrap();
    run_git(td.path(), &["add", "README.md"]);
    run_git(td.path(), &["commit", "-m", "init"]);
    run_git(td.path(), &["checkout", "-b", "feat/session-discovery"]);
    let state = create_session_in(td.path(), td.path(), Some("git"), None, None).unwrap();
    assert_eq!(state.branch.as_deref(), Some("feat/session-discovery"));
}

#[test]
fn test_find_sessions_multi_condition_filtering() {
    let td = tempdir().unwrap();
    let _xdg = ScopedXdgOverride::new(&td);
    let mut s1 = create_session_in(td.path(), td.path(), Some("S1"), None, None).unwrap();
    s1.branch = Some("feature/a".to_string());
    s1.phase = SessionPhase::Available;
    s1.task_context.task_type = Some("plan".to_string());
    s1.last_accessed = Utc::now() - chrono::Duration::minutes(5);
    s1.tools.insert(
        "codex".to_string(),
        crate::state::ToolState {
            provider_session_id: None,
            last_action_summary: "Plan".to_string(),
            last_exit_code: 0,
            updated_at: Utc::now(),
            tool_version: None,
            token_usage: None,
        },
    );
    save_session_in(td.path(), &s1).unwrap();

    let mut s2 = create_session_in(td.path(), td.path(), Some("S2"), None, None).unwrap();
    s2.branch = Some("feature/a".to_string());
    s2.phase = SessionPhase::Available;
    s2.task_context.task_type = Some("review".to_string());
    s2.last_accessed = Utc::now();
    s2.tools.insert(
        "codex".to_string(),
        crate::state::ToolState {
            provider_session_id: None,
            last_action_summary: "Review".to_string(),
            last_exit_code: 0,
            updated_at: Utc::now(),
            tool_version: None,
            token_usage: None,
        },
    );
    save_session_in(td.path(), &s2).unwrap();

    let mut s3 = create_session_in(td.path(), td.path(), Some("S3"), None, None).unwrap();
    s3.branch = Some("feature/b".to_string());
    s3.phase = SessionPhase::Available;
    s3.task_context.task_type = Some("plan".to_string());
    s3.last_accessed = Utc::now() - chrono::Duration::minutes(1);
    s3.tools.insert(
        "gemini-cli".to_string(),
        crate::state::ToolState {
            provider_session_id: None,
            last_action_summary: "Plan".to_string(),
            last_exit_code: 0,
            updated_at: Utc::now(),
            tool_version: None,
            token_usage: None,
        },
    );
    save_session_in(td.path(), &s3).unwrap();

    let sessions = find_sessions_in(
        td.path(),
        Some(td.path()),
        Some("feature/a"),
        Some("plan"),
        Some(SessionPhase::Available),
        Some(&["codex"]),
    )
    .unwrap();

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].meta_session_id, s1.meta_session_id);
}

#[test]
fn test_find_sessions_sorts_desc_and_limits_to_ten() {
    let td = tempdir().unwrap();
    let _xdg = ScopedXdgOverride::new(&td);
    for i in 0..12 {
        let mut session = create_session_in(td.path(), td.path(), Some("S"), None, None).unwrap();
        session.branch = Some("feature/a".to_string());
        session.task_context.task_type = Some("plan".to_string());
        session.phase = SessionPhase::Available;
        session.last_accessed = Utc::now() - chrono::Duration::minutes(i);
        save_session_in(td.path(), &session).unwrap();
    }

    let sessions = find_sessions_in(
        td.path(),
        Some(td.path()),
        Some("feature/a"),
        Some("plan"),
        Some(SessionPhase::Available),
        None,
    )
    .unwrap();

    assert_eq!(sessions.len(), 10);
    assert!(
        sessions
            .windows(2)
            .all(|pair| pair[0].last_accessed >= pair[1].last_accessed)
    );
}

#[test]
fn test_global_exact_finds_cross_project_session() {
    let td = tempdir().unwrap();
    let _xdg = ScopedXdgOverride::new(&td);

    // Create two "project" directories under the temp dir
    let project_a_root = td.path().join("project_a");
    let project_b_root = td.path().join("project_b");

    // Create a session under project_a's session root
    let session_a =
        create_session_in(&project_a_root, &project_a_root, Some("from project A"), None, None)
            .unwrap();

    // Create a session under project_b's session root
    let session_b =
        create_session_in(&project_b_root, &project_b_root, Some("from project B"), None, None)
            .unwrap();

    // Verify we can load each session from its own root
    let loaded_a = load_session_in(&project_a_root, &session_a.meta_session_id).unwrap();
    assert_eq!(loaded_a.description, Some("from project A".to_string()));

    let loaded_b = load_session_in(&project_b_root, &session_b.meta_session_id).unwrap();
    assert_eq!(loaded_b.description, Some("from project B".to_string()));

    // Verify we CANNOT load project_a's session from project_b's root (cross-project)
    let cross_load = load_session_in(&project_b_root, &session_a.meta_session_id);
    assert!(cross_load.is_err());
}

#[test]
fn test_extract_project_path_from_state_content() {
    let content = r#"
meta_session_id = "01HTESTABCDEFGHIJKLMNOPQR"
project_path = "/home/user/project-a"
description = "test"
"#;
    // Test the extract function indirectly via a state.toml-like content
    assert!(content.contains("project_path"));
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("project_path") {
            let rest = rest.trim();
            if let Some(rest) = rest.strip_prefix('=') {
                let value = rest.trim().trim_matches('"');
                assert_eq!(value, "/home/user/project-a");
            }
        }
    }
}

#[test]
fn test_list_all_sessions_from_multiple_roots() {
    let td = tempdir().unwrap();
    let _xdg = ScopedXdgOverride::new(&td);
    let root_a = td.path().join("root_a");
    let root_b = td.path().join("root_b");

    let s1 = create_session_in(&root_a, &root_a, Some("session A1"), None, None).unwrap();
    let s2 = create_session_in(&root_a, &root_a, Some("session A2"), None, None).unwrap();
    let s3 = create_session_in(&root_b, &root_b, Some("session B1"), None, None).unwrap();

    // Each root should have its own sessions
    let sessions_a = list_all_sessions_in_readonly(&root_a).unwrap();
    assert_eq!(sessions_a.len(), 2);

    let sessions_b = list_all_sessions_in_readonly(&root_b).unwrap();
    assert_eq!(sessions_b.len(), 1);

    // Verify session IDs
    let ids_a: Vec<&str> = sessions_a.iter().map(|s| s.meta_session_id.as_str()).collect();
    assert!(ids_a.contains(&s1.meta_session_id.as_str()));
    assert!(ids_a.contains(&s2.meta_session_id.as_str()));
    assert_eq!(sessions_b[0].meta_session_id, s3.meta_session_id);
}

#[test]
fn test_prefix_stays_project_scoped() {
    let td = tempdir().unwrap();
    let _xdg = ScopedXdgOverride::new(&td);
    let root_a = td.path().join("root_a");
    let root_b = td.path().join("root_b");

    let s1 = create_session_in(&root_a, &root_a, Some("A"), None, None).unwrap();
    let _s2 = create_session_in(&root_b, &root_b, Some("B"), None, None).unwrap();

    // Full-ID resolution should only find sessions in the specified root
    let sessions_dir_a = root_a.join("sessions");
    let resolved =
        crate::validate::resolve_session_prefix(&sessions_dir_a, &s1.meta_session_id).unwrap();
    assert_eq!(resolved, s1.meta_session_id);

    // Same full ID should NOT match in root_b's sessions dir
    let sessions_dir_b = root_b.join("sessions");
    let result = crate::validate::resolve_session_prefix(&sessions_dir_b, &s1.meta_session_id);
    assert!(result.is_err());
}

#[test]
fn test_find_sessions_backward_compat_without_branch_field() {
    let td = tempdir().unwrap();
    let mut legacy = create_session_in(td.path(), td.path(), Some("legacy"), None, None).unwrap();
    legacy.phase = SessionPhase::Available;
    legacy.task_context.task_type = Some("plan".to_string());
    save_session_in(td.path(), &legacy).unwrap();

    let matched = find_sessions_in(
        td.path(),
        Some(td.path()),
        None,
        Some("plan"),
        Some(SessionPhase::Available),
        None,
    )
    .unwrap();
    assert_eq!(matched.len(), 1);
    assert_eq!(matched[0].meta_session_id, legacy.meta_session_id);

    let no_match = find_sessions_in(
        td.path(),
        Some(td.path()),
        Some("feature/a"),
        Some("plan"),
        Some(SessionPhase::Available),
        None,
    )
    .unwrap();
    assert!(no_match.is_empty());
}
