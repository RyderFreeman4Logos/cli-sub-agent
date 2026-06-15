use super::*;
use crate::test_session_sandbox::ScopedSessionSandbox;
use csa_session::{
    TaskContext, TokenUsage, ToolState, create_session, get_session_dir, save_result, save_session,
};
use tempfile::tempdir;

fn make_result(tool: &str, status: &str, exit_code: i32, at: DateTime<Utc>) -> SessionResult {
    SessionResult {
        status: status.to_string(),
        exit_code,
        summary: "completed work".to_string(),
        tool: tool.to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: at - Duration::seconds(30),
        completed_at: at,
        events_count: 0,
        artifacts: Vec::new(),
        ..Default::default()
    }
}

#[cfg(unix)]
fn set_file_mtime_seconds_ago(path: &std::path::Path, seconds_ago: u64) {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock before unix epoch");
    let target = now.saturating_sub(std::time::Duration::from_secs(seconds_ago));
    let tv_sec = target.as_secs() as libc::time_t;
    let tv_nsec = target.subsec_nanos() as libc::c_long;
    let times = [
        libc::timespec { tv_sec, tv_nsec },
        libc::timespec { tv_sec, tv_nsec },
    ];
    let c_path = CString::new(path.as_os_str().as_bytes()).expect("path contains NUL");
    // SAFETY: path is NUL-terminated and timespec array lives for the syscall.
    let rc = unsafe { libc::utimensat(libc::AT_FDCWD, c_path.as_ptr(), times.as_ptr(), 0) };
    assert_eq!(rc, 0, "utimensat failed for {}", path.display());
}

#[cfg(unix)]
fn backdate_tree(path: &std::path::Path, seconds_ago: u64) {
    if path.is_dir() {
        for entry in std::fs::read_dir(path).expect("read_dir") {
            let entry = entry.expect("dir entry");
            backdate_tree(&entry.path(), seconds_ago);
        }
    }
    set_file_mtime_seconds_ago(path, seconds_ago);
}

#[test]
fn peek_report_classifies_recent_session_as_idle_and_limits_operations() {
    let tmp = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new_blocking(&tmp);
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let now = Utc::now();
    let mut session = create_session(&project, Some("issue #2106 peek"), None, Some("codex"))
        .expect("create session");
    session.created_at = now - Duration::minutes(5);
    session.last_accessed = now - Duration::seconds(20);
    session.tools.insert(
        "codex".to_string(),
        ToolState {
            provider_session_id: None,
            last_action_summary: "edited session stats".to_string(),
            last_exit_code: 0,
            updated_at: now - Duration::seconds(10),
            tool_version: None,
            token_usage: None,
        },
    );
    save_session(&session).unwrap();
    let session_dir = get_session_dir(&project, &session.meta_session_id).unwrap();
    save_result(
        &project,
        &session.meta_session_id,
        &make_result("codex", "success", 0, now - Duration::seconds(5)),
    )
    .unwrap();

    let report = build_peek_report(
        &session.meta_session_id,
        &session_dir,
        &project,
        2,
        250,
        now,
    )
    .unwrap();

    assert_eq!(report.state, PeekState::Idle);
    assert_eq!(report.idle_secs, 20);
    assert_eq!(report.elapsed_secs, 300);
    assert_eq!(report.operations.len(), 2);
    assert_eq!(report.operations[0].kind, "result");
    assert_eq!(report.operations[1].kind, "tool");

    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(json["state"], "idle");
    assert_eq!(json["idle_secs"], 20);

    let text = render_peek_text(&report);
    assert!(text.contains("State: idle"));
    assert!(text.contains("Operations:"));
}

#[cfg(unix)]
#[test]
fn peek_report_classifies_old_session_without_liveness_as_dead() {
    let tmp = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new_blocking(&tmp);
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let now = Utc::now();
    let mut session = create_session(&project, Some("dead peek"), None, Some("codex")).unwrap();
    session.created_at = now - Duration::hours(2);
    session.last_accessed = now - Duration::hours(1);
    save_session(&session).unwrap();
    let session_dir = get_session_dir(&project, &session.meta_session_id).unwrap();
    backdate_tree(&session_dir, 3_600);

    let report = build_peek_report(
        &session.meta_session_id,
        &session_dir,
        &project,
        5,
        250,
        now,
    )
    .unwrap();

    assert_eq!(report.state, PeekState::Dead);
    assert_eq!(report.phase, SessionPhase::Retired);
    assert!(report.operations.iter().any(|operation| {
        operation.kind == "session_state" && operation.summary == "phase=retired"
    }));
    assert!(!report.operations.iter().any(|operation| {
        operation.kind == "session_state" && operation.summary == "phase=active"
    }));
    let result = csa_session::load_result(&project, &session.meta_session_id)
        .unwrap()
        .expect("dead Active session should be reconciled");
    assert_eq!(result.status, "failure");
    assert_eq!(result.exit_code, 1);
}

