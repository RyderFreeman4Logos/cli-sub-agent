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
        started_at: chrono::Utc::now(),
        completed_at: chrono::Utc::now(),
        events_count: 0,
        artifacts: vec![crate::result::SessionArtifact::new("output/result.txt")],
    };
    save_result_in(td.path(), &state.meta_session_id, &result).unwrap();
    let loaded = load_result_in(td.path(), &state.meta_session_id)
        .unwrap()
        .unwrap();
    assert_eq!(loaded.status, "success");
    assert_eq!(loaded.exit_code, 0);
    assert_eq!(loaded.tool, "codex");
    assert_eq!(loaded.artifacts.len(), 1);
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
    assert!(status.success(), "git {:?} failed", args);
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
