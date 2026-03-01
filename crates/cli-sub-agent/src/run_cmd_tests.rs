use super::*;
use chrono::{TimeZone, Utc};
use clap::Parser;
use csa_core::types::ToolName;
use csa_session::{Genealogy, MetaSessionState, SessionPhase, TaskContext};
use std::collections::HashMap;

use crate::cli::{Cli, ReturnTarget, parse_return_to};
use crate::run_cmd_tool_selection::{
    resolve_heterogeneous_candidates, resolve_last_session_selection,
    take_next_runtime_fallback_tool,
};

fn test_session(
    meta_session_id: &str,
    last_accessed: chrono::DateTime<Utc>,
    phase: SessionPhase,
) -> MetaSessionState {
    MetaSessionState {
        meta_session_id: meta_session_id.to_string(),
        description: None,
        project_path: "/tmp/project".to_string(),
        branch: None,
        created_at: last_accessed,
        last_accessed,
        genealogy: Genealogy {
            parent_session_id: None,
            depth: 0,
            ..Default::default()
        },
        tools: HashMap::new(),
        context_status: Default::default(),
        total_token_usage: None,
        phase,
        task_context: TaskContext::default(),
        turn_count: 0,
        token_budget: None,
        sandbox_info: None,

        termination_reason: None,
        is_seed_candidate: false,
        git_head_at_creation: None,
        last_return_packet: None,
        fork_call_timestamps: Vec::new(),
    }
}

#[test]
fn test_resolve_last_session_selection_errors_on_empty_sessions() {
    let err = resolve_last_session_selection(Vec::new()).unwrap_err();
    assert!(
        err.to_string()
            .contains("No sessions found. Run a task first to create one.")
    );
}

#[test]
fn test_resolve_last_session_selection_warns_when_multiple_active_sessions_exist() {
    let latest = Utc
        .with_ymd_and_hms(2026, 2, 15, 10, 30, 0)
        .single()
        .unwrap();
    let older = Utc.with_ymd_and_hms(2026, 2, 15, 9, 0, 0).single().unwrap();
    let available = Utc.with_ymd_and_hms(2026, 2, 14, 8, 0, 0).single().unwrap();

    let sessions = vec![
        test_session("01ARZ3NDEKTSV4RRFFQ69G5FAV", older, SessionPhase::Active),
        test_session("01ARZ3NDEKTSV4RRFFQ69G5FAW", latest, SessionPhase::Active),
        test_session(
            "01ARZ3NDEKTSV4RRFFQ69G5FAX",
            available,
            SessionPhase::Available,
        ),
    ];

    let (selected_id, warning) = resolve_last_session_selection(sessions).unwrap();
    assert_eq!(selected_id, "01ARZ3NDEKTSV4RRFFQ69G5FAW");

    let warning = warning.expect("warning should be present");
    assert!(warning.contains("`--last` is ambiguous"));
    assert!(warning.contains("01ARZ3NDEKTSV4RRFFQ69G5FAV"));
    assert!(warning.contains("01ARZ3NDEKTSV4RRFFQ69G5FAW"));
    assert!(warning.contains(&latest.to_rfc3339()));
    assert!(warning.contains(&older.to_rfc3339()));
    assert!(warning.contains("--session <session-id>"));
}

#[test]
fn test_resolve_last_session_selection_has_no_warning_with_single_active_session() {
    let latest = Utc
        .with_ymd_and_hms(2026, 2, 15, 11, 0, 0)
        .single()
        .unwrap();
    let older = Utc.with_ymd_and_hms(2026, 2, 15, 9, 0, 0).single().unwrap();

    let sessions = vec![
        test_session("01ARZ3NDEKTSV4RRFFQ69G5FAV", older, SessionPhase::Active),
        test_session(
            "01ARZ3NDEKTSV4RRFFQ69G5FAW",
            latest,
            SessionPhase::Available,
        ),
    ];

    let (selected_id, warning) = resolve_last_session_selection(sessions).unwrap();
    assert_eq!(selected_id, "01ARZ3NDEKTSV4RRFFQ69G5FAW");
    assert!(warning.is_none());
}

