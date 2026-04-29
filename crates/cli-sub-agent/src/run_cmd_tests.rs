use super::resume::DEFAULT_PR_BOT_TIMEOUT_SECS;
use super::*;
use crate::cli::{Cli, ReturnTarget, parse_return_to};
use crate::run_cmd_tool_selection::{
    resolve_heterogeneous_candidates, resolve_last_session_selection,
    take_next_runtime_fallback_tool,
};
use chrono::{TimeZone, Utc};
use clap::Parser;
use csa_core::transport_events::{SessionEvent, StreamingMetadata};
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
fn run_rejects_unknown_codex_model_at_clap_parse() {
    let result = try_parse_cli(&["csa", "run", "--model-spec", "codex/openai/o3/xhigh", "x"]);
    let err = match result {
        Ok(_) => panic!("unknown model should fail clap parsing"),
        Err(err) => err,
    };
    let msg = err.to_string();
    assert!(msg.contains("o3"), "missing offending model: {msg}");
    assert!(msg.contains("gpt-5.5"), "missing valid alternative: {msg}");
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
fn run_rejects_unknown_tool_at_clap_parse() {
    let result = try_parse_cli(&["csa", "run", "--model-spec", "unknown-tool/x/y/medium", "x"]);
    let err = match result {
        Ok(_) => panic!("unknown tool should fail clap parsing"),
        Err(err) => err,
    };
    let msg = err.to_string();
    assert!(msg.contains("unknown-tool"), "{msg}");
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

#[path = "run_cmd_tests_lefthook.rs"]
mod lefthook_tests;
