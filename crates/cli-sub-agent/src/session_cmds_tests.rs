use super::resolve::resolve_session_prefix_from_dirs;
use super::{
    DeadActiveSessionReconciliation, display_acp_events, display_daemon_spool_logs,
    display_log_files, ensure_terminal_result_for_dead_active_session,
    ensure_terminal_result_for_dead_active_session_with_before_write, handle_session_is_alive,
    handle_session_kill, handle_session_wait, print_content_with_tail, select_sessions_for_list,
    session_to_json, status_from_phase_and_result, truncate_with_ellipsis,
};
use crate::cli::{Cli, Commands, SessionCommands};
use crate::session_cmds_daemon::{
    persist_daemon_completion_from_env, seed_daemon_session_env, synthesized_wait_next_step,
};
use crate::test_env_lock::TEST_ENV_LOCK;
use crate::test_session_sandbox::ScopedSessionSandbox;
use chrono::Utc;
use clap::Parser;
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

fn sample_session_state() -> MetaSessionState {
    let now = Utc::now();
    MetaSessionState {
        meta_session_id: "01J6F5W0M6Q7BW7Q3T0J4A8V45".to_string(),
        description: Some("Plan".to_string()),
        project_path: "/tmp/project".to_string(),
        branch: Some("feature/x".to_string()),
        created_at: now,
        last_accessed: now,
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
fn session_to_json_includes_branch_and_task_type() {
    let session = sample_session_state();
    let value = session_to_json(&session);
    assert_eq!(
        value.get("branch").and_then(|v| v.as_str()),
        Some("feature/x")
    );
    assert_eq!(
        value.get("task_type").and_then(|v| v.as_str()),
        Some("plan")
    );
}

#[test]
fn session_list_branch_filter_returns_matching_sessions() {
    let td = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new(&td);
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
    let _sandbox = ScopedSessionSandbox::new(&td);
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

#[test]
fn session_result_cli_parses_summary_flag() {
    let cli = Cli::try_parse_from([
        "csa",
        "session",
        "result",
        "--session",
        "01ABCDEF",
        "--summary",
    ])
    .unwrap();
    match cli.command {
        Commands::Session {
            cmd:
                SessionCommands::Result {
                    summary,
                    section,
                    full,
                    ..
                },
        } => {
            assert!(summary);
            assert!(section.is_none());
            assert!(!full);
        }
        _ => panic!("expected session result command"),
    }
}

#[test]
fn session_result_cli_parses_section_flag() {
    let cli = Cli::try_parse_from([
        "csa",
        "session",
        "result",
        "--session",
        "01ABCDEF",
        "--section",
        "details",
    ])
    .unwrap();
    match cli.command {
        Commands::Session {
            cmd:
                SessionCommands::Result {
                    summary,
                    section,
                    full,
                    ..
                },
        } => {
            assert!(!summary);
            assert_eq!(section.as_deref(), Some("details"));
            assert!(!full);
        }
        _ => panic!("expected session result command"),
    }
}

#[test]
fn session_result_cli_parses_full_flag() {
    let cli = Cli::try_parse_from([
        "csa",
        "session",
        "result",
        "--session",
        "01ABCDEF",
        "--full",
    ])
    .unwrap();
    match cli.command {
        Commands::Session {
            cmd:
                SessionCommands::Result {
                    summary,
                    section,
                    full,
                    ..
                },
        } => {
            assert!(!summary);
            assert!(section.is_none());
            assert!(full);
        }
        _ => panic!("expected session result command"),
    }
}

#[test]
fn session_result_cli_rejects_conflicting_flags() {
    // --summary and --full conflict
    let result = Cli::try_parse_from([
        "csa",
        "session",
        "result",
        "-s",
        "01ABC",
        "--summary",
        "--full",
    ]);
    assert!(result.is_err());

    // --summary and --section conflict
    let result = Cli::try_parse_from([
        "csa",
        "session",
        "result",
        "-s",
        "01ABC",
        "--summary",
        "--section",
        "x",
    ]);
    assert!(result.is_err());

    // --section and --full conflict
    let result = Cli::try_parse_from([
        "csa",
        "session",
        "result",
        "-s",
        "01ABC",
        "--section",
        "x",
        "--full",
    ]);
    assert!(result.is_err());
}

#[test]
fn session_result_cli_defaults_no_structured_flags() {
    let cli = Cli::try_parse_from(["csa", "session", "result", "--session", "01ABCDEF"]).unwrap();
    match cli.command {
        Commands::Session {
            cmd:
                SessionCommands::Result {
                    summary,
                    section,
                    full,
                    json,
                    ..
                },
        } => {
            assert!(!summary);
            assert!(section.is_none());
            assert!(!full);
            assert!(!json);
        }
        _ => panic!("expected session result command"),
    }
}

// ── format_file_size tests ────────────────────────────────────────

use super::format_file_size;

#[test]
fn format_file_size_covers_ranges() {
    assert_eq!(format_file_size(0), "0 B");
    assert_eq!(format_file_size(512), "512 B");
    assert_eq!(format_file_size(1024), "1.0 KB");
    assert_eq!(format_file_size(1536), "1.5 KB");
    assert_eq!(format_file_size(1048576), "1.0 MB");
}

// ── CLI --measure flag parsing ────────────────────────────────────

#[test]
fn session_measure_cli_parses() {
    let cli = Cli::try_parse_from(["csa", "session", "measure", "--session", "01ABCDEF"]).unwrap();
    match cli.command {
        Commands::Session {
            cmd: SessionCommands::Measure { session, json, .. },
        } => {
            assert_eq!(session, "01ABCDEF");
            assert!(!json);
        }
        _ => panic!("expected session measure command"),
    }
}

#[test]
fn session_measure_cli_parses_json_flag() {
    let cli = Cli::try_parse_from([
        "csa",
        "session",
        "measure",
        "--session",
        "01ABCDEF",
        "--json",
    ])
    .unwrap();
    match cli.command {
        Commands::Session {
            cmd: SessionCommands::Measure { json, .. },
        } => {
            assert!(json);
        }
        _ => panic!("expected session measure command"),
    }
}

// ── Fork display tests ────────────────────────────────────────────

fn sample_fork_session() -> MetaSessionState {
    let now = Utc::now();
    MetaSessionState {
        meta_session_id: "01KJ5CFQYE1AAAABBBBCCCCDD".to_string(),
        description: Some("Forked session".to_string()),
        project_path: "/tmp/project".to_string(),
        branch: Some("feat/fork".to_string()),
        created_at: now,
        last_accessed: now,
        genealogy: Genealogy {
            parent_session_id: Some("01KJ5AFQYE9AAAABBBBCCCCDD".to_string()),
            depth: 1,
            fork_of_session_id: Some("01KJ5AFQYE9AAAABBBBCCCCDD".to_string()),
            fork_provider_session_id: Some("provider-session-xyz".to_string()),
        },
        tools: HashMap::new(),
        context_status: ContextStatus::default(),
        total_token_usage: None,
        phase: SessionPhase::Active,
        task_context: TaskContext {
            task_type: Some("run".to_string()),
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
fn session_to_json_includes_fork_fields() {
    let session = sample_fork_session();
    let value = session_to_json(&session);

    assert_eq!(value.get("is_fork").and_then(|v| v.as_bool()), Some(true));
    assert_eq!(
        value.get("fork_of_session_id").and_then(|v| v.as_str()),
        Some("01KJ5AFQYE9AAAABBBBCCCCDD")
    );
    assert_eq!(
        value
            .get("fork_provider_session_id")
            .and_then(|v| v.as_str()),
        Some("provider-session-xyz")
    );
    assert_eq!(
        value.get("parent_session_id").and_then(|v| v.as_str()),
        Some("01KJ5AFQYE9AAAABBBBCCCCDD")
    );
    assert_eq!(value.get("depth").and_then(|v| v.as_u64()), Some(1));
}

#[test]
fn session_to_json_non_fork_has_is_fork_false() {
    let session = sample_session_state();
    let value = session_to_json(&session);

    assert_eq!(value.get("is_fork").and_then(|v| v.as_bool()), Some(false));
    assert!(value.get("fork_of_session_id").is_none());
    assert!(value.get("fork_provider_session_id").is_none());
}

#[test]
fn session_to_json_includes_depth_and_parent() {
    let mut session = sample_session_state();
    session.genealogy.parent_session_id = Some("01PARENT000000000000000000".to_string());
    session.genealogy.depth = 2;

    let value = session_to_json(&session);
    assert_eq!(
        value.get("parent_session_id").and_then(|v| v.as_str()),
        Some("01PARENT000000000000000000")
    );
    assert_eq!(value.get("depth").and_then(|v| v.as_u64()), Some(2));
}

#[path = "session_cmds_tests_daemon_pid_tail.rs"]
mod daemon_pid_tail_tests;
#[path = "session_cmds_tests_tail.rs"]
mod tail_tests;
#[path = "session_cmds_tests_tail_recovery.rs"]
mod tail_tests_recovery;
#[path = "session_cmds_tests_tail_wait.rs"]
mod tail_tests_wait;
