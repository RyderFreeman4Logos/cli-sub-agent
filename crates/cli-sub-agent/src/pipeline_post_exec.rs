//! Post-execution processing for pipeline sessions.
//!
//! Handles session state updates, token tracking, result persistence,
//! structured output parsing, hooks, and memory capture after tool execution.

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use tracing::{info, warn};

use csa_config::{GlobalConfig, MemoryBackend, ProjectConfig};
use csa_executor::{CODEX_EXEC_INITIAL_STALL_REASON, Executor};
use csa_hooks::{HookEvent, run_hooks_for_event};
use csa_session::{
    MetaSessionState, SessionArtifact, SessionResult, TokenUsage, ToolState, get_session_dir,
    load_result, load_session, save_result, save_session,
};

use crate::memory_capture;
use crate::pipeline_handoff::write_handoff_artifact;
use crate::run_helpers::{is_compress_command, parse_token_usage};
#[path = "pipeline_post_exec_audit.rs"]
mod audit;

const FALLBACK_OUTPUT_TAIL_LINES: usize = 8;
const OUTPUT_LOG_TAIL_READ_BYTES: u64 = 8 * 1024;
/// Sessions shorter than this threshold (in seconds) that exit 0 with zero
/// tool calls in sa-mode are classified as no-op exits.  Hardcoded — not
/// a config key — to keep the gate simple and predictable.
const NO_OP_ELAPSED_THRESHOLD_SECS: i64 = 60;
/// All inputs needed for post-execution processing.
pub(crate) struct PostExecContext<'a> {
    pub executor: &'a Executor,
    pub prompt: &'a str,
    pub effective_prompt: &'a str,
    pub task_type: Option<&'a str>,
    pub readonly_project_root: bool,
    pub project_root: &'a Path,
    pub config: Option<&'a ProjectConfig>,
    pub global_config: Option<&'a GlobalConfig>,
    pub session_dir: PathBuf,
    pub sessions_root: String,
    pub execution_start_time: chrono::DateTime<chrono::Utc>,
    pub hooks_config: &'a csa_hooks::HooksConfig,
    pub memory_project_key: Option<String>,
    pub provider_session_id: Option<String>,
    pub events_count: u64,
    pub transcript_artifacts: Vec<SessionArtifact>,
    /// File paths changed during tool execution (empty for PreRun or when
    /// git workspace snapshots are unavailable).
    pub changed_paths: Vec<String>,
    /// Fresh repo baseline captured immediately before the current execution.
    pub pre_exec_snapshot: Option<PreExecutionSnapshot>,
    /// Whether the transport observed any tool calls during execution.
    pub has_tool_calls: bool,
    /// Whether this session is running in SA (sub-agent / autonomous) mode.
    pub sa_mode: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PreExecutionSnapshot {
    pub head: String,
    pub porcelain: Option<String>,
}