#[test]
fn test_resolve_heterogeneous_candidates_preserves_order() {
    let enabled = vec![
        ToolName::GeminiCli,
        ToolName::Opencode,
        ToolName::Codex,
        ToolName::ClaudeCode,
    ];
    let candidates = resolve_heterogeneous_candidates(&ToolName::ClaudeCode, &enabled);
    assert_eq!(
        candidates,
        vec![ToolName::GeminiCli, ToolName::Opencode, ToolName::Codex]
    );
}

#[test]
fn test_take_next_runtime_fallback_tool_skips_current_and_tried() {
    let mut candidates = vec![ToolName::GeminiCli, ToolName::Codex, ToolName::Opencode];
    let tried_tools = vec!["gemini-cli".to_string()];

    let selected =
        take_next_runtime_fallback_tool(&mut candidates, ToolName::GeminiCli, &tried_tools)
            .expect("expected a fallback tool");

    assert_eq!(selected, ToolName::Codex);
    assert_eq!(candidates, vec![ToolName::Opencode]);
}

// ── CLI flag parsing tests for fork-first architecture ──────────────

fn try_parse_cli(args: &[&str]) -> Result<Cli, clap::Error> {
    Cli::try_parse_from(args)
}

#[test]
fn test_cli_fork_from_parses_ulid() {
    let cli = try_parse_cli(&["csa", "run", "--fork-from", "01ABC", "do stuff"]).unwrap();
    match cli.command {
        crate::cli::Commands::Run { fork_from, .. } => {
            assert_eq!(fork_from, Some("01ABC".to_string()));
        }
        _ => panic!("expected Run command"),
    }
}

#[test]
fn test_cli_fork_last_parses() {
    let cli = try_parse_cli(&["csa", "run", "--fork-last", "do stuff"]).unwrap();
    match cli.command {
        crate::cli::Commands::Run { fork_last, .. } => {
            assert!(fork_last);
        }
        _ => panic!("expected Run command"),
    }
}

#[test]
fn test_cli_fork_from_conflicts_with_session() {
    let result = try_parse_cli(&[
        "csa",
        "run",
        "--fork-from",
        "01ABC",
        "--session",
        "01DEF",
        "prompt",
    ]);
    assert!(result.is_err(), "fork-from and session should conflict");
}

#[test]
fn test_cli_fork_from_conflicts_with_last() {
    let result = try_parse_cli(&["csa", "run", "--fork-from", "01ABC", "--last", "prompt"]);
    assert!(result.is_err(), "fork-from and last should conflict");
}

#[test]
fn test_cli_fork_last_conflicts_with_session() {
    let result = try_parse_cli(&["csa", "run", "--fork-last", "--session", "01DEF", "prompt"]);
    assert!(result.is_err(), "fork-last and session should conflict");
}

#[test]
fn test_cli_fork_last_conflicts_with_last() {
    let result = try_parse_cli(&["csa", "run", "--fork-last", "--last", "prompt"]);
    assert!(result.is_err(), "fork-last and last should conflict");
}

#[test]
fn test_cli_fork_from_conflicts_with_fork_last() {
    let result = try_parse_cli(&[
        "csa",
        "run",
        "--fork-from",
        "01ABC",
        "--fork-last",
        "prompt",
    ]);
    assert!(result.is_err(), "fork-from and fork-last should conflict");
}

#[test]
fn test_cli_fork_from_conflicts_with_ephemeral() {
    let result = try_parse_cli(&[
        "csa",
        "run",
        "--fork-from",
        "01ABC",
        "--ephemeral",
        "prompt",
    ]);
    assert!(result.is_err(), "fork-from and ephemeral should conflict");
}

#[test]
fn test_cli_fork_last_conflicts_with_ephemeral() {
    let result = try_parse_cli(&["csa", "run", "--fork-last", "--ephemeral", "prompt"]);
    assert!(result.is_err(), "fork-last and ephemeral should conflict");
}

