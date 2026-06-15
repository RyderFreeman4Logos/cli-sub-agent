//! Post-execution processing for pipeline sessions.
//!
//! Handles session state updates, token tracking, result persistence,
//! structured output parsing, hooks, and memory capture after tool execution.

use std::fs;
use std::path::{Path, PathBuf};

use tracing::{info, warn};

use csa_config::{GlobalConfig, MemoryBackend, ProjectConfig};
use csa_executor::{CODEX_EXEC_INITIAL_STALL_REASON, Executor};
use csa_hooks::{HookEvent, run_hooks_for_event};
use csa_session::{
    MetaSessionState, PhaseEvent, SessionArtifact, SessionPhase, SessionResult, save_session,
};
#[cfg(test)]
use csa_session::{load_result, save_result};

use crate::memory_capture;
use crate::pipeline_handoff::write_handoff_artifact;
use crate::run_helpers::{is_compress_command, parse_token_usage};
use crate::session_outcome::{
    EffectiveOutcome, classify_effective_session_outcome, incidental_downgrade_note,
    task_kind_from_task_type,
};
#[path = "pipeline_post_exec_audit.rs"]
mod audit;

#[path = "pipeline_post_exec_fallback.rs"]
mod fallback;
#[cfg(test)]
use fallback::read_output_log_tail;
pub(crate) use fallback::{
    build_fallback_result_summary, collect_fallback_result_artifacts,
    ensure_terminal_result_for_session_on_post_exec_error,
    ensure_terminal_result_on_post_exec_error,
};
#[path = "pipeline_post_exec_helpers.rs"]
mod helpers;
#[path = "pipeline_post_exec_lefthook.rs"]
mod lefthook;
#[path = "pipeline_post_exec_no_op.rs"]
mod no_op;
#[path = "pipeline_post_exec_progress.rs"]
mod progress;
#[path = "pipeline_post_exec_result_sidecar.rs"]
mod result_sidecar;
// Re-exported privately so existing call sites (and the test submodule's
// `use super::*`) reach these mechanical helpers unqualified.
use helpers::{
    is_codex_exec_initial_stall_summary, maybe_compress_tool_output, update_cumulative_tokens,
    update_tool_state,
};
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
    /// Number of agent conversation turns observed in this run (one per
    /// `AgentMessage` event). `0` means the transport did not parse streaming
    /// events; `process_execution_result` falls back to `+= 1` to preserve the
    /// legacy "one increment per csa run" semantics.
    pub turn_count: u32,
    pub output_tokens: Option<u64>,
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

    // Increment turn count. Transports that parse streaming events (claude-code
    // CLI, codex/gemini ACP) populate `ctx.turn_count` with the number of
    // observed agent conversation turns; legacy transports leave it at `0` and
    // fall back to the historical `+= 1` per-invocation contract (#1438).
    session.turn_count = session.turn_count.saturating_add(ctx.turn_count.max(1));

    let has_meaningful_reasoning_output =
        no_op::has_meaningful_reasoning_output(&token_usage, ctx.output_tokens);
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
        post_exec_gate: None,
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
        kill_hint: None,
        last_item: None,
        fallback_chain: None,
        ..Default::default()
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
        // CSA-own gate: an initial stall is a real failure. Mark it explicitly so
        // the effective-outcome classifier (#161) never downgrades it, and force
        // a nonzero exit (a stall can otherwise exit 0).
        result.mark_gate_failure(CODEX_EXEC_INITIAL_STALL_REASON);
        session_result.exit_code = result.exit_code;
        session_result.status = SessionResult::status_from_exit_code(result.exit_code);
        session_result.summary = classified_summary.clone();
        result.summary = classified_summary.clone();
        if let Some(tool_state) = session.tools.get_mut(ctx.executor.tool_name()) {
            tool_state.last_exit_code = result.exit_code;
            tool_state.last_action_summary = classified_summary;
        }
    }
    if let Some(exhaustion) = crate::run_cmd_post::detect_permanent_tool_exhaustion_result(
        ctx.executor.tool_name(),
        result,
        None,
    ) {
        let exhausted_summary = crate::run_cmd_post::format_tool_exhausted_summary(
            ctx.executor.tool_name(),
            &exhaustion.matched_pattern,
        );
        // CSA-own gate: permanent tool exhaustion (quota/rate-limit) is a real
        // failure; mark it so the #161 classifier treats the exit as fatal.
        result.mark_gate_failure("tool-exhaustion");
        result.summary = exhausted_summary.clone();
        if !result.stderr_output.contains(&exhausted_summary) {
            if !result.stderr_output.is_empty() && !result.stderr_output.ends_with('\n') {
                result.stderr_output.push('\n');
            }
            result.stderr_output.push_str(&exhausted_summary);
            result.stderr_output.push('\n');
        }
        session_result.exit_code = 1;
        session_result.status = SessionResult::status_from_exit_code(1);
        session_result.summary = exhausted_summary.clone();
        if let Err(err) = session.apply_phase_event(PhaseEvent::ToolExhausted) {
            warn!(
                session = %session.meta_session_id,
                error = %err,
                "Skipping phase transition on tool exhaustion"
            );
            session.phase = SessionPhase::ToolExhausted;
        }
        session.termination_reason = Some("tool_exhausted".to_string());
        if let Some(tool_state) = session.tools.get_mut(ctx.executor.tool_name()) {
            tool_state.last_exit_code = 1;
            tool_state.last_action_summary = exhausted_summary;
        }
    }
    // Effective-outcome classification (#161): a model turn that COMPLETED must
    // not be flipped to `failure` solely because a hook or in-turn command
    // exited nonzero. The CSA-own gates above set `csa_gate_failure`; the
    // classifier treats those (and timeout/signal exits) as authoritative-fatal
    // and only downgrades a genuinely incidental nonzero exit on a completed
    // turn. The downgrade runs BEFORE the false-success gates below so they can
    // re-examine the now-zero exit; if one re-flags a real failure it calls
    // `mark_gate_failure`, which clears the incidental warning.
    let task_kind = task_kind_from_task_type(ctx.task_type);
    let final_output_present =
        !result.output.trim().is_empty() || !result.summary.trim().is_empty();
    let mut pending_incidental_warning: Option<(String, i32)> = None;
    match classify_effective_session_outcome(
        result.model_completed,
        result.exit_code,
        result.csa_gate_failure.as_deref(),
        task_kind,
        final_output_present,
    ) {
        EffectiveOutcome::IncidentalDowngrade { raw_exit_code } => {
            let note = incidental_downgrade_note(raw_exit_code, result.terminal_reason.as_deref());
            warn!(
                session = %session.meta_session_id,
                raw_exit_code,
                "Completed turn exited nonzero for an incidental reason — downgrading to success (#161)"
            );
            result.exit_code = 0;
            session_result.exit_code = 0;
            session_result.status = SessionResult::status_from_exit_code(0);
            if let Some(tool_state) = session.tools.get_mut(ctx.executor.tool_name()) {
                tool_state.last_exit_code = 0;
            }
            result.warnings.push(note.clone());
            pending_incidental_warning = Some((note, raw_exit_code));
        }
        EffectiveOutcome::ForceFailure => {
            warn!(
                session = %session.meta_session_id,
                "Model did not complete and produced no final output — forcing failure (#161)"
            );
            result.mark_gate_failure("model-incomplete-no-output");
            session_result.exit_code = result.exit_code;
            session_result.status = SessionResult::status_from_exit_code(result.exit_code);
            if let Some(tool_state) = session.tools.get_mut(ctx.executor.tool_name()) {
                tool_state.last_exit_code = result.exit_code;
            }
        }
        EffectiveOutcome::ExitCodeAuthoritative => {}
    }
    // No-op gate: fail short successful SA-mode runs with no tool calls/output.
    let elapsed_secs = (execution_end_time - ctx.execution_start_time).num_seconds();
    if ctx.sa_mode
        && ctx.task_type.is_none_or(|t| t == "run")
        && result.exit_code == 0
        && !result_sidecar::status_is_success(&ctx.session_dir, session.turn_count)
        && session.turn_count <= 1
        && !ctx.has_tool_calls
        && !has_meaningful_reasoning_output
        && ctx.changed_paths.is_empty()
        && elapsed_secs < no_op::ELAPSED_THRESHOLD_SECS
    {
        let original_summary = session_result.summary.clone();
        let no_op_summary = no_op::build_no_op_failure_summary(
            session.turn_count,
            elapsed_secs,
            ctx.executor.tool_name(),
            session.description.as_deref(),
            ctx.prompt,
            &original_summary,
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
        // CSA-own gate: a SA-mode no-op (zero useful work) is a real failure.
        result.mark_gate_failure("no-op-exit");
        // Sync tool_state so state.toml agrees with result.toml after rewrite.
        if let Some(tool_state) = session.tools.get_mut(ctx.executor.tool_name()) {
            tool_state.last_exit_code = 1;
            tool_state.last_action_summary = no_op_summary;
        }
    }
    // Worker-blocked gate (#1483): rewrite to failure when output/summary
    // contains "STATUS: BLOCKED" (Bash unavailable, EROFS, missing tooling).
    if result.exit_code == 0
        && blocked::worker_output_indicates_blocked(&result.output, &result.summary)
    {
        let blocked_summary = format!(
            "worker blocked: STATUS: BLOCKED detected; task was not completed. \
             Original summary: {}",
            result.summary,
        );
        warn!(
            session = %session.meta_session_id,
            original_summary = %result.summary,
            "STATUS: BLOCKED in session output — rewriting exit_code to 1"
        );
        session_result.exit_code = 1;
        session_result.status = csa_session::SessionResult::status_from_exit_code(1);
        session_result.summary = blocked_summary.clone();
        // CSA-own gate: worker reported STATUS: BLOCKED — a real failure.
        result.mark_gate_failure("worker-blocked");
        result.summary = blocked_summary.clone();
        if let Some(tool_state) = session.tools.get_mut(ctx.executor.tool_name()) {
            tool_state.last_exit_code = 1;
            tool_state.last_action_summary = blocked_summary;
        }
    }
    if result.exit_code == 0
        && ctx.task_type == Some("run")
        && !result_sidecar::status_is_success(&ctx.session_dir, session.turn_count)
        && session.turn_count <= 1
        && !ctx.has_tool_calls
        && !has_meaningful_reasoning_output
        && ctx.changed_paths.is_empty()
        && elapsed_secs < no_op::ELAPSED_THRESHOLD_SECS
        && let Err(err) = progress::maybe_mark_no_progress_session(
            ctx.project_root,
            session,
            result,
            &mut session_result,
        )
    {
        warn!(
            session = %session.meta_session_id,
            error = %err,
            "Skipping post-session no-progress detection; preserving success status"
        );
    }
    // Finalize the #161 incidental downgrade: if the success survived every
    // false-success gate above (exit still 0, no gate fired), record the raw
    // exit code and warning as diagnostics on the persisted envelope. If a gate
    // re-flagged the session, `mark_gate_failure` already cleared the warning on
    // `result`, so we drop the incidental note here too.
    if let Some((note, raw_exit_code)) = pending_incidental_warning
        && session_result.status == "success"
        && result.csa_gate_failure.is_none()
    {
        session_result.raw_process_exit_code = Some(raw_exit_code);
        session_result.warnings.push(note);
    }
    lefthook::maybe_record_core_hookspath_conflict(result, &mut session_result);
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
    result_sidecar::ensure_turn_scoped_manager_artifact(
        &ctx.session_dir,
        session.turn_count,
        &mut session_result,
    );
    if let Err(e) = crate::session_kill_diagnostics::save_result_with_signal_diagnostic(
        ctx.project_root,
        session,
        ctx.executor.tool_name(),
        &mut session_result,
        result.terminal_reason.as_deref(),
        Some(&mut result.stderr_output),
    ) {
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

#[path = "pipeline_post_exec_blocked.rs"]
mod blocked;

#[cfg(test)]
#[path = "pipeline_tests_post_exec.rs"]
mod tests;

#[cfg(test)]
#[path = "pipeline_tests_token_usage.rs"]
mod token_usage_tests;

#[cfg(test)]
#[path = "pipeline_tests_no_op_gate.rs"]
mod no_op_gate_tests;

#[cfg(test)]
#[path = "pipeline_tests_no_op_gate_2181.rs"]
mod no_op_gate_2181_tests;

#[cfg(test)]
#[path = "pipeline_tests_no_progress.rs"]
mod no_progress_tests;
