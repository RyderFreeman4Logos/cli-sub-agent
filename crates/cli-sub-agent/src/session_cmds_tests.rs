use super::resolve::resolve_session_prefix_from_dirs;
use super::{
    DeadActiveSessionReconciliation, display_acp_events, display_daemon_spool_logs,
    display_log_files, ensure_terminal_result_for_dead_active_session,
    ensure_terminal_result_for_dead_active_session_with_before_write,
    filter_sessions_by_csa_version, handle_session_is_alive, handle_session_kill,
    handle_session_list, handle_session_wait, is_session_stale_for_test, print_content_with_tail,
    resolve_session_status, select_sessions_for_list, session_to_json,
    status_from_phase_and_result, truncate_with_ellipsis,
};
use crate::cli::{Cli, Commands, SessionCommands};
use crate::session_cmds_daemon::{
    persist_daemon_completion_from_env, seed_daemon_session_env, synthesized_wait_next_step,
};
use crate::test_env_lock::TEST_ENV_LOCK;
use crate::test_session_sandbox::ScopedSessionSandbox;
use chrono::Utc;
use clap::{CommandFactory, Parser};
use csa_session::{
    ContextStatus, Genealogy, MetaSessionState, SessionPhase, SessionResult, TaskContext,
    TokenUsage, create_session, delete_session, get_session_dir, get_session_root, load_result,
    load_session, save_result, save_session,
};
use std::collections::HashMap;
use tempfile::tempdir;
#[test]
fn truncate_with_ellipsis_preserves_ascii_short_input() {
    let input = "short description";
    assert_eq!(truncate_with_ellipsis(input, 25), "short description");
}

#[test]
fn truncate_with_ellipsis_handles_multibyte_chinese() {
    let input = "\u{8FD9}\u{662F}\u{4E00}\u{4E2A}\u{7528}\u{4E8E}\u{6D4B}\u{8BD5}\u{622A}\u{65AD}\u{903B}\u{8F91}\u{7684}\u{4E2D}\u{6587}\u{63CF}\u{8FF0}\u{6587}\u{672C}";
    let expected = "\u{8FD9}\u{662F}\u{4E00}\u{4E2A}\u{7528}\u{4E8E}\u{6D4B}...";
    assert_eq!(truncate_with_ellipsis(input, 10), expected);
}
#[test]
fn truncate_with_ellipsis_handles_emoji_without_panic() {
    let input = "session 😀😃😄😁 description";
    assert_eq!(truncate_with_ellipsis(input, 12), "session 😀...");
}