/// Process the results of tool execution: update session, persist artifacts, fire hooks.
///
/// Returns the final `ExecutionResult` and metadata needed by the caller.
pub(crate) async fn process_execution_result(
    ctx: PostExecContext<'_>,
    session: &mut MetaSessionState,
    result: &mut csa_process::ExecutionResult,
) -> anyhow::Result<()> {
    let token_usage = parse_token_usage(&result.output);

    // Update session tool state
    update_tool_state(
        session,
        ctx.executor.tool_name(),
        &ctx.provider_session_id,
        result,
        &token_usage,
    );
    // Clear stale interruption markers once a run reaches post-exec.
    session.termination_reason = None;
    session.last_accessed = chrono::Utc::now();

    // Detect compress/compact commands: mark session as Available for reuse
    if result.exit_code == 0 && is_compress_command(ctx.prompt) {
        session.context_status.is_compacted = true;
        session.context_status.last_compacted_at = Some(chrono::Utc::now());
        match session.apply_phase_event(csa_session::PhaseEvent::Compressed) {
            Ok(()) => {
                info!(
                    session = %session.meta_session_id,
                    "Session compacted and marked Available for reuse"
                );
            }
            Err(e) => {
                warn!(
                    session = %session.meta_session_id,
                    error = %e,
                    "Skipping phase transition on compress"
                );
            }
        }
    }

    // Increment turn count
    session.turn_count += 1;

    // Update cumulative token usage
    update_cumulative_tokens(session, token_usage);

    // Write effective_prompt to input/ for audit trail
    write_prompt_audit(&ctx.session_dir, ctx.effective_prompt);

    // Persist structured output sections from output.log markers before
    // finalizing result.toml so we can repair low-signal summaries.
    persist_output_sections(&ctx.session_dir);
    let classified_codex_exec_initial_stall = is_codex_exec_initial_stall_summary(
        ctx.executor.tool_name(),
        result.exit_code,
        &result.summary,
    );
    let classified_summary = result.summary.clone();

    // Write structured result
    let execution_end_time = chrono::Utc::now();
    let mut session_result = SessionResult {
        status: SessionResult::status_from_exit_code(result.exit_code),
        exit_code: result.exit_code,
        summary: result.summary.clone(),
        tool: ctx.executor.tool_name().to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: ctx.execution_start_time,
        completed_at: execution_end_time,
        events_count: ctx.events_count,
        artifacts: ctx.transcript_artifacts.clone(),
        peak_memory_mb: result.peak_memory_mb,
        manager_fields: Default::default(),
    };
    if let Err(err) = crate::session_observability::enrich_result_from_session_dir(
        ctx.project_root,
        &session.meta_session_id,
        &ctx.session_dir,
        &mut session_result,
    ) {
        warn!(
            session = %session.meta_session_id,
            error = %err,
            "Failed to enrich session result from persisted artifacts"
        );
    } else if session_result.summary != result.summary {
        result.summary = session_result.summary.clone();
        if let Some(tool_state) = session.tools.get_mut(ctx.executor.tool_name()) {
            tool_state.last_action_summary = session_result.summary.clone();
        }
    }
    if classified_codex_exec_initial_stall {
        session_result.status = SessionResult::status_from_exit_code(1);
        session_result.summary = classified_summary.clone();
        result.summary = classified_summary.clone();
        if let Some(tool_state) = session.tools.get_mut(ctx.executor.tool_name()) {
            tool_state.last_action_summary = classified_summary;
        }
    }
    // No-op exit gate: detect sa-mode sessions that reported success but
    // produced zero useful work (single turn, no tool calls, very short
    // elapsed time).  Rewrite status to failure so orchestrators retry.
    let elapsed_secs = (execution_end_time - ctx.execution_start_time).num_seconds();
    if ctx.sa_mode
        && ctx.task_type.is_none_or(|t| t == "run")
        && result.exit_code == 0
        && session.turn_count <= 1
        && !ctx.has_tool_calls
        && ctx.changed_paths.is_empty()
        && elapsed_secs < NO_OP_ELAPSED_THRESHOLD_SECS
    {
        let original_summary = session_result.summary.clone();
        let no_op_summary = format!(
            "no-op exit detected: turn_count={}, elapsed={}s, no tool calls. Original: {}",
            session.turn_count, elapsed_secs, original_summary,
        );
        warn!(
            session = %session.meta_session_id,
            turn_count = session.turn_count,
            elapsed_secs,
            "SA-mode no-op exit gate triggered — rewriting status to failure"
        );
        session_result.exit_code = 1;
        session_result.status = SessionResult::status_from_exit_code(1);
        session_result.summary = no_op_summary.clone();
        result.summary = no_op_summary.clone();
        result.exit_code = 1;
        // Sync tool_state so state.toml agrees with result.toml after rewrite.
        if let Some(tool_state) = session.tools.get_mut(ctx.executor.tool_name()) {
            tool_state.last_exit_code = 1;
            tool_state.last_action_summary = no_op_summary;
        }
    }
    crate::pipeline_jj_journal::maybe_record_post_run_snapshot(
        ctx.config.map(|config| &config.vcs),
        ctx.project_root,
        &ctx.session_dir,
        &session.meta_session_id,
        ctx.executor.tool_name(),
        &ctx.changed_paths,
        result,
    );
    audit::maybe_record_repo_write_audit(&ctx, session, &mut session_result);
    if let Err(e) = save_result(ctx.project_root, &session.meta_session_id, &session_result) {
        warn!("Failed to save session result: {}", e);
    }
    // Best-effort cooldown marker (ctx already holds session_dir)
    csa_session::write_cooldown_marker_from_session_dir(
        &ctx.session_dir,
        &session.meta_session_id,
        session_result.completed_at,
    );

    // Save session
    save_session(session)?;

    // Write handoff.toml: structured context-transfer artifact for subsequent sessions.
    write_handoff_artifact(
        &ctx.session_dir,
        session,
        result,
        ctx.executor.tool_name(),
        ctx.execution_start_time,
    );

    // Derive changed crates from changed paths for hook variables.
    let changed_crates =
        crate::pipeline::changed_paths::derive_changed_crates(ctx.project_root, &ctx.changed_paths);
    let changed_paths_json =
        crate::pipeline::changed_paths::format_changed_paths_json(&ctx.changed_paths);
    let changed_crates_str = crate::pipeline::changed_paths::format_changed_crates(&changed_crates);
    let changed_crates_flags =
        crate::pipeline::changed_paths::format_changed_crates_flags(&changed_crates);

    // Fire PostRun and SessionComplete hooks, capture memory
    let hook_vars = std::collections::HashMap::from([
        ("session_id".to_string(), session.meta_session_id.clone()),
        (
            "session_dir".to_string(),
            ctx.session_dir.display().to_string(),
        ),
        ("sessions_root".to_string(), ctx.sessions_root.to_string()),
        ("tool".to_string(), ctx.executor.tool_name().to_string()),
        (
            "project_root".to_string(),
            ctx.project_root.display().to_string(),
        ),
        ("exit_code".to_string(), result.exit_code.to_string()),
        ("CHANGED_PATHS".to_string(), changed_paths_json),
        ("CHANGED_CRATES".to_string(), changed_crates_str),
        ("CHANGED_CRATES_FLAGS".to_string(), changed_crates_flags),
    ]);

    // PostRun hook: fires after every tool execution
    crate::pipeline::run_pipeline_hook(HookEvent::PostRun, ctx.hooks_config, &hook_vars)?;

    // PostEdit hook: fires when .rs files are among changed paths (observational clippy check)
    if ctx.changed_paths.iter().any(|p| p.ends_with(".rs")) {
        crate::pipeline::run_pipeline_hook(HookEvent::PostEdit, ctx.hooks_config, &hook_vars)?;
    }

    crate::pipeline_jj_journal::maybe_aggregate_session_snapshots(
        ctx.config.map(|config| &config.vcs),
        ctx.project_root,
        &ctx.session_dir,
        &session.meta_session_id,
        session.genealogy.depth,
        result,
    )
    .await;

    // Legacy memory capture. Mempal capture is tied to SessionComplete below
    // so it runs after the session result and hook artifacts are written.
    let memory_config = ctx
        .config
        .map(|cfg| &cfg.memory)
        .filter(|m| !m.is_default())
        .or_else(|| ctx.global_config.map(|cfg| &cfg.memory));
    if let Some(memory_config) = memory_config {
        match csa_memory::resolve_backend(memory_config.backend) {
            MemoryBackend::Legacy => {
                if let Err(e) = memory_capture::capture_session_memory(
                    memory_config,
                    &ctx.session_dir,
                    ctx.memory_project_key.as_deref(),
                    Some(ctx.executor.tool_name()),
                    Some(session.meta_session_id.as_str()),
                )
                .await
                {
                    warn!("Memory capture failed: {}", e);
                }
            }
            MemoryBackend::Mempal | MemoryBackend::Auto => {}
        }
    }

    // SessionComplete hook: git-commits session artifacts
    if let Err(e) = run_hooks_for_event(HookEvent::SessionComplete, ctx.hooks_config, &hook_vars) {
        warn!("SessionComplete hook failed: {}", e);
    }

    if let Some(memory_config) = memory_config
        && matches!(
            csa_memory::resolve_backend(memory_config.backend),
            MemoryBackend::Mempal
        )
    {
        csa_hooks::mempal_capture::spawn_mempal_ingest(
            memory_config,
            "csa-session",
            &ctx.session_dir.join("result.toml"),
            ctx.project_root,
            Some(ctx.executor.tool_name()),
        );
    }

    // Tool output compression: runs last so parse_token_usage and hooks see
    // the full output while the caller receives the compact placeholder.
    maybe_compress_tool_output(ctx.config, ctx.project_root, session, result)?;

    Ok(())
}

