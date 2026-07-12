use super::resume::DEFAULT_PR_BOT_TIMEOUT_SECS;
use super::*;
use crate::cli::{Cli, ReturnTarget, parse_return_to};
use crate::run_cmd_tool_selection::{
    resolve_heterogeneous_candidates, resolve_last_session_selection,
    take_next_runtime_fallback_tool,
};
use chrono::{TimeZone, Utc};
use clap::Parser;
use csa_core::transport_events::SessionEvent;
use csa_core::types::{OutputFormat, ToolName};
use csa_process::ExecutionResult;
use csa_session::{Genealogy, MetaSessionState, SessionPhase, TaskContext};
use std::collections::HashMap;

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
        csa_version: None,
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
        pre_session_porcelain: None,
        last_return_packet: None,
        change_id: None,
        spec_id: None,
        fork_call_timestamps: Vec::new(),
        vcs_identity: None,
        identity_version: 1,
    }
}

fn try_parse_cli(args: &[&str]) -> Result<Cli, clap::Error> {
    Cli::try_parse_from(args)
}

#[test]
fn run_accepts_unknown_codex_model_at_clap_parse() {
    let result = try_parse_cli(&["csa", "run", "--model-spec", "codex/openai/o3/xhigh", "x"]);
    if let Err(err) = result {
        panic!("unknown model should pass through to backend validation: {err}");
    }
}

#[test]
fn run_accepts_valid_codex_model_at_clap_parse() {
    let result = try_parse_cli(&[
        "csa",
        "run",
        "--model-spec",
        "codex/openai/gpt-5.5/xhigh",
        "x",
    ]);
    if let Err(err) = result {
        panic!("should accept valid spec: {err}");
    }
}

#[test]
fn run_parses_allow_user_daemon_ipc_flag() {
    let cli = try_parse_cli(&["csa", "run", "--allow-user-daemon-ipc", "prompt"])
        .expect("run cli should parse");
    match cli.command {
        crate::cli::Commands::Run {
            allow_user_daemon_ipc,
            ..
        } => assert!(allow_user_daemon_ipc),
        _ => panic!("expected run subcommand"),
    }
}

#[test]
fn run_cli_parses_fast_but_more_cost_flag() {
    let cli = try_parse_cli(&["csa", "run", "--fast-but-more-cost", "x"]).unwrap();

    match cli.command {
        crate::cli::Commands::Run {
            fast_but_more_cost, ..
        } => assert!(fast_but_more_cost),
        _ => panic!("expected run command"),
    }
}

#[test]
fn run_cli_parses_build_jobs_flag() {
    let cli = try_parse_cli(&["csa", "run", "--build-jobs", "4", "x"]).unwrap();

    match cli.command {
        crate::cli::Commands::Run { build_jobs, .. } => assert_eq!(build_jobs, Some(4)),
        _ => panic!("expected run command"),
    }
}

#[test]
fn run_cli_parses_resource_override_flags() {
    let cli = try_parse_cli(&[
        "csa",
        "run",
        "--memory-max-mb",
        "6144",
        "--min-free-memory-mb",
        "512",
        "x",
    ])
    .unwrap();

    match cli.command {
        crate::cli::Commands::Run {
            memory_max_mb,
            min_free_memory_mb,
            ..
        } => {
            assert_eq!(memory_max_mb, Some(6144));
            assert_eq!(min_free_memory_mb, Some(512));
        }
        _ => panic!("expected run command"),
    }
}

#[test]
fn run_cli_rejects_memory_override_below_config_minimum() {
    let result = try_parse_cli(&["csa", "run", "--memory-max-mb", "255", "x"]);

    assert!(
        result.is_err(),
        "memory_max_mb below 256 should be rejected"
    );
}

#[test]
fn run_cli_rejects_zero_build_jobs() {
    let result = try_parse_cli(&["csa", "run", "--build-jobs", "0", "x"]);

    assert!(result.is_err(), "build_jobs=0 should be rejected");
}

#[test]
fn run_cli_parses_allow_fallback_flag() {
    let cli = try_parse_cli(&["csa", "run", "--tool", "codex", "--allow-fallback", "x"]).unwrap();

    match cli.command {
        crate::cli::Commands::Run { allow_fallback, .. } => assert!(allow_fallback),
        _ => panic!("expected run command"),
    }
}

#[test]
fn run_cli_parses_require_commit_flag() {
    let cli = try_parse_cli(&["csa", "run", "--require-commit", "--sa-mode", "false", "x"])
        .expect("--require-commit should parse");

    match cli.command {
        crate::cli::Commands::Run { require_commit, .. } => assert!(require_commit),
        _ => panic!("expected run command"),
    }
}

#[test]
fn run_defers_unknown_model_spec_tool_to_command_catalog() {
    let result = try_parse_cli(&["csa", "run", "--model-spec", "unknown-tool/x/y/medium", "x"]);
    assert!(
        result.is_ok(),
        "clap parsing is syntax-only; the immutable command catalog rejects unknown tools"
    );
}

#[test]
fn run_accepts_openai_compat_with_arbitrary_model() {
    let result = try_parse_cli(&[
        "csa",
        "run",
        "--model-spec",
        "openai-compat/local/my-fine-tune/medium",
        "x",
    ]);
    if let Err(err) = result {
        panic!("openai-compat must skip model validation: {err}");
    }
}

#[test]
fn run_accepts_goal_at_clap_parse() {
    let cli = try_parse_cli(&["csa", "run", "--goal", "tests pass", "fix it"]).unwrap();

    match cli.command {
        crate::cli::Commands::Run { goal, .. } => {
            assert_eq!(goal.as_deref(), Some("tests pass"));
        }
        _ => panic!("expected run command"),
    }
}

#[test]
fn run_cli_hint_difficulty_parses() {
    let cli = try_parse_cli(&[
        "csa",
        "run",
        "--tool",
        "claude",
        "--hint-difficulty",
        "quick_question",
        "prompt",
    ])
    .unwrap();
    match cli.command {
        crate::cli::Commands::Run {
            hint_difficulty, ..
        } => assert_eq!(hint_difficulty.as_deref(), Some("quick_question")),
        _ => panic!("expected Run command"),
    }
}

include!("run_cmd_tests_core.rs");
include!("run_cmd_tests_policy.rs");

#[path = "run_cmd_tier_tests.rs"]
mod tier_tests;

#[path = "run_cmd_tests_tail.rs"]
mod tail_tests;

#[path = "run_cmd_tests_no_verify.rs"]
mod no_verify_tests;

#[path = "run_cmd_tests_lefthook.rs"]
mod lefthook_tests;
