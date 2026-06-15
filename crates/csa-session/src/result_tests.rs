use super::*;
use chrono::Utc;

// ── Serialization round-trip ───────────────────────────────────

#[test]
fn test_session_result_toml_roundtrip() {
    let now = Utc::now();
    let result = SessionResult {
        post_exec_gate: None,
        status: "success".to_string(),
        exit_code: 0,
        summary: "All tests passed".to_string(),
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now,
        events_count: 4,
        artifacts: vec![SessionArtifact::new("output/diff.patch")],
        peak_memory_mb: None,
        kill_hint: Some("memory_pressure".to_string()),
        last_item: Some("running tests".to_string()),
        fallback_chain: None,
        ..Default::default()
    };

    let toml_str = toml::to_string_pretty(&result).expect("Serialize should succeed");
    let loaded: SessionResult = toml::from_str(&toml_str).expect("Deserialize should succeed");

    assert_eq!(loaded.status, "success");
    assert_eq!(loaded.exit_code, 0);
    assert_eq!(loaded.summary, "All tests passed");
    assert_eq!(loaded.tool, "codex");
    assert_eq!(loaded.events_count, 4);
    assert_eq!(loaded.artifacts.len(), 1);
    assert_eq!(loaded.artifacts[0].path, "output/diff.patch");
    assert_eq!(loaded.kill_hint.as_deref(), Some("memory_pressure"));
    assert_eq!(loaded.last_item.as_deref(), Some("running tests"));
}

#[test]
fn test_session_result_empty_optional_fields_omitted() {
    let now = Utc::now();
    let result = SessionResult {
        post_exec_gate: None,
        status: "failure".to_string(),
        exit_code: 1,
        summary: "Build failed".to_string(),
        tool: "gemini-cli".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now,
        events_count: 0,
        artifacts: vec![],
        ..Default::default()
    };

    let toml_str = toml::to_string_pretty(&result).expect("Serialize should succeed");
    assert!(
        !toml_str.contains("artifacts"),
        "Empty artifacts should be omitted from serialization"
    );
    assert!(
        !toml_str.contains("events_count"),
        "Zero events_count should be omitted from serialization"
    );
    assert!(
        !toml_str.contains("uncommitted_changes"),
        "Clean sessions should omit uncommitted_changes"
    );
    assert!(
        !toml_str.contains("require_commit_recovery"),
        "Clean sessions should omit require_commit_recovery"
    );
    assert!(
        !toml_str.contains("kill_hint"),
        "Missing kill hint should be omitted from serialization"
    );
    assert!(
        !toml_str.contains("kill_diagnostics"),
        "Missing kill diagnostics should be omitted from serialization"
    );
    assert!(
        !toml_str.contains("last_item"),
        "Missing last_item should be omitted from serialization"
    );

    let loaded: SessionResult = toml::from_str(&toml_str).expect("Deserialize should succeed");
    assert!(loaded.artifacts.is_empty());
    assert_eq!(loaded.events_count, 0);
    assert!(loaded.uncommitted_changes.is_none());
    assert!(loaded.require_commit_recovery.is_none());
}

