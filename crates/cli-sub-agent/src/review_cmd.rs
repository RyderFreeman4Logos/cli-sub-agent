use crate::cli::ReviewArgs;
use crate::pipeline::resolve_effective_initial_response_timeout_for_tool;
#[cfg(test)]
use crate::pipeline::resolve_initial_response_timeout_for_tool;
use crate::review_consensus::CLEAN;
#[cfg(test)]
use crate::review_consensus::{consensus_strategy_label, parse_consensus_strategy};
#[cfg(test)]
use crate::review_context::discover_review_context_for_branch;
#[cfg(test)]
use crate::review_context::{ResolvedReviewContext, ResolvedReviewContextKind};
use crate::review_context::{resolve_review_context, validate_review_prompt_file};
#[cfg(test)]
use crate::review_routing::ReviewRoutingMetadata;
use crate::startup_env::StartupSubtreeEnv;
use anyhow::Result;
#[cfg(test)]
use csa_config::GlobalConfig;
#[cfg(test)]
use csa_config::ProjectConfig;
use csa_core::types::ReviewDecision;
use csa_session::state::ReviewSessionMeta;
use tracing::{debug, error, warn};
#[path = "review_cmd_output.rs"]
pub(crate) mod output;
pub(crate) use output::clean_detection::detect_bounded_clean_verdict_token;
use output::{is_worktree_submodule, persist_review_result_exit_code};
#[path = "review_cmd_artifact_consistency.rs"]
mod artifact_consistency;
#[path = "review_cmd_artifact_parse.rs"]
mod artifact_parse;
#[path = "review_cmd_bug_class.rs"]
mod bug_class_pipeline;
#[path = "review_cmd_check_verdict.rs"]
mod check_verdict;
#[path = "review_cmd_chunking.rs"]
mod chunking;
#[path = "review_cmd_depth.rs"]
mod depth;
#[path = "review_cmd_diff_size.rs"]
mod diff_size;
#[path = "review_cmd_dirty_tree.rs"]
mod dirty_tree;
#[path = "review_cmd_execute.rs"]
mod execute;
#[path = "review_cmd_failure_post.rs"]
mod failure_post;
#[path = "review_cmd_findings_toml.rs"]
mod findings_toml;
#[path = "review_cmd_fix.rs"]
mod fix;
#[path = "review_cmd_fix_finding.rs"]
mod fix_finding;
#[path = "review_cmd_flow.rs"]
mod flow;
#[path = "review_cmd_gate.rs"]
mod gate;
#[path = "review_cmd_mempal.rs"]
mod mempal;
#[path = "review_cmd_multi.rs"]
mod multi;
#[path = "review_cmd_multi_repo_write_audit.rs"]
mod multi_repo_write_audit;
#[path = "review_cmd_parent_artifacts.rs"]
mod parent_artifacts;
#[path = "review_cmd_post_review.rs"]
mod post_review;
#[path = "review_cmd_preflight.rs"]
pub(crate) mod preflight;
#[path = "review_cmd_prior_rounds.rs"]
mod prior_rounds;
#[path = "review_cmd_prose_findings.rs"]
mod prose_findings;
#[path = "review_cmd_prose_resolution.rs"]
mod prose_resolution;
#[path = "review_cmd_resolve.rs"]
mod resolve;
#[path = "review_cmd_result.rs"]
mod result_handling;
#[path = "review_convergence/mod.rs"]
mod review_convergence;
#[path = "review_cmd_reviewers.rs"]
mod reviewers;
#[path = "review_cmd_session_fix.rs"]
mod session_fix;
#[path = "review_cmd_subtree_pin.rs"]
mod subtree_pin;
#[path = "review_cmd_tier_candidates.rs"]
mod tier_candidates;
#[path = "review_cmd_tier_gate.rs"]
mod tier_gate;
#[cfg(test)]
pub(crate) use bug_class_pipeline::try_extract_recurring_bug_class_skills;
#[cfg(test)]
use bug_class_pipeline::try_resolve_review_iterations;
use bug_class_pipeline::{maybe_extract_recurring_bug_class_skills, resolve_review_iterations};
#[cfg(test)]
pub(crate) use check_verdict::check_review_verdict_for_target;
#[cfg(test)]
use execute::execute_review;
use execute::{compute_diff_fingerprint, execute_review_with_tier_filter};
#[cfg(test)]
#[rustfmt::skip]
pub(crate) use flow::{ execute_review_for_tests, persist_review_sidecars_if_session_exists, should_run_fix_loop };
#[cfg(test)]
use flow::persist_review_sidecars_if_session_exists_with_diff_size;
#[cfg(not(test))]
use flow::{persist_review_sidecars_if_session_exists_with_diff_size, should_run_fix_loop};
use post_review::{build_post_review_output, emit_post_review_output, review_scope_is_cumulative};
use prior_rounds::load_prior_rounds_section_or_persist_error;
#[cfg(test)]
use resolve::build_review_instruction;
#[cfg(test)]
pub(crate) use resolve::resolve_review_tool;
use resolve::{
    ReviewProjectPromptOptions, build_review_instruction_for_project, derive_scope_for_project,
    resolve_review_effective_tier, resolve_review_model, resolve_review_readonly_configured,
    resolve_review_readonly_project_root, resolve_review_stream_mode, resolve_review_thinking,
    resolve_review_tier_name, review_scope_allows_auto_discovery, verify_review_skill_available,
};
use result_handling::resolve_single_review_result;
#[rustfmt::skip]
use reviewers::resolve_effective_reviewer_selection_for_args;
#[cfg(test)]
#[rustfmt::skip]
pub(crate) use { fix::persist_fix_final_artifacts_for_tests, output::persist_review_verdict_for_tests };

pub(crate) use execute::compute_diff_fingerprint as compute_review_diff_fingerprint;

#[path = "review_cmd_handle.rs"]
mod handle;
pub(crate) use handle::handle_review;

#[cfg(test)]
#[path = "review_cmd_tests_barrel.rs"]
pub(crate) mod tests;
