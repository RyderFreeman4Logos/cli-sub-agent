use super::{
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