#[test]
fn test_session_result_require_commit_recovery_roundtrip() {
    let now = Utc::now();
    let result = SessionResult {
        status: "failure".to_string(),
        exit_code: 1,
        summary: "writer session ended with uncommitted changes (--require-commit set)".to_string(),
        tool: "codex".to_string(),
        started_at: now,
        completed_at: now,
        require_commit_recovery: Some(RequireCommitRecoveryDiagnostic {
            require_commit: true,
            commit_created: false,
            dirty_worktree: true,
            changed_paths: vec!["src/lib.rs".to_string(), "README.md".to_string()],
            changed_paths_truncated: 2,
            termination_status: "signal".to_string(),
            exit_code: 143,
            termination_signal: Some(15),
            kill_hint: Some("memory_pressure".to_string()),
            suggested_recovery_action: "inspect_changed_paths_then_commit_or_revert".to_string(),
        }),
        ..Default::default()
    };

    let toml_str = toml::to_string_pretty(&result).expect("Serialize should succeed");
    assert!(toml_str.contains("[require_commit_recovery]"));
    assert!(toml_str.contains("require_commit = true"));
    assert!(toml_str.contains("commit_created = false"));
    assert!(!toml_str.contains("file contents"));

    let loaded: SessionResult = toml::from_str(&toml_str).expect("Deserialize should succeed");
    let recovery = loaded
        .require_commit_recovery
        .expect("recovery diagnostic should roundtrip");
    assert!(recovery.require_commit);
    assert!(!recovery.commit_created);
    assert!(recovery.dirty_worktree);
    assert_eq!(
        recovery.changed_paths,
        vec!["src/lib.rs".to_string(), "README.md".to_string()]
    );
    assert_eq!(recovery.changed_paths_truncated, 2);
    assert_eq!(recovery.termination_status, "signal");
    assert_eq!(recovery.exit_code, 143);
    assert_eq!(recovery.termination_signal, Some(15));
    assert_eq!(recovery.kill_hint.as_deref(), Some("memory_pressure"));
}

#[test]
fn test_session_result_uncommitted_changes_roundtrip() {
    let now = Utc::now();
    let result = SessionResult {
        post_exec_gate: None,
        status: "success".to_string(),
        exit_code: 0,
        summary: "Done".to_string(),
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now,
        events_count: 0,
        artifacts: vec![],
        peak_memory_mb: None,
        kill_hint: None,
        kill_diagnostics: None,
        last_item: None,
        fallback_chain: None,
        gate_timeout: false,
        warnings: Vec::new(),
        raw_process_exit_code: None,
        uncommitted_changes: Some(UncommittedChanges {
            file_count: 7,
            insertions: 240,
            deletions: 12,
            approx_diff_tokens: 1_024,
            files: vec!["src/lib.rs".to_string()],
            truncated: 6,
        }),
        large_diff_warning: Some(LargeDiffWarningReport {
            changed_files: 7,
            changed_lines: 252,
            approx_diff_tokens: 1_024,
        }),
        require_commit_recovery: None,
        manager_fields: Default::default(),
    };

    let toml_str = toml::to_string_pretty(&result).expect("Serialize should succeed");
    assert!(toml_str.contains("[uncommitted_changes]"));

    let loaded: SessionResult = toml::from_str(&toml_str).expect("Deserialize should succeed");
    let changes = loaded
        .uncommitted_changes
        .expect("uncommitted_changes should roundtrip");
    assert_eq!(changes.file_count, 7);
    assert_eq!(changes.insertions, 240);
    assert_eq!(changes.deletions, 12);
    assert_eq!(changes.changed_lines(), 252);
    assert_eq!(changes.approx_diff_tokens, 1_024);
    assert_eq!(changes.truncated, 6);
    let warning = loaded
        .large_diff_warning
        .expect("large_diff_warning should roundtrip");
    assert_eq!(warning.changed_files, 7);
    assert_eq!(warning.changed_lines, 252);
    assert_eq!(warning.approx_diff_tokens, 1_024);
}

#[test]
fn test_session_result_artifacts_support_legacy_path_strings() {
    let raw = r#"
status = "success"
exit_code = 0
summary = "ok"
tool = "codex"
started_at = "2026-01-01T00:00:00Z"
completed_at = "2026-01-01T00:00:00Z"
artifacts = ["output/a.txt", "output/b.txt"]
"#;
    let loaded: SessionResult = toml::from_str(raw).expect("Deserialize should succeed");
    assert_eq!(loaded.artifacts.len(), 2);
    assert_eq!(loaded.artifacts[0].path, "output/a.txt");
    assert_eq!(loaded.artifacts[1].path, "output/b.txt");
}

#[test]
fn test_result_without_post_exec_gate_deserializes_to_none() {
    // Backward compat (#1726): pre-existing result.toml files have no
    // [post_exec_gate] table; they must still deserialize, with the field None.
    let raw = r#"
status = "success"
exit_code = 0
summary = "ok"
tool = "codex"
started_at = "2026-01-01T00:00:00Z"
completed_at = "2026-01-01T00:00:00Z"
"#;
    let loaded: SessionResult = toml::from_str(raw).expect("Deserialize should succeed");
    assert!(loaded.post_exec_gate.is_none());
}