#[test]
fn test_cli_legacy_session_still_works() {
    let cli = try_parse_cli(&["csa", "run", "--session", "01ABC", "prompt"]).unwrap();
    match cli.command {
        crate::cli::Commands::Run { session, .. } => {
            assert_eq!(session, Some("01ABC".to_string()));
        }
        _ => panic!("expected Run command"),
    }
}

#[test]
fn test_cli_legacy_last_still_works() {
    let cli = try_parse_cli(&["csa", "run", "--last", "prompt"]).unwrap();
    match cli.command {
        crate::cli::Commands::Run { last, .. } => {
            assert!(last);
        }
        _ => panic!("expected Run command"),
    }
}

#[test]
fn test_cli_no_memory_flag_parses() {
    let cli = try_parse_cli(&["csa", "run", "--no-memory", "prompt"]).unwrap();
    match cli.command {
        crate::cli::Commands::Run { no_memory, .. } => {
            assert!(no_memory);
        }
        _ => panic!("expected Run command"),
    }
}

#[test]
fn test_cli_memory_query_flag_parses() {
    let cli = try_parse_cli(&["csa", "run", "--memory-query", "custom", "prompt"]).unwrap();
    match cli.command {
        crate::cli::Commands::Run { memory_query, .. } => {
            assert_eq!(memory_query.as_deref(), Some("custom"));
        }
        _ => panic!("expected Run command"),
    }
}

#[test]
fn test_cli_fork_call_parses_without_return_to() {
    let cli = try_parse_cli(&["csa", "run", "--fork-call", "task"]).unwrap();
    match cli.command {
        crate::cli::Commands::Run {
            fork_call,
            return_to,
            ..
        } => {
            assert!(fork_call);
            let parsed = return_to
                .as_deref()
                .map(parse_return_to)
                .transpose()
                .unwrap()
                .unwrap_or(ReturnTarget::Auto);
            assert_eq!(parsed, ReturnTarget::Auto);
        }
        _ => panic!("expected Run command"),
    }
}

#[test]
fn test_cli_fork_call_return_to_last_parses() {
    let cli = try_parse_cli(&["csa", "run", "--fork-call", "--return-to", "last", "task"]).unwrap();
    match cli.command {
        crate::cli::Commands::Run { return_to, .. } => {
            assert_eq!(return_to.as_deref(), Some("last"));
            assert_eq!(
                parse_return_to(return_to.as_deref().unwrap()).unwrap(),
                ReturnTarget::Last
            );
        }
        _ => panic!("expected Run command"),
    }
}

#[test]
fn test_cli_fork_call_return_to_auto_parses() {
    let cli = try_parse_cli(&["csa", "run", "--fork-call", "--return-to", "auto", "task"]).unwrap();
    match cli.command {
        crate::cli::Commands::Run { return_to, .. } => {
            assert_eq!(return_to.as_deref(), Some("auto"));
            assert_eq!(
                parse_return_to(return_to.as_deref().unwrap()).unwrap(),
                ReturnTarget::Auto
            );
        }
        _ => panic!("expected Run command"),
    }
}

#[test]
fn test_cli_fork_call_return_to_session_id_parses() {
    let cli = try_parse_cli(&[
        "csa",
        "run",
        "--fork-call",
        "--return-to",
        "01KJXYZ",
        "task",
    ])
    .unwrap();
    match cli.command {
        crate::cli::Commands::Run { return_to, .. } => {
            assert_eq!(return_to.as_deref(), Some("01KJXYZ"));
            assert_eq!(
                parse_return_to(return_to.as_deref().unwrap()).unwrap(),
                ReturnTarget::SessionId("01KJXYZ".to_string())
            );
        }
        _ => panic!("expected Run command"),
    }
}