fn is_codex_exec_initial_stall_summary(tool_name: &str, exit_code: i32, summary: &str) -> bool {
    tool_name == "codex"
        && exit_code == 137
        && summary.starts_with(&format!(
            "{CODEX_EXEC_INITIAL_STALL_REASON}: no stdout within "
        ))
        && summary.contains(" (effort=")
}

/// If tool output compression is enabled, persist the original output to
/// `{session_dir}/tool_outputs/` and replace `result.output` with a compact
/// placeholder.
fn maybe_compress_tool_output(
    config: Option<&ProjectConfig>,
    project_root: &Path,
    session: &MetaSessionState,
    result: &mut csa_process::ExecutionResult,
) -> anyhow::Result<()> {
    let Some(cfg) = config else { return Ok(()) };
    if !cfg.session.tool_output_compression {
        return Ok(());
    }
    let threshold = cfg.session.tool_output_threshold_bytes;
    if let csa_process::CompressDecision::Compress {
        original_bytes,
        replacement: _,
    } = csa_process::should_compress_output(&result.output, threshold)
    {
        let session_dir = get_session_dir(project_root, &session.meta_session_id)?;
        let store = csa_session::tool_output_store::ToolOutputStore::new(&session_dir)?;
        let index = session.turn_count.saturating_sub(1);
        store.store(index, result.output.as_bytes())?;
        store.append_manifest(index, original_bytes as u64)?;
        info!(
            session = %session.meta_session_id,
            original_bytes,
            index,
            "Compressed tool output stored"
        );
        // Override generic placeholder with session-specific one for recoverability.
        result.output = format!(
            "[Tool output compressed: {original_bytes} bytes → csa session tool-output {} {index}]",
            session.meta_session_id
        );
    }
    Ok(())
}