#[test]
fn test_successful_result_omits_post_exec_gate_table() {
    // A successful (post_exec_gate = None) result must serialize WITHOUT a
    // [post_exec_gate] table, so successful sessions emit no spurious table.
    let now = Utc::now();
    let result = SessionResult {
        post_exec_gate: None,
        status: "success".to_string(),
        exit_code: 0,
        summary: "ok".to_string(),
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now,
        events_count: 0,
        artifacts: Vec::new(),
        ..Default::default()
    };
    let toml_str = toml::to_string_pretty(&result).expect("serialize");
    assert!(
        !toml_str.contains("post_exec_gate"),
        "successful result must not serialize a [post_exec_gate] table: {toml_str}"
    );
}

#[test]
fn test_result_with_post_exec_gate_table_roundtrips() {
    // A failed-gate result round-trips its [post_exec_gate] table intact.
    let now = Utc::now();
    let report = crate::post_exec_gate_report::PostExecGateReport::from_redacted_gate_output(
        "just pre-commit",
        100,
        "FAIL [   0.005s] pkg::a\nerror: Recipe `test` failed on line 1 with exit code 100",
    );
    let result = SessionResult {
        post_exec_gate: Some(report.clone()),
        status: "failure".to_string(),
        exit_code: 1,
        summary: "POST-EXEC GATE FAILED (exit=100, step=just test)".to_string(),
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now,
        events_count: 0,
        artifacts: Vec::new(),
        ..Default::default()
    };
    let toml_str = toml::to_string_pretty(&result).expect("serialize");
    let loaded: SessionResult = toml::from_str(&toml_str).expect("Deserialize should succeed");
    assert_eq!(loaded.post_exec_gate, Some(report));
}

// ── status_from_exit_code ──────────────────────────────────────

#[test]
fn test_status_from_exit_code_success() {
    assert_eq!(SessionResult::status_from_exit_code(0), "success");
}

#[test]
fn test_status_from_exit_code_failure() {
    assert_eq!(SessionResult::status_from_exit_code(1), "failure");
    assert_eq!(SessionResult::status_from_exit_code(2), "failure");
    assert_eq!(SessionResult::status_from_exit_code(127), "failure");
}

#[test]
fn test_status_from_exit_code_signal() {
    assert_eq!(SessionResult::status_from_exit_code(137), "signal"); // SIGKILL
    assert_eq!(SessionResult::status_from_exit_code(143), "signal"); // SIGTERM
}

#[test]
fn test_status_from_exit_code_negative() {
    // Negative exit codes should be treated as failure
    assert_eq!(SessionResult::status_from_exit_code(-1), "failure");
}

// ── File I/O round-trip ────────────────────────────────────────

#[test]
fn test_session_result_file_roundtrip() {
    let tmp = tempfile::tempdir().expect("Failed to create temp dir");
    let path = tmp.path().join(RESULT_FILE_NAME);

    let now = Utc::now();
    let result = SessionResult {
        post_exec_gate: None,
        status: "success".to_string(),
        exit_code: 0,
        summary: "Done".to_string(),
        tool: "opencode".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now,
        events_count: 2,
        artifacts: vec![
            SessionArtifact::new("output/a.txt"),
            SessionArtifact::with_stats("output/acp-events.jsonl", 10, 256),
        ],
        ..Default::default()
    };

    let contents = toml::to_string_pretty(&result).unwrap();
    std::fs::write(&path, &contents).expect("Write should succeed");

    let read_back = std::fs::read_to_string(&path).expect("Read should succeed");
    let loaded: SessionResult = toml::from_str(&read_back).expect("Parse should succeed");

    assert_eq!(loaded.status, result.status);
    assert_eq!(loaded.exit_code, result.exit_code);
    assert_eq!(loaded.events_count, 2);
    assert_eq!(loaded.artifacts.len(), 2);
}
