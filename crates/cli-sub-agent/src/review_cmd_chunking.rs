use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use anyhow::{Context, Result};
use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::env::{CSA_PARENT_SESSION_DIR_ENV_KEY, CSA_SESSION_DIR_ENV_KEY};
use csa_core::types::ToolName;
use csa_session::ReviewDiffSize;
use csa_session::review_artifact::ReviewArtifact;
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tracing::{info, warn};

use crate::cli::{ReviewArgs, ReviewChunkingMode};
use crate::review_consensus::{CLEAN, HAS_ISSUES, UNAVAILABLE, build_consolidated_artifact};
use crate::review_routing::ReviewRoutingMetadata;
use crate::startup_env::StartupSubtreeEnv;

use super::execute::execute_review_with_tier_filter;
use super::output::{ReviewerOutcome, print_reviewer_outcomes};
use super::result_handling::{
    build_reviewer_outcome, build_unavailable_reviewer_outcome, reviewer_unavailable_error_reason,
};

const DEFAULT_ACTIVATE_FILES: usize = 20;
const DEFAULT_ACTIVATE_CHANGED_LINES: usize = 1_000;
const DEFAULT_ACTIVATE_DIFF_BYTES: usize = 80 * 1024;
const DEFAULT_TARGET_FILES_PER_CHUNK: usize = 10;
const DEFAULT_MAX_FILES_PER_CHUNK: usize = 12;
const DEFAULT_TARGET_CHANGED_LINES_PER_CHUNK: usize = 550;
const DEFAULT_MAX_CHANGED_LINES_PER_CHUNK: usize = 700;
const DEFAULT_MAX_CHUNKS: usize = 12;
const DEFAULT_MAX_CONCURRENCY: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ReviewChunkActivationReason {
    Always,
    FileCount,
    ChangedLines,
    DiffBytes,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct ReviewChunkingConfig {
    pub(super) mode: ReviewChunkingMode,
    pub(super) activate_files: usize,
    pub(super) activate_changed_lines: usize,
    pub(super) activate_diff_bytes: usize,
    pub(super) target_files_per_chunk: usize,
    pub(super) max_files_per_chunk: usize,
    pub(super) target_changed_lines_per_chunk: usize,
    pub(super) max_changed_lines_per_chunk: usize,
    pub(super) max_chunks: usize,
    pub(super) max_concurrency: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct ReviewChunkPlan {
    pub(super) scope: String,
    pub(super) activation_reason: ReviewChunkActivationReason,
    pub(super) total_files: usize,
    pub(super) total_changed_lines: usize,
    pub(super) raw_diff_bytes: usize,
    pub(super) chunks: Vec<ReviewChunk>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct ReviewChunk {
    pub(super) id: usize,
    pub(super) group: String,
    pub(super) files: Vec<ReviewChunkFile>,
    pub(super) pathspecs: Vec<String>,
    pub(super) changed_lines: usize,
    pub(super) estimated_tokens: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct ReviewChunkFile {
    pub(super) path: String,
    pub(super) status: String,
    pub(super) changed_lines: usize,
}

pub(super) struct ChunkedReviewContext<'a> {
    pub(super) args: &'a ReviewArgs,
    pub(super) plan: ReviewChunkPlan,
    pub(super) chunking_config: ReviewChunkingConfig,
    pub(super) tool: ToolName,
    pub(super) prompt: &'a str,
    pub(super) scope: &'a str,
    pub(super) project_root: &'a Path,
    pub(super) config: &'a Option<ProjectConfig>,
    pub(super) global_config: &'a GlobalConfig,
    pub(super) model_catalog: &'a csa_config::EffectiveModelCatalog,
    pub(super) pre_session_hook: Option<csa_hooks::PreSessionHookInvocation>,
    pub(super) review_routing: ReviewRoutingMetadata,
    pub(super) diff_size: Option<&'a ReviewDiffSize>,
    pub(super) large_diff_warning: Option<super::diff_size::LargeDiffWarning>,
    pub(super) review_model: Option<String>,
    pub(super) resolved_model_spec: Option<String>,
    pub(super) resolved_tier_name: Option<String>,
    pub(super) tier_active: bool,
    pub(super) tier_preference_order: Vec<String>,
    pub(super) review_thinking: Option<String>,
    pub(super) stream_mode: csa_process::StreamMode,
    pub(super) idle_timeout_seconds: u64,
    pub(super) initial_response_timeout_seconds: Option<u64>,
    pub(super) execution_no_failover: bool,
    pub(super) explicit_tool_with_failover: Option<ToolName>,
    pub(super) readonly_project_root: bool,
    pub(super) allow_user_daemon_ipc: bool,
    pub(super) build_jobs: Option<u32>,
    pub(super) review_mode: &'a str,
    pub(super) current_depth: u32,
    pub(super) startup_env: &'a StartupSubtreeEnv,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ChunkedReviewAudit {
    schema_version: u32,
    scope: String,
    diff_fingerprint: Option<String>,
    activation_reason: ReviewChunkActivationReason,
    final_verdict: String,
    fail_closed: bool,
    chunks: Vec<ChunkAuditEntry>,
    synthesis_session_id: Option<String>,
    timestamp: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ChunkAuditEntry {
    chunk_id: usize,
    group: String,
    pathspecs: Vec<String>,
    changed_lines: usize,
    estimated_tokens: usize,
    session_id: String,
    verdict: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ReviewChunkSessionMeta {
    schema_version: u32,
    chunk_id: usize,
    original_scope: String,
    diff_fingerprint: Option<String>,
    all_changed_files: Vec<String>,
    pathspecs: Vec<String>,
}

impl Default for ReviewChunkingConfig {
    fn default() -> Self {
        Self {
            mode: ReviewChunkingMode::Auto,
            activate_files: DEFAULT_ACTIVATE_FILES,
            activate_changed_lines: DEFAULT_ACTIVATE_CHANGED_LINES,
            activate_diff_bytes: DEFAULT_ACTIVATE_DIFF_BYTES,
            target_files_per_chunk: DEFAULT_TARGET_FILES_PER_CHUNK,
            max_files_per_chunk: DEFAULT_MAX_FILES_PER_CHUNK,
            target_changed_lines_per_chunk: DEFAULT_TARGET_CHANGED_LINES_PER_CHUNK,
            max_changed_lines_per_chunk: DEFAULT_MAX_CHANGED_LINES_PER_CHUNK,
            max_chunks: DEFAULT_MAX_CHUNKS,
            max_concurrency: DEFAULT_MAX_CONCURRENCY,
        }
    }
}

impl ReviewChunkingConfig {
    pub(super) fn for_args(mode: ReviewChunkingMode) -> Self {
        Self {
            mode,
            ..Self::default()
        }
    }

    pub(super) fn concurrency(&self) -> usize {
        self.max_concurrency.max(1)
    }
}

impl ReviewChunkPlan {
    pub(super) fn chunk_count(&self) -> usize {
        self.chunks.len()
    }
}

pub(super) fn should_bypass_chunking(
    mode: ReviewChunkingMode,
    fix: bool,
    session_present: bool,
) -> bool {
    mode == ReviewChunkingMode::Off || fix || session_present
}

pub(super) fn plan_review_chunks(
    project_root: &Path,
    scope: &str,
    diff_size: Option<&ReviewDiffSize>,
    config: &ReviewChunkingConfig,
) -> Result<Option<ReviewChunkPlan>> {
    let Some(reason) = activation_reason(diff_size, config) else {
        return Ok(None);
    };

    let files = collect_review_chunk_files(project_root, scope)
        .with_context(|| format!("failed to collect changed files for review scope {scope}"))?;
    if files.len() <= 1 {
        return Ok(None);
    }

    Ok(Some(plan_review_chunks_from_files(
        scope, diff_size, files, reason, config,
    )))
}

pub(super) async fn run_chunked_review(ctx: ChunkedReviewContext<'_>) -> Result<i32> {
    if ctx.args.fix {
        anyhow::bail!("--fix is not supported by chunked review");
    }
    if ctx.args.session.is_some() {
        anyhow::bail!("--session is not supported by chunked review");
    }

    let parent_startup_env = parent_startup_env_for_chunked_review(
        ctx.args.daemon_child,
        ctx.args.session_id.as_deref(),
        ctx.startup_env,
        ctx.project_root,
    )?;
    let diff_fingerprint = super::execute::compute_diff_fingerprint(ctx.project_root, ctx.scope);
    super::parent_artifacts::clear_multi_reviewer_artifact_dirs(
        ctx.plan.chunk_count().saturating_add(1),
        &parent_startup_env,
    )
    .await?;

    info!(
        chunks = ctx.plan.chunk_count(),
        scope = %ctx.scope,
        "Running chunked review"
    );

    let chunk_tools = vec![ctx.tool; ctx.plan.chunk_count()];
    let mut join_set = JoinSet::new();
    let semaphore = Arc::new(Semaphore::new(ctx.chunking_config.concurrency()));
    for chunk in ctx.plan.chunks.clone() {
        let semaphore = Arc::clone(&semaphore);
        let reviewer_index = chunk.id.saturating_sub(1);
        let chunk_prompt = build_chunk_review_instruction(
            ctx.prompt,
            &ctx.plan,
            &chunk,
            ctx.tool,
            ctx.project_root,
            parent_startup_env.session_id(),
            diff_fingerprint.as_deref(),
        );
        let reviewer_project_root = ctx.project_root.to_path_buf();
        let reviewer_config = ctx.config.as_ref().cloned();
        let reviewer_global = ctx.global_config.clone();
        let reviewer_model_catalog = ctx.model_catalog.clone();
        let reviewer_pre_session_hook = ctx.pre_session_hook.clone();
        let reviewer_routing = ctx.review_routing.clone();
        let reviewer_description = format!(
            "review[chunk-{}]: {}",
            chunk.id,
            crate::run_helpers::truncate_prompt(ctx.scope, 80)
        );
        let reviewer_model = ctx.review_model.clone();
        let reviewer_model_spec = ctx.resolved_model_spec.clone();
        let reviewer_tier_name = ctx.resolved_tier_name.clone();
        let reviewer_tier_active = ctx.tier_active;
        let reviewer_tier_preference_order = ctx.tier_preference_order.clone();
        let reviewer_thinking = ctx.review_thinking.clone();
        let stream_mode = ctx.stream_mode;
        let idle_timeout_seconds = ctx.idle_timeout_seconds;
        let initial_response_timeout_seconds = ctx.initial_response_timeout_seconds;
        let execution_no_failover = ctx.execution_no_failover;
        let explicit_tool_with_failover = ctx.explicit_tool_with_failover;
        let readonly_project_root = ctx.readonly_project_root;
        let allow_user_daemon_ipc = ctx.allow_user_daemon_ipc;
        let build_jobs = ctx.build_jobs;
        let force_override = ctx.args.force_override_user_config;
        let force_ignore_tier = ctx.args.force_ignore_tier_setting;
        let fast_but_more_cost = ctx.args.fast_but_more_cost;
        let no_fs_sandbox = ctx.args.no_fs_sandbox;
        let error_marker_scan_override = ctx.args.error_marker_scan_override();
        let resource_overrides = ctx.args.resource_overrides();
        let extra_writable = ctx.args.extra_writable.clone();
        let extra_readable = ctx.args.extra_readable.clone();
        let startup_env = parent_startup_env.clone();
        let parent_session_dir = parent_startup_env.session_dir().map(PathBuf::from);
        let all_changed_files = all_changed_files(&ctx.plan);
        let original_scope = ctx.scope.to_string();
        let chunk_pathspecs = chunk.pathspecs.clone();
        let chunk_diff_fingerprint = diff_fingerprint.clone();
        let current_depth = ctx.current_depth;
        let tool = ctx.tool;

        join_set.spawn(async move {
            let _permit = semaphore
                .acquire_owned()
                .await
                .context("chunked review concurrency semaphore closed")?;
            let session_result = match execute_review_with_tier_filter(
                tool,
                chunk_prompt,
                None,
                reviewer_model,
                reviewer_model_spec,
                reviewer_tier_name,
                reviewer_tier_active,
                reviewer_tier_preference_order,
                reviewer_thinking,
                reviewer_description,
                &reviewer_project_root,
                reviewer_config.as_ref(),
                &reviewer_global,
                &reviewer_model_catalog,
                reviewer_pre_session_hook,
                reviewer_routing,
                stream_mode,
                idle_timeout_seconds,
                initial_response_timeout_seconds,
                force_override,
                force_ignore_tier,
                execution_no_failover,
                explicit_tool_with_failover,
                build_jobs,
                fast_but_more_cost,
                false,
                no_fs_sandbox,
                allow_user_daemon_ipc,
                readonly_project_root,
                &extra_writable,
                &extra_readable,
                error_marker_scan_override,
                resource_overrides,
                current_depth,
                &startup_env,
            )
            .await
            {
                Ok(session_result) => session_result,
                Err(err) => {
                    if let Some(reason) = reviewer_unavailable_error_reason(&err, tool) {
                        warn!(
                            chunk_id = chunk.id,
                            tool = %tool,
                            reason = %reason,
                            "Chunk reviewer unavailable; chunked review will fail closed"
                        );
                        return Ok(build_unavailable_reviewer_outcome(
                            reviewer_index,
                            tool,
                            reason,
                        ));
                    }
                    return Err(err);
                }
            };
            let outcome = build_reviewer_outcome(reviewer_index, tool, &session_result)?;
            persist_chunk_session_meta(
                &reviewer_project_root,
                parent_session_dir.as_deref(),
                reviewer_index,
                &outcome,
                ReviewChunkSessionMeta {
                    schema_version: 1,
                    chunk_id: chunk.id,
                    original_scope,
                    diff_fingerprint: chunk_diff_fingerprint,
                    all_changed_files,
                    pathspecs: chunk_pathspecs,
                },
            )
            .with_context(|| format!("failed to write chunk {} metadata", chunk.id))?;
            Ok(outcome)
        });
    }

    let mut outcomes =
        super::multi::collect_reviewer_outcomes(&mut join_set, &chunk_tools, ctx.args.timeout)
            .await?;
    let mut repo_write_audit_blocked =
        super::multi_repo_write_audit::apply_repo_write_audit_findings_to_multi_outcomes(
            ctx.project_root,
            &mut outcomes,
        );

    let chunks_all_usable = outcomes
        .iter()
        .all(ReviewerOutcome::produced_usable_verdict);
    if chunks_all_usable {
        let synthesis = run_synthesis_review(
            &ctx,
            &parent_startup_env,
            diff_fingerprint.as_deref(),
            &outcomes,
        )
        .await?;
        outcomes.push(synthesis);
        repo_write_audit_blocked |=
            super::multi_repo_write_audit::apply_repo_write_audit_findings_to_multi_outcomes(
                ctx.project_root,
                &mut outcomes,
            );
    }

    let all_reviewers_unavailable = !outcomes.is_empty()
        && outcomes
            .iter()
            .all(|outcome| outcome.verdict == UNAVAILABLE);
    let mut final_verdict = final_chunked_verdict(&outcomes, all_reviewers_unavailable);
    if repo_write_audit_blocked {
        final_verdict = HAS_ISSUES;
    }

    let review_iterations = outcomes
        .first()
        .map(|outcome| {
            super::bug_class_pipeline::resolve_review_iterations(
                ctx.project_root,
                &outcome.session_id,
            )
        })
        .unwrap_or(1);
    let head_sha = csa_session::detect_git_head(ctx.project_root).unwrap_or_default();
    super::multi::persist_multi_review_sidecars(
        ctx.project_root,
        parent_startup_env.session_dir().map(Path::new),
        ctx.scope,
        &outcomes,
        &head_sha,
        super::multi::ReviewRunMeta {
            review_iterations,
            diff_fingerprint: diff_fingerprint.clone(),
            review_mode: Some(ctx.review_mode),
        },
        super::diff_size::ReviewDiffReport {
            diff_size: ctx.diff_size,
            large_diff_warning: ctx.large_diff_warning,
        },
    );

    let consensus_artifacts = super::parent_artifacts::MultiReviewerConsensusArtifacts {
        project_root: ctx.project_root,
        reviewers: outcomes.len(),
        outcomes: &outcomes,
        final_verdict,
        all_reviewers_unavailable,
        head_sha: &head_sha,
        scope: ctx.scope,
        run_review_mode: Some(ctx.review_mode),
        review_iterations,
        diff_fingerprint: diff_fingerprint.clone(),
        diff_size: ctx.diff_size,
        large_diff_warning: ctx.large_diff_warning,
    };
    if let Err(err) = super::parent_artifacts::write_multi_reviewer_consensus_artifacts(
        consensus_artifacts,
        &parent_startup_env,
    ) {
        warn!(
            error = %err,
            "Failed to write chunked-review consensus artifacts (continuing)"
        );
    }

    let fail_closed = outcomes
        .iter()
        .any(|outcome| !outcome.produced_usable_verdict());
    if let Err(err) = write_chunked_review_audit(
        ctx.project_root,
        &parent_startup_env,
        &ctx.plan,
        &outcomes,
        diff_fingerprint.clone(),
        final_verdict,
        fail_closed,
    ) {
        warn!(error = %err, "Failed to write chunked-review audit artifact");
    }

    print_reviewer_outcomes(&outcomes);
    println!(
        "===== Chunked Review =====\nchunks: {}\nsynthesis: {}\nfinal_decision: {final_verdict}",
        ctx.plan.chunk_count(),
        chunks_all_usable
    );

    let review_session_ids = outcomes
        .iter()
        .filter(|outcome| outcome.produced_usable_verdict())
        .map(|outcome| outcome.session_id.clone())
        .collect::<Vec<_>>();
    super::bug_class_pipeline::maybe_extract_recurring_bug_class_skills(
        ctx.project_root,
        &review_session_ids,
    );

    Ok(if final_verdict == CLEAN { 0 } else { 1 })
}

#[path = "review_cmd_chunking_plan.rs"]
mod planning;
use planning::*;

#[path = "review_cmd_chunking_synthesis.rs"]
mod synthesis;
use synthesis::*;

#[cfg(test)]
#[path = "review_cmd_chunking_tests.rs"]
mod tests;