pub(crate) fn ensure_terminal_result_for_session_on_post_exec_error(
    project_root: &Path,
    session_id: &str,
    tool_name: &str,
    execution_start_time: chrono::DateTime<chrono::Utc>,
    error: &anyhow::Error,
) {
    let mut session = match load_session(project_root, session_id) {
        Ok(session) => session,
        Err(load_err) => {
            warn!(
                session = %session_id,
                error = %load_err,
                "Failed to load session for post-exec error fallback"
            );
            return;
        }
    };

    ensure_terminal_result_on_post_exec_error(
        project_root,
        &mut session,
        tool_name,
        execution_start_time,
        error,
    );
}

pub(crate) fn build_fallback_result_summary(session_dir: &Path, summary_prefix: &str) -> String {
    match read_output_log_tail(session_dir, FALLBACK_OUTPUT_TAIL_LINES) {
        Some(output_tail) => format!("{summary_prefix}\n\nLast output:\n{output_tail}"),
        None => summary_prefix.to_string(),
    }
}

pub(crate) fn collect_fallback_result_artifacts(
    project_root: &Path,
    session_id: &str,
) -> Vec<SessionArtifact> {
    match csa_session::list_artifacts(project_root, session_id) {
        Ok(artifact_names) => artifact_names
            .into_iter()
            .map(|name| SessionArtifact::new(format!("output/{name}")))
            .collect(),
        Err(err) => {
            warn!(
                session = %session_id,
                error = %err,
                "Failed to enumerate output artifacts for fallback result"
            );
            Vec::new()
        }
    }
}

