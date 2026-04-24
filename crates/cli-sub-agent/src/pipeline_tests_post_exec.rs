use super::*;
use crate::test_session_sandbox::ScopedSessionSandbox;
use csa_session::{create_session, get_session_dir, load_session};

#[test]
fn ensure_terminal_result_on_post_exec_error_writes_missing_result() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new_blocking(&tmp);
    let project_root = tmp.path();
    let mut session =
        create_session(project_root, Some("test"), None, Some("codex")).expect("create session");

    assert!(
        load_result(project_root, &session.meta_session_id)
            .expect("load result")
            .is_none(),
        "precondition: result.toml must be missing"
    );

    let started_at = chrono::Utc::now() - chrono::Duration::seconds(1);
    let err = anyhow::anyhow!("post-run hook failed");
    ensure_terminal_result_on_post_exec_error(
        project_root,
        &mut session,
        "codex",
        started_at,
        &err,
    );

    let persisted = load_result(project_root, &session.meta_session_id)
        .expect("load fallback result")
        .expect("fallback result should exist");
    assert_eq!(persisted.status, "failure");
    assert_eq!(persisted.exit_code, 1);
    assert!(
        persisted.summary.contains("post-exec:"),
        "summary should indicate post-exec fallback"
    );

    let reloaded = load_session(project_root, &session.meta_session_id)
        .expect("reload session after fallback");
    assert_eq!(
        reloaded.termination_reason.as_deref(),
        Some("post_exec_error")
    );
}

#[test]
fn ensure_terminal_result_on_post_exec_error_keeps_existing_result() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new_blocking(&tmp);
    let project_root = tmp.path();
    let mut session =
        create_session(project_root, Some("test"), None, Some("codex")).expect("create session");
    let now = chrono::Utc::now();
    let existing = SessionResult {
        status: "success".to_string(),
        exit_code: 0,
        summary: "already persisted".to_string(),
        tool: "codex".to_string(),
        started_at: now,
        completed_at: now,
        events_count: 1,
        artifacts: vec![SessionArtifact::new("output/acp-events.jsonl")],
        peak_memory_mb: None,
        manager_fields: Default::default(),
    };
    save_result(project_root, &session.meta_session_id, &existing).expect("write existing result");

    let err = anyhow::anyhow!("late hook failure");
    ensure_terminal_result_on_post_exec_error(project_root, &mut session, "codex", now, &err);

    let persisted = load_result(project_root, &session.meta_session_id)
        .expect("load existing result")
        .expect("existing result should remain");
    assert_eq!(persisted.status, "success");
    assert_eq!(persisted.exit_code, 0);
    assert_eq!(persisted.summary, "already persisted");
}

#[test]
fn ensure_terminal_result_for_session_on_post_exec_error_persists_output_tail_for_fork() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new_blocking(&tmp);
    let project_root = tmp.path();
    let parent = create_session(project_root, Some("parent"), None, Some("codex"))
        .expect("create parent session");
    let child = create_session(
        project_root,
        Some("fork"),
        Some(&parent.meta_session_id),
        Some("codex"),
    )
    .expect("create forked child session");
    let session_id = child.meta_session_id.clone();
    let session_dir = get_session_dir(project_root, &session_id).expect("session dir");
    fs::create_dir_all(session_dir.join("output")).expect("create output dir");
    fs::write(
        session_dir.join("output.log"),
        "first line\nstill running\npartial summary line\n",
    )
    .expect("write output log");
    fs::write(
        session_dir.join("output").join("user-result.toml"),
        "status = \"success\"\nsummary = \"sidecar\"\n",
    )
    .expect("write sidecar result");

    let started_at = chrono::Utc::now() - chrono::Duration::seconds(1);
    let err = anyhow::anyhow!("wall timeout interrupted fork before post-exec");
    ensure_terminal_result_for_session_on_post_exec_error(
        project_root,
        &session_id,
        "codex",
        started_at,
        &err,
    );

    let persisted = load_result(project_root, &session_id)
        .expect("load fallback result")
        .expect("fallback result should exist");
    assert_eq!(persisted.status, "failure");
    assert_eq!(persisted.exit_code, 1);
    assert!(
        persisted.summary.contains("partial summary line"),
        "summary should include output.log tail"
    );
    assert!(
        persisted
            .artifacts
            .iter()
            .any(|artifact| artifact.path == "output/user-result.toml"),
        "fallback should register user-result sidecar"
    );

    let reloaded = load_session(project_root, &session_id).expect("reload session");
    assert_eq!(
        reloaded.termination_reason.as_deref(),
        Some("post_exec_error")
    );
}

// Handoff artifact tests are in pipeline_handoff.rs

#[test]
fn codex_exec_initial_stall_summary_forces_failure_status_in_result_toml() {
    let now = chrono::Utc::now();
    let mut result = SessionResult {
        status: SessionResult::status_from_exit_code(137),
        exit_code: 137,
        summary: "codex_exec_initial_stall: no stdout within 300s (effort=high, retry_attempted=true, original_effort=xhigh)".to_string(),
        tool: "codex".to_string(),
        started_at: now,
        completed_at: now,
        events_count: 0,
        artifacts: Vec::new(),
        peak_memory_mb: None,
        manager_fields: Default::default(),
    };

    if is_codex_exec_initial_stall_summary(&result.tool, result.exit_code, &result.summary) {
        result.status = SessionResult::status_from_exit_code(1);
    }

    let toml = toml::to_string_pretty(&result).expect("serialize result.toml");
    assert_eq!(result.status, "failure");
    assert!(toml.contains("status = \"failure\""));
    assert!(toml.contains(CODEX_EXEC_INITIAL_STALL_REASON));
}

#[test]
fn codex_exec_initial_stall_detection_rejects_plain_substring_collisions() {
    assert!(!is_codex_exec_initial_stall_summary(
        "codex",
        0,
        "completed successfully after discussing codex_exec_initial_stall handling"
    ));
    assert!(!is_codex_exec_initial_stall_summary(
        "claude-code",
        137,
        "codex_exec_initial_stall: no stdout within 300s (effort=high, retry_attempted=true)"
    ));
}

#[test]
fn read_output_log_tail_reads_from_file_end_window() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let session_dir = tmp.path();
    let contents = (0..1500)
        .map(|idx| format!("line-{idx:04}"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(session_dir.join("output.log"), format!("{contents}\n")).expect("write output");

    let tail = read_output_log_tail(session_dir, 3).expect("tail");
    assert_eq!(tail, "line-1497\nline-1498\nline-1499");
    assert!(
        !tail.contains("line-0000"),
        "tail reader should not depend on loading the full file"
    );
}