#[test]
fn test_cli_fork_call_conflicts_with_session() {
    let result = try_parse_cli(&["csa", "run", "--fork-call", "--session", "01KJXYZ", "task"]);
    let err = match result {
        Ok(_) => panic!("fork-call and session should conflict"),
        Err(err) => err,
    };
    assert!(err.to_string().contains("--fork-call"));
    assert!(err.to_string().contains("--session"));
}

#[test]
fn test_cli_fork_call_conflicts_with_last() {
    let result = try_parse_cli(&["csa", "run", "--fork-call", "--last", "task"]);
    let err = match result {
        Ok(_) => panic!("fork-call and last should conflict"),
        Err(err) => err,
    };
    assert!(err.to_string().contains("--fork-call"));
    assert!(err.to_string().contains("--last"));
}

#[test]
fn test_cli_fork_call_conflicts_with_ephemeral() {
    let result = try_parse_cli(&["csa", "run", "--fork-call", "--ephemeral", "task"]);
    let err = match result {
        Ok(_) => panic!("fork-call and ephemeral should conflict"),
        Err(err) => err,
    };
    assert!(err.to_string().contains("--fork-call"));
    assert!(err.to_string().contains("--ephemeral"));
}

#[test]
fn test_cli_return_to_requires_fork_call() {
    let result = try_parse_cli(&["csa", "run", "--return-to", "last", "task"]);
    let err = match result {
        Ok(_) => panic!("return-to should require fork-call"),
        Err(err) => err,
    };
    assert!(err.to_string().contains("--return-to"));
    assert!(err.to_string().contains("--fork-call"));
}

#[test]
fn signal_interruption_exit_code_detects_sigterm_from_error_chain() {
    let err = anyhow::anyhow!("transport failure")
        .context("Failed to execute tool via transport")
        .context("Execution interrupted by SIGTERM");
    assert_eq!(signal_interruption_exit_code(&err), Some(143));
}

#[test]
fn signal_interruption_exit_code_detects_sigint_from_error_chain() {
    let err = anyhow::anyhow!("Execution interrupted by SIGINT");
    assert_eq!(signal_interruption_exit_code(&err), Some(130));
}

#[test]
fn extract_meta_session_id_from_error_reads_context_marker() {
    let err = anyhow::anyhow!("Execution interrupted by SIGTERM")
        .context("meta_session_id=01KJTESTSIGTERMABCDE12345");
    assert_eq!(
        extract_meta_session_id_from_error(&err).as_deref(),
        Some("01KJTESTSIGTERMABCDE12345")
    );
}

#[test]
fn extract_meta_session_id_from_error_returns_none_without_marker() {
    let err = anyhow::anyhow!("Execution interrupted by SIGTERM");
    assert_eq!(extract_meta_session_id_from_error(&err), None);
}

#[test]
fn build_resume_hint_command_includes_skill_when_present() {
    let command = build_resume_hint_command(
        "01KJTESTSIGTERMABCDE12345",
        ToolName::Codex,
        Some("pr-codex-bot"),
    );
    assert_eq!(
        command,
        "csa run --session 01KJTESTSIGTERMABCDE12345 --tool codex --skill pr-codex-bot"
    );
}

#[test]
fn skill_session_description_uses_stable_prefix() {
    assert_eq!(skill_session_description("dev2merge"), "skill:dev2merge");
}

#[test]
fn session_matches_interrupted_skill_requires_signal_and_skill_tag() {
    let mut session = test_session(
        "01KJTESTSIGTERMABCDE12345",
        Utc.with_ymd_and_hms(2026, 3, 1, 13, 10, 0)
            .single()
            .unwrap(),
        SessionPhase::Active,
    );
    session.description = Some(skill_session_description("dev2merge"));
    session.termination_reason = Some("sigterm".to_string());
    assert!(session_matches_interrupted_skill(&session, "dev2merge"));

    session.termination_reason = None;
    assert!(!session_matches_interrupted_skill(&session, "dev2merge"));

    session.termination_reason = Some("sigterm".to_string());
    session.description = Some(skill_session_description("mktd"));
    assert!(!session_matches_interrupted_skill(&session, "dev2merge"));
}