/// Best-effort fail-safe when post-exec processing returns an error.
///
/// If `result.toml` is missing, persist a synthetic failure result so callers
/// never observe an Active session without a terminal result packet.
pub(crate) fn ensure_terminal_result_on_post_exec_error(
    project_root: &Path,
    session: &mut MetaSessionState,
    tool_name: &str,
    execution_start_time: chrono::DateTime<chrono::Utc>,
    error: &anyhow::Error,
) {
    match load_result(project_root, &session.meta_session_id) {
        Ok(Some(_)) => return,
        Ok(None) => {}
        Err(load_err) => {
            warn!(
                session = %session.meta_session_id,
                error = %load_err,
                "Failed to read existing result.toml during post-exec error fallback; attempting overwrite"
            );
        }
    }

    let summary_prefix = format!("post-exec: {error}");
    let summary = match get_session_dir(project_root, &session.meta_session_id) {
        Ok(session_dir) => build_fallback_result_summary(&session_dir, &summary_prefix),
        Err(session_dir_err) => {
            warn!(
                session = %session.meta_session_id,
                error = %session_dir_err,
                "Failed to resolve session dir for post-exec fallback summary"
            );
            summary_prefix
        }
    };
    let artifacts = collect_fallback_result_artifacts(project_root, &session.meta_session_id);
    let completed_at = chrono::Utc::now();
    let fallback_result = SessionResult {
        status: "failure".to_string(),
        exit_code: 1,
        summary,
        tool: tool_name.to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: execution_start_time,
        completed_at,
        events_count: 0,
        artifacts,
        peak_memory_mb: None,
        manager_fields: Default::default(),
    };

    if let Err(save_err) = save_result(project_root, &session.meta_session_id, &fallback_result) {
        warn!(
            session = %session.meta_session_id,
            error = %save_err,
            "Failed to write fallback post-exec result.toml"
        );
        return;
    }
    // Best-effort cooldown marker
    csa_session::write_cooldown_marker_for_project(
        project_root,
        &session.meta_session_id,
        completed_at,
    );

    session.termination_reason = Some("post_exec_error".to_string());
    session.last_accessed = completed_at;
    if let Err(save_err) = save_session(session) {
        warn!(
            session = %session.meta_session_id,
            error = %save_err,
            "Failed to persist session state after fallback post-exec result write"
        );
    }
}

fn read_output_log_tail(session_dir: &Path, max_lines: usize) -> Option<String> {
    let output_log_path = session_dir.join("output.log");
    let mut file = match File::open(&output_log_path) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return None,
        Err(err) => {
            warn!(
                path = %output_log_path.display(),
                error = %err,
                "Failed to read output.log for fallback summary"
            );
            return None;
        }
    };

    let file_len = match file.metadata() {
        Ok(metadata) => metadata.len(),
        Err(err) => {
            warn!(
                path = %output_log_path.display(),
                error = %err,
                "Failed to stat output.log for fallback summary"
            );
            return None;
        }
    };
    let tail_start = file_len.saturating_sub(OUTPUT_LOG_TAIL_READ_BYTES);
    if let Err(err) = file.seek(SeekFrom::Start(tail_start)) {
        warn!(
            path = %output_log_path.display(),
            error = %err,
            "Failed to seek output.log for fallback summary"
        );
        return None;
    }

    // Read raw bytes first, then decode — seeking may land mid-UTF-8 char.
    let mut raw_bytes = Vec::new();
    match file.read_to_end(&mut raw_bytes) {
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return None,
        Err(err) => {
            warn!(
                path = %output_log_path.display(),
                error = %err,
                "Failed to read output.log for fallback summary"
            );
            return None;
        }
    };

    // Lossy decode handles mid-UTF-8 seek boundary gracefully.
    let mut contents = String::from_utf8_lossy(&raw_bytes).into_owned();
    if tail_start > 0
        && let Some(first_newline) = contents.find('\n')
    {
        contents.drain(..=first_newline);
    }

    let tail = contents
        .lines()
        .rev()
        .filter(|line| !line.trim().is_empty())
        .take(max_lines)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if tail.is_empty() {
        return None;
    }

    Some(tail.into_iter().rev().collect::<Vec<_>>().join("\n"))
}

