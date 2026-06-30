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

pub(super) fn activation_reason(
    diff_size: Option<&ReviewDiffSize>,
    config: &ReviewChunkingConfig,
) -> Option<ReviewChunkActivationReason> {
    match config.mode {
        ReviewChunkingMode::Off => None,
        ReviewChunkingMode::Always => Some(ReviewChunkActivationReason::Always),
        ReviewChunkingMode::Auto => {
            let diff_size = diff_size?;
            if diff_size.files >= config.activate_files {
                Some(ReviewChunkActivationReason::FileCount)
            } else if diff_size.changed_lines > config.activate_changed_lines {
                Some(ReviewChunkActivationReason::ChangedLines)
            } else if diff_size.bytes > config.activate_diff_bytes {
                Some(ReviewChunkActivationReason::DiffBytes)
            } else {
                None
            }
        }
    }
}

fn collect_review_chunk_files(project_root: &Path, scope: &str) -> Result<Vec<ReviewChunkFile>> {
    let mut files = collect_numstat_files(project_root, scope)?;
    apply_name_status(project_root, scope, &mut files)?;
    if scope == "uncommitted" {
        append_untracked_files(project_root, &mut files)?;
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    files.dedup_by(|left, right| left.path == right.path);
    Ok(files)
}

fn collect_numstat_files(project_root: &Path, scope: &str) -> Result<Vec<ReviewChunkFile>> {
    let output = run_git(project_root, &git_diff_args(scope, "--numstat"))?;
    Ok(parse_numstat_output(&output))
}

fn apply_name_status(
    project_root: &Path,
    scope: &str,
    files: &mut [ReviewChunkFile],
) -> Result<()> {
    let output = run_git(project_root, &git_diff_args(scope, "--name-status"))?;
    let statuses = parse_name_status_output(&output);
    for file in files {
        if let Some(status) = statuses.get(&file.path) {
            file.status = status.clone();
        }
    }
    Ok(())
}

fn append_untracked_files(project_root: &Path, files: &mut Vec<ReviewChunkFile>) -> Result<()> {
    let output = run_git(
        project_root,
        &["ls-files", "--others", "--exclude-standard"]
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>(),
    )?;
    let existing = files
        .iter()
        .map(|file| file.path.clone())
        .collect::<BTreeSet<_>>();
    for path in output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        if !existing.contains(path) {
            files.push(ReviewChunkFile {
                path: path.to_string(),
                status: "A".to_string(),
                changed_lines: 1,
            });
        }
    }
    Ok(())
}

fn run_git(project_root: &Path, args: &[String]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(project_root)
        .output()
        .with_context(|| format!("failed to run git {}", args.join(" ")))?;
    if !output.status.success() {
        anyhow::bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn git_diff_args(scope: &str, mode_flag: &str) -> Vec<String> {
    let mut args = match scope {
        "uncommitted" => vec!["diff".to_string(), "HEAD".to_string()],
        _ if scope.starts_with("range:") => vec![
            "diff".to_string(),
            scope.trim_start_matches("range:").to_string(),
        ],
        _ if scope.starts_with("base:") => vec![
            "diff".to_string(),
            scope.trim_start_matches("base:").to_string(),
        ],
        _ if scope.starts_with("commit:") => vec![
            "show".to_string(),
            "--format=".to_string(),
            scope.trim_start_matches("commit:").to_string(),
        ],
        _ if scope.starts_with("files:") => {
            let mut args = vec!["diff".to_string(), "HEAD".to_string(), "--".to_string()];
            args.extend(
                scope
                    .trim_start_matches("files:")
                    .split_whitespace()
                    .map(str::to_string),
            );
            args
        }
        _ => vec!["diff".to_string(), scope.to_string()],
    };
    let insert_at = 1;
    args.insert(insert_at, mode_flag.to_string());
    args.insert(insert_at + 1, "-M".to_string());
    args.insert(insert_at + 2, "--no-color".to_string());
    args
}

fn parse_numstat_output(output: &str) -> Vec<ReviewChunkFile> {
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let added = fields.next()?;
            let deleted = fields.next()?;
            let path = fields.next()?;
            let changed_lines =
                parse_numstat_count(added).saturating_add(parse_numstat_count(deleted));
            Some(ReviewChunkFile {
                path: normalize_numstat_path(path),
                status: "M".to_string(),
                changed_lines,
            })
        })
        .collect()
}