fn make_result(status: &str, exit_code: i32) -> SessionResult {
    let now = Utc::now();
    SessionResult {
        status: status.to_string(),
        exit_code,
        summary: "summary".to_string(),
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now,
        events_count: 0,
        artifacts: Vec::new(),
        peak_memory_mb: None,
        manager_fields: Default::default(),
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
    // SAFETY: `utimensat` is called with a valid NUL-terminated path and stack-allocated timespec array.
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

struct EnvVarGuard {
    key: &'static str,
    original: Option<String>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let original = std::env::var(key).ok();
        // SAFETY: test-scoped env mutation guarded by a process-wide mutex.
        unsafe { std::env::set_var(key, value) };
        Self { key, original }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        // SAFETY: test-scoped env mutation guarded by a process-wide mutex.
        unsafe {
            match self.original.as_deref() {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

#[test]
fn session_status_uses_phase_when_no_result() {
    assert_eq!(
        status_from_phase_and_result(&SessionPhase::Active, None),
        "Active"
    );
    assert_eq!(
        status_from_phase_and_result(&SessionPhase::Available, None),
        "Available"
    );
}

#[test]
fn session_status_marks_non_zero_as_failed() {
    let failure = make_result("failure", 1);
    let signal = make_result("signal", 137);
    let inconsistent_success = make_result("success", 2);

    assert_eq!(
        status_from_phase_and_result(&SessionPhase::Active, Some(&failure)),
        "Failed"
    );
    assert_eq!(
        status_from_phase_and_result(&SessionPhase::Active, Some(&signal)),
        "Failed"
    );
    assert_eq!(
        status_from_phase_and_result(&SessionPhase::Active, Some(&inconsistent_success)),
        "Failed"
    );
}

#[test]
fn session_status_marks_unknown_result_as_error() {
    let unknown = make_result("mystery", 0);
    assert_eq!(
        status_from_phase_and_result(&SessionPhase::Active, Some(&unknown)),
        "Error"
    );
}

#[test]
fn retired_phase_shows_failure_when_result_failed() {
    let failure = make_result("failure", 1);
    assert_eq!(
        status_from_phase_and_result(&SessionPhase::Retired, Some(&failure)),
        "Failed"
    );
}

#[test]
fn retired_phase_shows_retired_when_result_succeeded() {
    let success = make_result("success", 0);
    assert_eq!(
        status_from_phase_and_result(&SessionPhase::Retired, Some(&success)),
        "Retired"
    );
}

// #1118 part D ────────────────────────────────────────────────────────────────
//
// Active sessions whose `last_accessed` has not advanced for >= the stale
// threshold are reported as `Stale` so operators can `csa session kill` them.
// The threshold is `2 * kv_cache.long_poll_seconds` (default 480s).

#[test]
fn is_session_stale_returns_false_for_recent_active_session() {
    let now = Utc::now();
    let mut session = sample_session_state();
    session.phase = SessionPhase::Active;
    session.last_accessed = now - chrono::Duration::seconds(60);

    assert!(
        !is_session_stale_for_test(&session, 480, now),
        "session accessed 60s ago should not be stale at threshold 480s",
    );
}

#[test]
fn is_session_stale_returns_true_for_stale_active_session() {
    let now = Utc::now();
    let mut session = sample_session_state();
    session.phase = SessionPhase::Active;
    session.last_accessed = now - chrono::Duration::seconds(1_000);

    assert!(
        is_session_stale_for_test(&session, 480, now),
        "session accessed 1000s ago should be stale at threshold 480s",
    );
}

#[test]
fn is_session_stale_ignores_non_active_phase() {
    let now = Utc::now();
    let mut session = sample_session_state();
    session.last_accessed = now - chrono::Duration::seconds(1_000);

    for phase in [SessionPhase::Available, SessionPhase::Retired] {
        let phase_label = format!("{phase:?}");
        session.phase = phase;
        assert!(
            !is_session_stale_for_test(&session, 480, now),
            "non-Active session should never be reported as stale (phase={phase_label})",
        );
    }
}

#[cfg(unix)]
#[test]
fn resolve_session_status_reports_stale_for_active_session_without_progress() {
    let td = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new_blocking(&td);
    let project = td.path();

    let s = create_session(project, Some("stale-detection"), None, Some("codex")).unwrap();
    let mut session = load_session(project, &s.meta_session_id).unwrap();
    session.phase = SessionPhase::Active;
    // Backdate last_accessed past the worst-case threshold (Max-tier Opus
    // 3000s long-poll → 6000s stale threshold).
    session.last_accessed = Utc::now() - chrono::Duration::seconds(7_200);
    save_session(&session).unwrap();

    // The dead-active reconciler synthesizes a `Failed` result.toml for any
    // Active session whose process is gone (#540), pre-empting the stale path.
    // Spawn a real long-lived child and write its PID into `locks/codex.lock`
    // so liveness checks see the session as alive — only then can stale
    // detection (#1118 part D) surface.
    let session_dir = get_session_dir(project, &s.meta_session_id).unwrap();
    let mut child = std::process::Command::new("sleep")
        .arg("60")
        .spawn()
        .expect("spawn keepalive child");
    let locks_dir = session_dir.join("locks");
    std::fs::create_dir_all(&locks_dir).unwrap();
    std::fs::write(
        locks_dir.join("codex.lock"),
        format!(r#"{{"pid": {}}}"#, child.id()),
    )
    .unwrap();

    let resolved = resolve_session_status(&session);

    child.kill().ok();
    child.wait().ok();

    assert_eq!(
        resolved, "Stale",
        "stale Active session with live process and no progress should resolve to 'Stale'"
    );

    delete_session(project, &s.meta_session_id).unwrap();
}

fn sample_session_state() -> MetaSessionState {
    let now = Utc::now();
    MetaSessionState {
        meta_session_id: "01J6F5W0M6Q7BW7Q3T0J4A8V45".to_string(),
        description: Some("Plan".to_string()),
        project_path: "/tmp/project".to_string(),
        branch: Some("feature/x".to_string()),
        created_at: now,
        last_accessed: now,
        csa_version: Some("0.1.450".to_string()),
        genealogy: Genealogy {
            parent_session_id: None,
            depth: 0,
            ..Default::default()
        },
        tools: HashMap::new(),
        context_status: ContextStatus::default(),
        total_token_usage: Some(TokenUsage {
            input_tokens: Some(10),
            output_tokens: Some(20),
            total_tokens: Some(30),
            estimated_cost_usd: None,
        }),
        phase: SessionPhase::Available,
        task_context: TaskContext {
            task_type: Some("plan".to_string()),
            tier_name: None,
        },
        turn_count: 0,
        token_budget: None,
        sandbox_info: None,
        termination_reason: None,
        is_seed_candidate: false,
        git_head_at_creation: None,
        pre_session_porcelain: None,
        last_return_packet: None,
        change_id: None,
        spec_id: None,
        fork_call_timestamps: Vec::new(),
        vcs_identity: None,
        identity_version: 1,
    }
}

#[test]
fn session_list_branch_filter_returns_matching_sessions() {
    let td = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new_blocking(&td);
    let project = td.path();

    let s1 = create_session(project, Some("S1"), None, None).unwrap();
    let s2 = create_session(project, Some("S2"), None, None).unwrap();

    let mut session1 = load_session(project, &s1.meta_session_id).unwrap();
    session1.branch = Some("feature/x".to_string());
    save_session(&session1).unwrap();

    let mut session2 = load_session(project, &s2.meta_session_id).unwrap();
    session2.branch = Some("feature/y".to_string());
    save_session(&session2).unwrap();

    let filtered = select_sessions_for_list(project, Some("feature/x"), None).unwrap();
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].meta_session_id, s1.meta_session_id);

    delete_session(project, &s1.meta_session_id).unwrap();
    delete_session(project, &s2.meta_session_id).unwrap();
}

#[test]
fn session_list_selection_not_truncated_to_ten() {
    let td = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new_blocking(&td);
    let project = td.path();

    for _ in 0..12 {
        let _ = create_session(project, Some("bulk"), None, None).unwrap();
    }

    let sessions = select_sessions_for_list(project, None, None).unwrap();
    assert_eq!(sessions.len(), 12);
}

#[test]
fn session_list_cli_parses_branch_filter() {
    let cli = Cli::try_parse_from(["csa", "session", "list", "--branch", "feature/x"]).unwrap();
    match cli.command {
        Commands::Session {
            cmd: SessionCommands::List { branch, .. },
        } => assert_eq!(branch.as_deref(), Some("feature/x")),
        _ => panic!("expected session list command"),
    }
}

#[test]
fn session_list_cli_parses_csa_version_filter() {
    let cli = Cli::try_parse_from([
        "csa",
        "session",
        "list",
        "--csa-version",
        "0.1.450",
        "--show-version",
    ])
    .unwrap();
    match cli.command {
        Commands::Session {
            cmd:
                SessionCommands::List {
                    csa_version,
                    show_version,
                    ..
                },
        } => {
            assert_eq!(csa_version.as_deref(), Some("0.1.450"));
            assert!(show_version);
        }
        _ => panic!("expected session list command"),
    }
}

#[test]
fn session_list_cli_help_mentions_tree_filter_incompatibility() {
    let mut list_cmd = Cli::command()
        .find_subcommand("session")
        .expect("session command")
        .find_subcommand("list")
        .expect("session list command")
        .clone();
    let help = list_cmd.render_long_help().to_string();

    assert!(help.contains("incompatible with --limit/--since/--status"));
}

#[test]
fn session_wait_cli_help_mentions_memory_warn_exit_code() {
    let mut wait_cmd = Cli::command()
        .find_subcommand("session")
        .expect("session command")
        .find_subcommand("wait")
        .expect("session wait command")
        .clone();
    let help = wait_cmd.render_long_help().to_string();

    assert!(help.contains("--memory-warn-mb"));
    assert!(help.contains("exits with code 33"));
    assert!(help.contains("CSA:MEMORY_WARN"));
}

#[test]
fn session_list_tree_rejects_limit_flag() {
    let td = tempdir().unwrap();
    let err = handle_session_list(
        Some(td.path().display().to_string()),
        None,
        None,
        true,
        false,
        super::SessionListFilters {
            limit: Some(10),
            since: None,
            status: None,
            csa_version: None,
            show_version: false,
        },
        csa_core::types::OutputFormat::Text,
    )
    .unwrap_err();

    assert!(
        err.to_string().contains("--tree is incompatible"),
        "got: {err}"
    );
}

#[test]
fn session_list_tree_accepts_no_filters() {
    let td = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new_blocking(&td);
    let project = td.path();
    let _session = create_session(project, Some("Tree session"), None, None).unwrap();

    handle_session_list(
        Some(project.display().to_string()),
        None,
        None,
        true,
        false,
        super::SessionListFilters {
            limit: None,
            since: None,
            status: None,
            csa_version: None,
            show_version: false,
        },
        csa_core::types::OutputFormat::Text,
    )
    .expect("tree listing without filters should succeed");
}

#[test]
fn session_list_filter_csa_version_matches() {
    let mut first = sample_session_state();
    first.csa_version = Some("0.1.450".to_string());
    let mut second = sample_session_state();
    second.meta_session_id = "01J6F5W0M6Q7BW7Q3T0J4A8V46".to_string();
    second.csa_version = Some("0.1.451".to_string());

    let filtered = filter_sessions_by_csa_version(vec![first.clone(), second], Some("0.1.450"));
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].meta_session_id, first.meta_session_id);
}

#[test]
fn resolve_session_prefix_falls_back_to_legacy_sessions_dir() {
    let td = tempdir().unwrap();
    let primary_sessions_dir = td.path().join("new").join("sessions");
    let legacy_sessions_dir = td.path().join("legacy").join("sessions");
    std::fs::create_dir_all(&legacy_sessions_dir).unwrap();

    let legacy_id = "01HY7ABCDEFGHIJKLMNOPQRSTU";
    std::fs::create_dir_all(legacy_sessions_dir.join(legacy_id)).unwrap();

    let resolved = resolve_session_prefix_from_dirs(
        "01HY7",
        &primary_sessions_dir,
        Some(&legacy_sessions_dir),
    )
    .unwrap();

    assert_eq!(resolved.session_id, legacy_id);
    assert_eq!(resolved.sessions_dir, legacy_sessions_dir);
}

#[test]
fn resolve_session_prefix_does_not_hide_primary_ambiguity() {
    let td = tempdir().unwrap();
    let primary_sessions_dir = td.path().join("new").join("sessions");
    std::fs::create_dir_all(&primary_sessions_dir).unwrap();
    std::fs::create_dir_all(primary_sessions_dir.join("01HY7AAAAAAAAAAAAAAAAAAAAA")).unwrap();
    std::fs::create_dir_all(primary_sessions_dir.join("01HY7BBBBBBBBBBBBBBBBBBBBBB")).unwrap();

    let legacy_sessions_dir = td.path().join("legacy").join("sessions");
    std::fs::create_dir_all(&legacy_sessions_dir).unwrap();
    std::fs::create_dir_all(legacy_sessions_dir.join("01HY7CCCCCCCCCCCCCCCCCCCCCC")).unwrap();

    let err = resolve_session_prefix_from_dirs(
        "01HY7",
        &primary_sessions_dir,
        Some(&legacy_sessions_dir),
    )
    .unwrap_err();
    assert!(err.to_string().contains("Ambiguous session prefix"));
}

// ── display_log_files tests ───────────────────────────────────────

#[test]
fn display_log_files_returns_false_when_logs_dir_missing() {
    let td = tempdir().unwrap();
    let session_dir = td.path().join("session");
    std::fs::create_dir_all(&session_dir).unwrap();

    let result = display_log_files(&session_dir, "test-session", None).unwrap();
    assert!(!result, "should return false when logs/ dir does not exist");
}

#[test]
fn display_log_files_returns_false_when_all_empty() {
    let td = tempdir().unwrap();
    let session_dir = td.path().join("session");
    let logs_dir = session_dir.join("logs");
    std::fs::create_dir_all(&logs_dir).unwrap();

    // Create empty log files (simulates broken _log_writer)
    std::fs::write(logs_dir.join("run-2026-01-01.log"), "").unwrap();
    std::fs::write(logs_dir.join("run-2026-01-02.log"), "").unwrap();

    let result = display_log_files(&session_dir, "test-session", None).unwrap();
    assert!(
        !result,
        "should return false when all log files are empty (ACP fallback trigger)"
    );
}

#[test]
fn display_log_files_returns_true_when_content_exists() {
    let td = tempdir().unwrap();
    let session_dir = td.path().join("session");
    let logs_dir = session_dir.join("logs");
    std::fs::create_dir_all(&logs_dir).unwrap();

    std::fs::write(logs_dir.join("run-2026-01-01.log"), "some log output\n").unwrap();

    let result = display_log_files(&session_dir, "test-session", None).unwrap();
    assert!(
        result,
        "should return true when at least one log file has content"
    );
}

#[test]
fn display_log_files_returns_false_when_no_log_files() {
    let td = tempdir().unwrap();
    let session_dir = td.path().join("session");
    let logs_dir = session_dir.join("logs");
    std::fs::create_dir_all(&logs_dir).unwrap();

    // Create a non-.log file — should be ignored
    std::fs::write(logs_dir.join("notes.txt"), "not a log").unwrap();

    let result = display_log_files(&session_dir, "test-session", None).unwrap();
    assert!(!result, "should return false when no .log files exist");
}

#[test]
fn display_daemon_spool_logs_returns_true_when_stdout_or_stderr_exist() {
    let td = tempdir().unwrap();
    let session_dir = td.path().join("session");
    std::fs::create_dir_all(&session_dir).unwrap();
    std::fs::write(session_dir.join("stdout.log"), "daemon stdout\n").unwrap();
    std::fs::write(session_dir.join("stderr.log"), "daemon stderr\n").unwrap();

    let displayed = display_daemon_spool_logs(&session_dir, None).unwrap();
    assert!(displayed, "daemon spool logs should count as session logs");
}

#[test]
fn display_daemon_spool_logs_returns_false_when_spools_missing() {
    let td = tempdir().unwrap();
    let session_dir = td.path().join("session");
    std::fs::create_dir_all(&session_dir).unwrap();

    let displayed = display_daemon_spool_logs(&session_dir, None).unwrap();
    assert!(
        !displayed,
        "missing daemon spool logs should not report output"
    );
}

// ── display_acp_events tests ──────────────────────────────────────

#[test]
fn display_acp_events_succeeds_when_jsonl_exists() {
    let td = tempdir().unwrap();
    let session_dir = td.path().join("session");
    let output_dir = session_dir.join("output");
    std::fs::create_dir_all(&output_dir).unwrap();

    let events = r#"{"seq":1,"ts":"2026-01-01T00:00:00Z","type":"prompt_start"}
{"seq":2,"ts":"2026-01-01T00:00:01Z","type":"prompt_end"}
"#;
    std::fs::write(output_dir.join("acp-events.jsonl"), events).unwrap();

    // Should not error
    display_acp_events(&session_dir, "test-session", None, None).unwrap();
}

#[test]
fn display_acp_events_succeeds_with_tail() {
    let td = tempdir().unwrap();
    let session_dir = td.path().join("session");
    let output_dir = session_dir.join("output");
    std::fs::create_dir_all(&output_dir).unwrap();

    let events = r#"{"seq":1,"ts":"2026-01-01T00:00:00Z","type":"a"}
{"seq":2,"ts":"2026-01-01T00:00:01Z","type":"b"}
{"seq":3,"ts":"2026-01-01T00:00:02Z","type":"c"}
"#;
    std::fs::write(output_dir.join("acp-events.jsonl"), events).unwrap();

    // Should not error with tail
    display_acp_events(&session_dir, "test-session", Some(1), None).unwrap();
}

#[test]
fn display_acp_events_handles_missing_file() {
    let td = tempdir().unwrap();
    let session_dir = td.path().join("session");
    std::fs::create_dir_all(&session_dir).unwrap();

    // No output/acp-events.jsonl — should succeed (prints message to stderr)
    display_acp_events(&session_dir, "test-session", None, None).unwrap();
}

// ── CLI --events flag parsing ─────────────────────────────────────

#[test]
fn session_logs_cli_parses_events_flag() {
    let cli = Cli::try_parse_from([
        "csa",
        "session",
        "logs",
        "--session",
        "01ABCDEF",
        "--events",
    ])
    .unwrap();
    match cli.command {
        Commands::Session {
            cmd: SessionCommands::Logs { events, .. },
        } => assert!(events, "events flag should be true"),
        _ => panic!("expected session logs command"),
    }
}

#[test]
fn session_logs_cli_events_defaults_to_false() {
    let cli = Cli::try_parse_from(["csa", "session", "logs", "--session", "01ABCDEF"]).unwrap();
    match cli.command {
        Commands::Session {
            cmd: SessionCommands::Logs { events, .. },
        } => assert!(!events, "events flag should default to false"),
        _ => panic!("expected session logs command"),
    }
}

// ── print_content_with_tail tests ─────────────────────────────────

#[test]
fn print_content_with_tail_no_panic_on_empty() {
    // Should not panic on empty content
    print_content_with_tail("", None);
    print_content_with_tail("", Some(5));
}

#[test]
fn print_content_with_tail_no_panic_on_large_tail() {
    // Tail larger than line count should not panic
    print_content_with_tail("line1\nline2\n", Some(100));
}

// ── CLI --summary/--section/--full flag parsing ───────────────────

include!("session_cmds_tests_fork_tail.rs");

#[path = "session_cmds_tests_daemon_pid_tail.rs"]
mod daemon_pid_tail_tests;
#[path = "session_cmds_tests_list_format.rs"]
mod list_format_tests;
#[path = "session_cmds_tests_result_cli.rs"]
mod result_cli_tests;
#[path = "session_cmds_tests_tail.rs"]
mod tail_tests;
#[path = "session_cmds_tests_tail_recovery.rs"]
mod tail_tests_recovery;
#[path = "session_cmds_tests_tail_wait.rs"]
mod tail_tests_wait;
#[path = "session_cmds_tests_tail_wait_lock.rs"]
mod tail_tests_wait_lock;
