use super::resume::DEFAULT_PR_BOT_TIMEOUT_SECS;
use super::*;
use crate::cli::{Cli, ReturnTarget, parse_return_to};
use crate::run_cmd_tool_selection::{
    resolve_heterogeneous_candidates, resolve_last_session_selection,
    take_next_runtime_fallback_tool,
};
use chrono::{TimeZone, Utc};
use clap::Parser;
use csa_acp::SessionEvent;
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

include!("run_cmd_tests_core.rs");
include!("run_cmd_tests_policy.rs");

#[path = "run_cmd_tier_tests.rs"]
mod tier_tests;

#[path = "run_cmd_tests_tail.rs"]
mod tail_tests;

#[path = "run_cmd_tests_lefthook.rs"]
mod lefthook_tests;