fn update_tool_state(
    session: &mut MetaSessionState,
    tool_name: &str,
    provider_session_id: &Option<String>,
    result: &csa_process::ExecutionResult,
    token_usage: &Option<TokenUsage>,
) {
    session
        .tools
        .entry(tool_name.to_string())
        .and_modify(|t| {
            if let Some(session_id) = provider_session_id {
                t.provider_session_id = Some(session_id.clone());
            }
            t.last_action_summary = result.summary.clone();
            t.last_exit_code = result.exit_code;
            t.updated_at = chrono::Utc::now();

            if let Some(usage) = token_usage {
                t.token_usage = Some(usage.clone());
            }
        })
        .or_insert_with(|| ToolState {
            provider_session_id: provider_session_id.clone(),
            last_action_summary: result.summary.clone(),
            last_exit_code: result.exit_code,
            updated_at: chrono::Utc::now(),
            tool_version: None,
            token_usage: token_usage.clone(),
        });
}

fn update_cumulative_tokens(session: &mut MetaSessionState, token_usage: Option<TokenUsage>) {
    let Some(new_usage) = token_usage else {
        return;
    };

    let cumulative = session
        .total_token_usage
        .get_or_insert(TokenUsage::default());
    cumulative.input_tokens =
        Some(cumulative.input_tokens.unwrap_or(0) + new_usage.input_tokens.unwrap_or(0));
    cumulative.output_tokens =
        Some(cumulative.output_tokens.unwrap_or(0) + new_usage.output_tokens.unwrap_or(0));
    cumulative.total_tokens =
        Some(cumulative.total_tokens.unwrap_or(0) + new_usage.total_tokens.unwrap_or(0));
    cumulative.estimated_cost_usd = Some(
        cumulative.estimated_cost_usd.unwrap_or(0.0) + new_usage.estimated_cost_usd.unwrap_or(0.0),
    );

    // Update token budget tracking
    if let Some(ref mut budget) = session.token_budget {
        let tokens_used = new_usage.total_tokens.unwrap_or(0);
        budget.record_usage(tokens_used);
        if budget.is_hard_exceeded() {
            warn!(
                session = %session.meta_session_id,
                used = budget.used,
                allocated = budget.allocated,
                "Token budget hard threshold reached — advisory only"
            );
        } else if budget.is_soft_exceeded() {
            warn!(
                session = %session.meta_session_id,
                used = budget.used,
                allocated = budget.allocated,
                remaining = budget.remaining(),
                "Token budget soft threshold reached"
            );
        }
    }
}

fn write_prompt_audit(session_dir: &Path, effective_prompt: &str) {
    let input_dir = session_dir.join("input");
    if input_dir.exists() {
        let prompt_path = input_dir.join("prompt.txt");
        if let Err(e) = fs::write(&prompt_path, effective_prompt) {
            warn!("Failed to write prompt to input/: {}", e);
        }
    }
}

fn persist_output_sections(session_dir: &Path) {
    let output_log_path = session_dir.join("output.log");
    if output_log_path.exists()
        && let Err(e) =
            csa_session::persist_structured_output_from_file(session_dir, &output_log_path)
    {
        warn!("Failed to persist structured output: {}", e);
    }
}

#[cfg(test)]
#[path = "pipeline_tests_post_exec.rs"]
mod tests;

#[cfg(test)]
#[path = "pipeline_tests_no_op_gate.rs"]
mod no_op_gate_tests;