fn parse_numstat_count(raw: &str) -> usize {
    raw.parse::<usize>().unwrap_or(0)
}

pub(super) fn normalize_numstat_path(raw: &str) -> String {
    if let Some((prefix, rename)) = raw.split_once('{')
        && let Some((_, rest)) = rename.split_once("=>")
        && let Some((to, suffix)) = rest.split_once('}')
    {
        return format!("{}{}{}", prefix, to.trim(), suffix);
    }
    if let Some((_, to)) = raw.split_once("=>") {
        return to.trim().to_string();
    }
    raw.to_string()
}

pub(super) fn parse_name_status_output(output: &str) -> BTreeMap<String, String> {
    let mut statuses = BTreeMap::new();
    for line in output.lines() {
        let fields = line.split('\t').collect::<Vec<_>>();
        if fields.len() < 2 {
            continue;
        }
        let status = fields[0].chars().next().unwrap_or('M').to_string();
        let path = if status == "R" || status == "C" {
            fields.get(2).copied().unwrap_or(fields[1])
        } else {
            fields[1]
        };
        statuses.insert(path.to_string(), status);
    }
    statuses
}

pub(super) fn plan_review_chunks_from_files(
    scope: &str,
    diff_size: Option<&ReviewDiffSize>,
    files: Vec<ReviewChunkFile>,
    activation_reason: ReviewChunkActivationReason,
    config: &ReviewChunkingConfig,
) -> ReviewChunkPlan {
    let mut grouped = BTreeMap::<String, Vec<ReviewChunkFile>>::new();
    for file in files {
        grouped
            .entry(group_key_for_path(&file.path))
            .or_default()
            .push(file);
    }

    let mut chunks = Vec::new();
    let mut current = Vec::new();
    for (_, mut group_files) in grouped {
        group_files.sort_by(|left, right| left.path.cmp(&right.path));
        let group_chunks = split_group_files(group_files, config);
        for group_chunk in group_chunks {
            if should_start_new_chunk(&current, &group_chunk, config) {
                chunks.push(std::mem::take(&mut current));
            }
            current.extend(group_chunk);
        }
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks = cap_chunk_count(chunks, config.max_chunks);

    let chunks = chunks
        .into_iter()
        .enumerate()
        .map(|(idx, files)| build_chunk(idx + 1, files))
        .collect::<Vec<_>>();
    let total_changed_lines = chunks.iter().map(|chunk| chunk.changed_lines).sum();
    let total_files = chunks.iter().map(|chunk| chunk.files.len()).sum();
    let raw_diff_bytes = diff_size.map_or(0, |size| size.bytes);

    ReviewChunkPlan {
        scope: scope.to_string(),
        activation_reason,
        total_files,
        total_changed_lines,
        raw_diff_bytes,
        chunks,
    }
}

fn split_group_files(
    files: Vec<ReviewChunkFile>,
    config: &ReviewChunkingConfig,
) -> Vec<Vec<ReviewChunkFile>> {
    let mut chunks = Vec::new();
    let mut current = Vec::new();
    for file in files {
        let next_lines = changed_lines(&current).saturating_add(file.changed_lines);
        let next_files = current.len().saturating_add(1);
        if !current.is_empty()
            && (next_files > config.max_files_per_chunk
                || next_lines > config.max_changed_lines_per_chunk)
        {
            chunks.push(std::mem::take(&mut current));
        }
        current.push(file);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

fn should_start_new_chunk(
    current: &[ReviewChunkFile],
    incoming: &[ReviewChunkFile],
    config: &ReviewChunkingConfig,
) -> bool {
    !current.is_empty()
        && (current.len().saturating_add(incoming.len()) > config.target_files_per_chunk
            || changed_lines(current).saturating_add(changed_lines(incoming))
                > config.target_changed_lines_per_chunk)
}

fn cap_chunk_count(
    mut chunks: Vec<Vec<ReviewChunkFile>>,
    max_chunks: usize,
) -> Vec<Vec<ReviewChunkFile>> {
    let max_chunks = max_chunks.max(1);
    while chunks.len() > max_chunks {
        let merge_index = chunks
            .windows(2)
            .enumerate()
            .min_by_key(|(_, pair)| {
                pair[0]
                    .len()
                    .saturating_add(pair[1].len())
                    .saturating_add(changed_lines(&pair[0]))
                    .saturating_add(changed_lines(&pair[1]))
            })
            .map(|(idx, _)| idx)
            .unwrap_or(0);
        let right = chunks.remove(merge_index + 1);
        chunks[merge_index].extend(right);
    }
    chunks
}

fn build_chunk(id: usize, files: Vec<ReviewChunkFile>) -> ReviewChunk {
    let changed_lines = changed_lines(&files);
    let pathspecs = files.iter().map(|file| file.path.clone()).collect();
    let group = summarize_chunk_group(&files);
    ReviewChunk {
        id,
        group,
        estimated_tokens: estimate_tokens(files.len(), changed_lines),
        files,
        pathspecs,
        changed_lines,
    }
}

fn changed_lines(files: &[ReviewChunkFile]) -> usize {
    files.iter().map(|file| file.changed_lines).sum()
}

fn estimate_tokens(files: usize, changed_lines: usize) -> usize {
    files
        .saturating_mul(80)
        .saturating_add(changed_lines.saturating_mul(6))
}

fn summarize_chunk_group(files: &[ReviewChunkFile]) -> String {
    let groups = files
        .iter()
        .map(|file| group_key_for_path(&file.path))
        .collect::<BTreeSet<_>>();
    if groups.len() == 1 {
        groups.into_iter().next().unwrap_or_else(|| ".".to_string())
    } else {
        "mixed".to_string()
    }
}

fn group_key_for_path(path: &str) -> String {
    let path = Path::new(path);
    let mut components = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => value.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>();
    if components.is_empty() {
        return ".".to_string();
    }
    if components.first() == Some(&"crates") && components.len() >= 2 {
        return format!("crates/{}", components[1]);
    }
    if components.first() == Some(&"src") && components.len() >= 2 {
        components.truncate(2);
        return components.join("/");
    }
    components[0].to_string()
}

async fn run_synthesis_review(
    ctx: &ChunkedReviewContext<'_>,
    parent_startup_env: &StartupSubtreeEnv,
    diff_fingerprint: Option<&str>,
    outcomes: &[ReviewerOutcome],
) -> Result<ReviewerOutcome> {
    let reviewer_index = ctx.plan.chunk_count();
    let consolidated = load_consolidated_chunk_artifact(
        ctx.project_root,
        parent_startup_env.session_dir().map(Path::new),
        outcomes,
    )?;
    let prompt = build_synthesis_review_instruction(
        ctx.prompt,
        &ctx.plan,
        outcomes,
        &consolidated,
        reviewer_index + 1,
        diff_fingerprint,
    );
    let result = execute_review_with_tier_filter(
        ctx.tool,
        prompt,
        None,
        ctx.review_model.clone(),
        ctx.resolved_model_spec.clone(),
        ctx.resolved_tier_name.clone(),
        ctx.tier_active,
        ctx.tier_preference_order.clone(),
        ctx.review_thinking.clone(),
        format!(
            "review[synthesis]: {}",
            crate::run_helpers::truncate_prompt(ctx.scope, 80)
        ),
        ctx.project_root,
        ctx.config.as_ref(),
        ctx.global_config,
        ctx.pre_session_hook.clone(),
        ctx.review_routing.clone(),
        ctx.stream_mode,
        ctx.idle_timeout_seconds,
        ctx.initial_response_timeout_seconds,
        ctx.args.force_override_user_config,
        ctx.args.force_ignore_tier_setting,
        ctx.execution_no_failover,
        ctx.explicit_tool_with_failover,
        ctx.build_jobs,
        ctx.args.fast_but_more_cost,
        false,
        ctx.args.no_fs_sandbox,
        ctx.allow_user_daemon_ipc,
        ctx.readonly_project_root,
        &ctx.args.extra_writable,
        &ctx.args.extra_readable,
        ctx.args.error_marker_scan_override(),
        ctx.args.resource_overrides(),
        ctx.current_depth,
        parent_startup_env,
    )
    .await;

    match result {
        Ok(result) => build_reviewer_outcome(reviewer_index, ctx.tool, &result),
        Err(err) => {
            let reason = reviewer_unavailable_error_reason(&err, ctx.tool)
                .unwrap_or_else(|| format!("synthesis review failed: {err:#}"));
            Ok(build_unavailable_reviewer_outcome(
                reviewer_index,
                ctx.tool,
                reason,
            ))
        }
    }
}

fn build_chunk_review_instruction(
    base_prompt: &str,
    plan: &ReviewChunkPlan,
    chunk: &ReviewChunk,
    tool: ToolName,
    project_root: &Path,
    current_session_id: Option<&str>,
    diff_fingerprint: Option<&str>,
) -> String {
    let output_dir = reviewer_output_dir(chunk.id);
    let manifest = render_changed_file_manifest(&plan.chunks);
    let pathspecs = chunk.pathspecs.join("\n- ");
    let fingerprint = diff_fingerprint.unwrap_or("unavailable");
    let mut prompt = format!(
        "{base_prompt}\n\n\
## Chunked Review Scope\n\
You are chunk reviewer {id} of {total}. Review only findings whose concrete evidence is in this chunk.\n\
Original scope: {scope}\n\
Diff fingerprint: {fingerprint}\n\
Reviewer tool hint: {tool}\n\
Project root: {root}\n\
Parent session: {parent}\n\n\
Chunk group: {group}\n\
Chunk changed lines: {changed_lines}\n\
Chunk pathspecs:\n- {pathspecs}\n\n\
All changed files manifest:\n{manifest}\n\n\
Write review artifacts to {output_dir}/review-findings.json and {output_dir}/review-report.md.\n\
Report only findings with in-chunk file/line evidence. Do not report cross-chunk speculation.",
        id = chunk.id,
        total = plan.chunk_count(),
        scope = plan.scope,
        tool = tool.as_str(),
        root = project_root.display(),
        parent = current_session_id.unwrap_or("unknown"),
        group = chunk.group,
        changed_lines = chunk.changed_lines,
    );
    crate::review_design_anchor::append_design_anchor(&mut prompt);
    prompt
}

fn build_synthesis_review_instruction(
    base_prompt: &str,
    plan: &ReviewChunkPlan,
    outcomes: &[ReviewerOutcome],
    consolidated: &ReviewArtifact,
    reviewer_index: usize,
    diff_fingerprint: Option<&str>,
) -> String {
    let output_dir = reviewer_output_dir(reviewer_index);
    let findings =
        serde_json::to_string_pretty(&consolidated.findings).unwrap_or_else(|_| "[]".to_string());
    let outcome_summary = outcomes
        .iter()
        .map(|outcome| {
            format!(
                "- chunk {} => {} ({})",
                outcome.reviewer_index + 1,
                outcome.verdict,
                outcome.session_id
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let manifest = render_changed_file_manifest(&plan.chunks);
    let fingerprint = diff_fingerprint.unwrap_or("unavailable");
    format!(
        "{base_prompt}\n\n\
## Chunked Review Synthesis\n\
You are the synthesis reviewer for {chunks} chunk reviews.\n\
Original scope: {scope}\n\
Diff fingerprint: {fingerprint}\n\n\
Chunk outcomes:\n{outcome_summary}\n\n\
All changed files manifest:\n{manifest}\n\n\
Deterministically consolidated chunk findings before synthesis:\n```json\n{findings}\n```\n\n\
Write review artifacts to {output_dir}/review-findings.json and {output_dir}/review-report.md.\n\
Add only cross-file findings backed by concrete file/line evidence from the changed files. \
Do not repeat findings already present in the consolidated list.",
        chunks = plan.chunk_count(),
        scope = plan.scope,
    )
}

fn render_changed_file_manifest(chunks: &[ReviewChunk]) -> String {
    chunks
        .iter()
        .flat_map(|chunk| {
            chunk.files.iter().map(move |file| {
                format!(
                    "- chunk {} [{} +{}] {}",
                    chunk.id, file.status, file.changed_lines, file.path
                )
            })
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn reviewer_output_dir(reviewer_index: usize) -> String {
    let reviewer_dir = format!("reviewer-{reviewer_index}");
    format!("${{{CSA_PARENT_SESSION_DIR_ENV_KEY}:-${CSA_SESSION_DIR_ENV_KEY}}}/{reviewer_dir}")
}

fn all_changed_files(plan: &ReviewChunkPlan) -> Vec<String> {
    plan.chunks
        .iter()
        .flat_map(|chunk| chunk.files.iter().map(|file| file.path.clone()))
        .collect()
}

fn parent_startup_env_for_chunked_review(
    daemon_child: bool,
    session_id: Option<&str>,
    startup_env: &StartupSubtreeEnv,
    project_root: &Path,
) -> Result<StartupSubtreeEnv> {
    if daemon_child && let Some(session_id) = session_id {
        let session_dir = csa_session::get_session_dir(project_root, session_id)?;
        return Ok(startup_env
            .clone()
            .with_current_session(session_id, session_dir.display().to_string()));
    }

    Ok(startup_env.clone())
}

fn load_consolidated_chunk_artifact(
    project_root: &Path,
    parent_session_dir: Option<&Path>,
    outcomes: &[ReviewerOutcome],
) -> Result<ReviewArtifact> {
    let artifacts = outcomes
        .iter()
        .filter_map(|outcome| load_chunk_artifact(project_root, parent_session_dir, outcome).ok())
        .collect::<Vec<_>>();
    Ok(build_consolidated_artifact(artifacts, "chunked-review"))
}

fn load_chunk_artifact(
    project_root: &Path,
    parent_session_dir: Option<&Path>,
    outcome: &ReviewerOutcome,
) -> Result<ReviewArtifact> {
    let reviewer_dir = format!("reviewer-{}", outcome.reviewer_index + 1);
    let mut candidates = Vec::new();
    if let Some(parent_session_dir) = parent_session_dir {
        candidates.push(
            parent_session_dir
                .join(&reviewer_dir)
                .join("review-findings.json"),
        );
    }
    if let Ok(session_dir) = csa_session::get_session_dir(project_root, &outcome.session_id) {
        candidates.push(session_dir.join(&reviewer_dir).join("review-findings.json"));
        candidates.push(session_dir.join("output").join("review-findings.json"));
    }
    for path in candidates {
        if !path.exists() {
            continue;
        }
        let content = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        return super::parent_artifacts::parse_reviewer_artifact(&path, &content);
    }
    anyhow::bail!("no review-findings.json for {}", outcome.session_id)
}

fn persist_chunk_session_meta(
    project_root: &Path,
    parent_session_dir: Option<&Path>,
    reviewer_index: usize,
    outcome: &ReviewerOutcome,
    meta: ReviewChunkSessionMeta,
) -> Result<()> {
    let payload = serde_json::to_vec_pretty(&meta).context("failed to serialize chunk metadata")?;
    if let Ok(session_dir) = csa_session::get_session_dir(project_root, &outcome.session_id) {
        let output_dir = session_dir.join("output");
        fs::create_dir_all(&output_dir)
            .with_context(|| format!("failed to create {}", output_dir.display()))?;
        fs::write(output_dir.join("chunked-review-meta.json"), &payload)
            .context("failed to write child chunk metadata")?;
    }
    if let Some(parent_session_dir) = parent_session_dir {
        let reviewer_dir = parent_session_dir.join(format!("reviewer-{}", reviewer_index + 1));
        fs::create_dir_all(&reviewer_dir)
            .with_context(|| format!("failed to create {}", reviewer_dir.display()))?;
        fs::write(reviewer_dir.join("chunked-review-meta.json"), payload)
            .context("failed to write parent chunk metadata")?;
    }
    Ok(())
}

fn final_chunked_verdict(outcomes: &[ReviewerOutcome], all_unavailable: bool) -> &'static str {
    if all_unavailable {
        return UNAVAILABLE;
    }
    if outcomes
        .iter()
        .any(|outcome| !outcome.produced_usable_verdict())
    {
        return HAS_ISSUES;
    }
    if outcomes.iter().any(|outcome| outcome.verdict == HAS_ISSUES) {
        HAS_ISSUES
    } else {
        CLEAN
    }
}

fn write_chunked_review_audit(
    project_root: &Path,
    startup_env: &StartupSubtreeEnv,
    plan: &ReviewChunkPlan,
    outcomes: &[ReviewerOutcome],
    diff_fingerprint: Option<String>,
    final_verdict: &str,
    fail_closed: bool,
) -> Result<()> {
    let session_dir = audit_session_dir(project_root, startup_env, outcomes);
    let output_dir = session_dir.join("output");
    fs::create_dir_all(&output_dir)
        .with_context(|| format!("failed to create {}", output_dir.display()))?;
    let chunks = plan
        .chunks
        .iter()
        .filter_map(|chunk| {
            outcomes
                .iter()
                .find(|outcome| outcome.reviewer_index + 1 == chunk.id)
                .map(|outcome| ChunkAuditEntry {
                    chunk_id: chunk.id,
                    group: chunk.group.clone(),
                    pathspecs: chunk.pathspecs.clone(),
                    changed_lines: chunk.changed_lines,
                    estimated_tokens: chunk.estimated_tokens,
                    session_id: outcome.session_id.clone(),
                    verdict: outcome.verdict.to_string(),
                })
        })
        .collect::<Vec<_>>();
    let synthesis_session_id = outcomes
        .iter()
        .find(|outcome| outcome.reviewer_index == plan.chunk_count())
        .map(|outcome| outcome.session_id.clone());
    let audit = ChunkedReviewAudit {
        schema_version: 1,
        scope: plan.scope.clone(),
        diff_fingerprint,
        activation_reason: plan.activation_reason,
        final_verdict: final_verdict.to_string(),
        fail_closed,
        chunks,
        synthesis_session_id,
        timestamp: chrono::Utc::now(),
    };
    let payload =
        serde_json::to_vec_pretty(&audit).context("failed to serialize chunked-review audit")?;
    fs::write(output_dir.join("chunked-review.json"), payload)
        .context("failed to write output/chunked-review.json")?;
    Ok(())
}

fn audit_session_dir(
    project_root: &Path,
    startup_env: &StartupSubtreeEnv,
    outcomes: &[ReviewerOutcome],
) -> PathBuf {
    if let Some(session_dir) = startup_env.session_dir() {
        return PathBuf::from(session_dir);
    }
    outcomes
        .first()
        .and_then(|outcome| csa_session::get_session_dir(project_root, &outcome.session_id).ok())
        .unwrap_or_else(|| project_root.to_path_buf())
}

#[cfg(test)]
#[path = "review_cmd_chunking_tests.rs"]
mod tests;