#[test]
fn stats_report_filters_since_and_groups_by_issue_and_tool() {
    let tmp = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new_blocking(&tmp);
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let now = Utc::now();

    let mut first = create_session(
        &project,
        Some("Implement GitHub issue #2106"),
        None,
        Some("codex"),
    )
    .unwrap();
    first.branch = Some("fix/2106-session-observability".to_string());
    first.created_at = now - Duration::minutes(20);
    first.last_accessed = now - Duration::minutes(10);
    first.task_context = TaskContext {
        task_type: Some("implement".to_string()),
        tier_name: Some("tier-4-critical".to_string()),
    };
    first.total_token_usage = Some(TokenUsage {
        input_tokens: Some(1_000),
        cache_read_input_tokens: Some(250),
        output_tokens: Some(400),
        reasoning_output_tokens: None,
        total_tokens: Some(1_400),
        estimated_cost_usd: None,
    });
    save_session(&first).unwrap();
    save_result(
        &project,
        &first.meta_session_id,
        &make_result("codex", "success", 0, now - Duration::minutes(10)),
    )
    .unwrap();

    let mut second = create_session(
        &project,
        Some("review issue #2106"),
        None,
        Some("gemini-cli"),
    )
    .unwrap();
    second.created_at = now - Duration::minutes(8);
    second.last_accessed = now - Duration::minutes(4);
    second.total_token_usage = Some(TokenUsage {
        input_tokens: Some(500),
        cache_read_input_tokens: None,
        output_tokens: Some(100),
        reasoning_output_tokens: None,
        total_tokens: None,
        estimated_cost_usd: Some(0.125),
    });
    save_session(&second).unwrap();
    save_result(
        &project,
        &second.meta_session_id,
        &make_result("gemini-cli", "success", 0, now - Duration::minutes(4)),
    )
    .unwrap();

    let mut old = create_session(&project, Some("issue #9999 old"), None, Some("codex")).unwrap();
    old.created_at = now - Duration::days(2);
    old.last_accessed = now - Duration::days(2);
    save_session(&old).unwrap();

    let report = build_stats_report(
        &project,
        "30m".to_string(),
        Duration::minutes(30),
        true,
        true,
        true,
        now,
    )
    .unwrap();

    assert_eq!(report.total.session_count, 2);
    assert_eq!(report.total.tokens.uncached_input_tokens, 1_250);
    assert_eq!(report.total.tokens.cached_input_tokens, 250);
    assert_eq!(report.total.tokens.output_tokens, 500);
    assert_eq!(report.total.tokens.total_tokens, 2_000);
    assert_eq!(
        report.total.cost.as_ref().unwrap().estimated_usd,
        Some(0.125)
    );
    assert_eq!(report.by_issue.len(), 1);
    assert_eq!(report.by_issue[0].key, "#2106");
    assert_eq!(report.by_issue[0].issue_source.as_deref(), Some("explicit"));
    assert_eq!(report.by_tool.len(), 2);
    assert!(report.by_tool.iter().any(|group| group.key == "codex"));
    assert!(report.by_tool.iter().any(|group| group.key == "gemini-cli"));

    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(json["session_count"], 2);
    assert_eq!(json["by_issue"][0]["key"], "#2106");

    let text = render_stats_text(&report);
    assert!(text.contains("Sessions since 30m: 2"));
    assert!(text.contains("By issue:"));
    assert!(text.contains("By tool:"));
}

#[test]
fn stats_cost_is_unknown_without_positive_recorded_estimate() {
    let tmp = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new_blocking(&tmp);
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let now = Utc::now();

    let mut session = create_session(&project, Some("fix/2106"), None, Some("codex")).unwrap();
    session.created_at = now - Duration::minutes(2);
    session.last_accessed = now - Duration::minutes(1);
    session.total_token_usage = Some(TokenUsage {
        input_tokens: Some(100),
        output_tokens: Some(50),
        reasoning_output_tokens: None,
        total_tokens: Some(150),
        estimated_cost_usd: Some(0.0),
        cache_read_input_tokens: None,
    });
    save_session(&session).unwrap();

    let report = build_stats_report(
        &project,
        "10m".to_string(),
        Duration::minutes(10),
        false,
        false,
        true,
        now,
    )
    .unwrap();

    let cost = report.total.cost.as_ref().expect("cost requested");
    assert_eq!(cost.estimated_usd, None);
    assert_eq!(cost.source, CostSource::Unknown);
}

#[test]
fn issue_link_uses_branch_number_only_as_heuristic_fallback() {
    let mut session = MetaSessionState {
        description: Some("general implementation".to_string()),
        branch: Some("fix/2106-session-observability".to_string()),
        ..Default::default()
    };
    let (key, source) = issue_key_for_session(&session);
    assert_eq!(key, "#2106");
    assert_eq!(source, IssueSource::Heuristic);

    session.description = Some("GitHub issue #1762 stats".to_string());
    let (key, source) = issue_key_for_session(&session);
    assert_eq!(key, "#1762");
    assert_eq!(source, IssueSource::Explicit);
}

#[cfg(unix)]
#[test]
fn peek_preserves_fail_closed_daemon_completion_without_result() {
    let tmp = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new_blocking(&tmp);
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let now = Utc::now();
    let mut session = create_session(
        &project,
        Some("result-success-completion-without-result"),
        None,
        Some("codex"),
    )
    .unwrap();
    session.last_accessed = now - Duration::minutes(30);
    save_session(&session).unwrap();
    let session_dir = get_session_dir(&project, &session.meta_session_id).unwrap();
    std::fs::write(
        session_dir.join("daemon-completion.toml"),
        "exit_code = 0\nstatus = \"success\"\n",
    )
    .unwrap();
    backdate_tree(&session_dir, 1_800);

    let report = build_peek_report(
        &session.meta_session_id,
        &session_dir,
        &project,
        5,
        250,
        now,
    )
    .unwrap();

    assert_eq!(report.state, PeekState::Dead);
    let result = csa_session::load_result(&project, &session.meta_session_id)
        .unwrap()
        .expect("session peek should synthesize fail-closed result");
    assert_eq!(result.status, "failure");
    assert_eq!(result.exit_code, 1);
    assert_eq!(result.raw_process_exit_code, Some(0));
    assert!(
        result
            .summary
            .contains("treating daemon completion as failure")
    );
}
