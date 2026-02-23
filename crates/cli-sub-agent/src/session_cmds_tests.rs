use super::{
    display_acp_events, display_log_files, print_content_with_tail,
    resolve_session_prefix_from_dirs, select_sessions_for_list, session_to_json,
    status_from_phase_and_result, truncate_with_ellipsis,
};
use crate::cli::{Cli, Commands, SessionCommands};
use chrono::Utc;
use clap::Parser;
use csa_session::{
    ContextStatus, Genealogy, MetaSessionState, SessionPhase, SessionResult, TaskContext,
    TokenUsage, create_session, delete_session, load_session, save_session,
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
fn retired_phase_takes_precedence_over_failure_result() {
    let failure = make_result("failure", 1);
    assert_eq!(
        status_from_phase_and_result(&SessionPhase::Retired, Some(&failure)),
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
    }
}

#[test]
fn session_to_json_includes_branch_and_task_type() {
    let session = sample_session_state();
    let value = session_to_json(std::path::Path::new("/tmp/project"), &session);
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
    display_acp_events(&session_dir, "test-session", None).unwrap();
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
    display_acp_events(&session_dir, "test-session", Some(1)).unwrap();
}

#[test]
fn display_acp_events_handles_missing_file() {
    let td = tempdir().unwrap();
    let session_dir = td.path().join("session");
    std::fs::create_dir_all(&session_dir).unwrap();

    // No output/acp-events.jsonl — should succeed (prints message to stderr)
    display_acp_events(&session_dir, "test-session", None).unwrap();
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

// ── display_structured_output tests ───────────────────────────────

use super::{
    compute_token_measurement, display_all_sections, display_single_section,
    display_summary_section, format_file_size, format_number,
};

#[test]
fn display_summary_section_with_structured_output() {
    let tmp = tempdir().unwrap();
    let output =
        "<!-- CSA:SECTION:summary -->\nThis is the summary.\n<!-- CSA:SECTION:summary:END -->";
    csa_session::persist_structured_output(tmp.path(), output).unwrap();

    // Should succeed without error
    display_summary_section(tmp.path(), "test", false).unwrap();
}

#[test]
fn display_summary_section_falls_back_to_output_log() {
    let tmp = tempdir().unwrap();
    let session_dir = tmp.path();
    // Write output.log without structured markers
    std::fs::write(session_dir.join("output.log"), "Line 1\nLine 2\nLine 3\n").unwrap();

    // Should succeed (falls back to output.log)
    display_summary_section(session_dir, "test", false).unwrap();
}

#[test]
fn display_summary_section_handles_no_output() {
    let tmp = tempdir().unwrap();
    // No output.log, no index.toml — should print message to stderr
    display_summary_section(tmp.path(), "test", false).unwrap();
}

#[test]
fn display_single_section_returns_content() {
    let tmp = tempdir().unwrap();
    let output = "<!-- CSA:SECTION:details -->\nDetail content\n<!-- CSA:SECTION:details:END -->";
    csa_session::persist_structured_output(tmp.path(), output).unwrap();

    display_single_section(tmp.path(), "test", "details", false).unwrap();
}

#[test]
fn display_single_section_errors_on_missing_id() {
    let tmp = tempdir().unwrap();
    let output = "<!-- CSA:SECTION:summary -->\nContent\n<!-- CSA:SECTION:summary:END -->";
    csa_session::persist_structured_output(tmp.path(), output).unwrap();

    let err = display_single_section(tmp.path(), "test", "nonexistent", false).unwrap_err();
    assert!(err.to_string().contains("not found"));
    assert!(err.to_string().contains("summary")); // lists available sections
}

#[test]
fn display_single_section_errors_when_no_structured_output() {
    let tmp = tempdir().unwrap();
    let err = display_single_section(tmp.path(), "test", "any", false).unwrap_err();
    assert!(err.to_string().contains("No structured output"));
}

#[test]
fn display_all_sections_shows_all_in_order() {
    let tmp = tempdir().unwrap();
    let output = "<!-- CSA:SECTION:intro -->\nIntro\n<!-- CSA:SECTION:intro:END -->\n\
                   <!-- CSA:SECTION:body -->\nBody\n<!-- CSA:SECTION:body:END -->";
    csa_session::persist_structured_output(tmp.path(), output).unwrap();

    display_all_sections(tmp.path(), "test", false).unwrap();
}

#[test]
fn display_all_sections_falls_back_to_output_log() {
    let tmp = tempdir().unwrap();
    let session_dir = tmp.path();
    std::fs::write(session_dir.join("output.log"), "raw output here\n").unwrap();

    display_all_sections(session_dir, "test", false).unwrap();
}

#[test]
fn format_file_size_covers_ranges() {
    assert_eq!(format_file_size(0), "0 B");
    assert_eq!(format_file_size(512), "512 B");
    assert_eq!(format_file_size(1024), "1.0 KB");
    assert_eq!(format_file_size(1536), "1.5 KB");
    assert_eq!(format_file_size(1048576), "1.0 MB");
}

// ── format_number tests ───────────────────────────────────────────

#[test]
fn format_number_small_values() {
    assert_eq!(format_number(0), "0");
    assert_eq!(format_number(42), "42");
    assert_eq!(format_number(999), "999");
}

#[test]
fn format_number_with_commas() {
    assert_eq!(format_number(1000), "1,000");
    assert_eq!(format_number(3456), "3,456");
    assert_eq!(format_number(1234567), "1,234,567");
}

// ── compute_token_measurement tests ───────────────────────────────

#[test]
fn measure_structured_output_with_summary() {
    let tmp = tempdir().unwrap();
    let output = "<!-- CSA:SECTION:summary -->\n\
                   Summary line one.\n\
                   Summary line two.\n\
                   <!-- CSA:SECTION:summary:END -->\n\
                   <!-- CSA:SECTION:analysis -->\n\
                   Analysis paragraph one with many words to increase token count.\n\
                   Analysis paragraph two with additional detail and explanation.\n\
                   <!-- CSA:SECTION:analysis:END -->\n\
                   <!-- CSA:SECTION:details -->\n\
                   Detailed implementation notes with code examples and references.\n\
                   More detail lines for testing purposes.\n\
                   <!-- CSA:SECTION:details:END -->\n\
                   <!-- CSA:SECTION:implementation -->\n\
                   Implementation code and final notes.\n\
                   <!-- CSA:SECTION:implementation:END -->";
    csa_session::persist_structured_output(tmp.path(), output).unwrap();

    let m = compute_token_measurement(tmp.path(), "01TEST123").unwrap();
    assert!(m.is_structured);
    assert_eq!(m.section_count, 4);
    assert_eq!(
        m.section_names,
        vec!["summary", "analysis", "details", "implementation"]
    );
    assert!(m.summary_tokens > 0);
    assert!(m.total_tokens > m.summary_tokens);
    assert!(m.savings_percent > 0.0);
    assert_eq!(m.savings_tokens, m.total_tokens - m.summary_tokens);
}

#[test]
fn measure_unstructured_output_no_savings() {
    let tmp = tempdir().unwrap();
    let output = "Plain text without any markers.\nSecond line.\nThird line.";
    csa_session::persist_structured_output(tmp.path(), output).unwrap();

    let m = compute_token_measurement(tmp.path(), "01TEST456").unwrap();
    assert!(!m.is_structured);
    assert_eq!(m.section_count, 1);
    assert_eq!(m.section_names, vec!["full"]);
    // For unstructured, summary_tokens = first section = total
    assert_eq!(m.summary_tokens, m.total_tokens);
    assert_eq!(m.savings_tokens, 0);
    assert_eq!(m.savings_percent, 0.0);
}

#[test]
fn measure_empty_output() {
    let tmp = tempdir().unwrap();
    csa_session::persist_structured_output(tmp.path(), "").unwrap();

    let m = compute_token_measurement(tmp.path(), "01EMPTY").unwrap();
    assert!(!m.is_structured);
    assert_eq!(m.total_tokens, 0);
    assert_eq!(m.summary_tokens, 0);
    assert_eq!(m.savings_tokens, 0);
    assert_eq!(m.savings_percent, 0.0);
}

#[test]
fn measure_no_index_falls_back_to_output_log() {
    let tmp = tempdir().unwrap();
    let session_dir = tmp.path();
    std::fs::write(
        session_dir.join("output.log"),
        "Some raw output content here.\n",
    )
    .unwrap();

    let m = compute_token_measurement(session_dir, "01NOINDEX").unwrap();
    assert!(!m.is_structured);
    assert!(m.total_tokens > 0);
    assert_eq!(m.summary_tokens, m.total_tokens);
    assert_eq!(m.savings_tokens, 0);
    assert!(m.section_names.is_empty());
}

#[test]
fn measure_no_output_at_all() {
    let tmp = tempdir().unwrap();
    let m = compute_token_measurement(tmp.path(), "01NOTHING").unwrap();
    assert!(!m.is_structured);
    assert_eq!(m.total_tokens, 0);
    assert_eq!(m.savings_tokens, 0);
}

#[test]
fn measure_single_named_section_is_structured() {
    let tmp = tempdir().unwrap();
    let output =
        "<!-- CSA:SECTION:report -->\nReport content here.\n<!-- CSA:SECTION:report:END -->";
    csa_session::persist_structured_output(tmp.path(), output).unwrap();

    let m = compute_token_measurement(tmp.path(), "01SINGLE").unwrap();
    // Single section that is NOT "full" counts as structured
    assert!(m.is_structured);
    assert_eq!(m.section_count, 1);
    assert_eq!(m.section_names, vec!["report"]);
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
    }
}

#[test]
fn session_to_json_includes_fork_fields() {
    let session = sample_fork_session();
    let value = session_to_json(std::path::Path::new("/tmp/project"), &session);

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
    let value = session_to_json(std::path::Path::new("/tmp/project"), &session);

    assert_eq!(value.get("is_fork").and_then(|v| v.as_bool()), Some(false));
    assert!(value.get("fork_of_session_id").is_none());
    assert!(value.get("fork_provider_session_id").is_none());
}

#[test]
fn session_to_json_includes_depth_and_parent() {
    let mut session = sample_session_state();
    session.genealogy.parent_session_id = Some("01PARENT000000000000000000".to_string());
    session.genealogy.depth = 2;

    let value = session_to_json(std::path::Path::new("/tmp/project"), &session);
    assert_eq!(
        value.get("parent_session_id").and_then(|v| v.as_str()),
        Some("01PARENT000000000000000000")
    );
    assert_eq!(value.get("depth").and_then(|v| v.as_u64()), Some(2));
}
