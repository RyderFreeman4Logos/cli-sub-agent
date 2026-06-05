//! Fix loop for `csa review --fix`: resumes the review session to apply fixes,
//! then re-gates via quality pipeline after each round.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use tracing::{error, info, warn};

use crate::bug_class::{CONSOLIDATED_REVIEW_ARTIFACT_FILE, SINGLE_REVIEW_ARTIFACT_FILE};
use crate::review_routing::ReviewRoutingMetadata;
use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::{ReviewDecision, ToolName};
use csa_session::{
    FindingsFile, FixConvergenceMeta, ReviewVerdictArtifact, state::ReviewSessionMeta,
    write_findings_toml,
};

use super::CLEAN;
use super::output::{is_review_output_empty, sanitize_review_output};
use super::resolve::ANTI_RECURSION_PREAMBLE;

#[path = "review_cmd_fix_clean_output.rs"]
mod clean_output;
#[path = "review_cmd_fix_convergence.rs"]
mod convergence;
#[path = "review_cmd_fix_diff_report.rs"]
mod diff_report_artifacts;
#[path = "review_cmd_fix_noop.rs"]
mod noop;
use convergence::{
    FixTerminalOutcome, fix_exit_code_for_convergence, pre_verdict_non_convergence_reason,
};
#[cfg(test)]
pub(crate) use diff_report_artifacts::{
    persist_fix_final_artifacts_for_tests, persist_fix_final_artifacts_for_tests_with_noop_probe,
    persist_fix_final_artifacts_for_tests_with_output,
    persist_fix_final_artifacts_for_tests_with_output_and_diff_report,
};
use noop::{FixNoOpProbe, apply_fix_loop_noop_signal, is_fix_loop_noop_failure_reason};

/// Context for review fix-loop execution.
pub(crate) struct FixLoopContext<'a> {
    pub effective_tool: ToolName,
    pub config: Option<&'a ProjectConfig>,
    pub global_config: &'a GlobalConfig,
    pub review_model: Option<String>,
    pub effective_tier_model_spec: Option<String>,
    pub review_thinking: Option<String>,
    pub review_routing: ReviewRoutingMetadata,
    pub stream_mode: csa_process::StreamMode,
    pub idle_timeout_seconds: u64,
    pub initial_response_timeout_seconds: Option<u64>,
    pub force_override_user_config: bool,
    pub force_ignore_tier_setting: bool,
    pub no_failover: bool,
    pub build_jobs: Option<u32>,
    pub fast_but_more_cost: bool,
    pub no_fs_sandbox: bool,
    /// #1652 scan override (#1745, #1847).
    pub error_marker_scan_override: Option<bool>,
    pub extra_writable: &'a [PathBuf],
    pub extra_readable: &'a [PathBuf],
    pub timeout: Option<u64>,
    pub diff_report: super::diff_size::ReviewDiffReport<'a>,
    pub project_root: &'a Path,
    pub scope: String,
    pub decision: String,
    pub verdict: String,
    /// Review mode that produced this verdict ("standard" or "red-team"), #1817.
    pub review_mode: Option<String>,
    pub max_rounds: u8,
    pub initial_session_id: String,
    pub review_iterations: u32,
    pub current_depth: u32,
    pub startup_env: &'a crate::startup_env::StartupSubtreeEnv,
}

/// Run fix rounds and return the final exit code.
///
/// Each round resumes the review session with a fix prompt, then re-runs
/// the quality gate. Returns `Ok(0)` only after the quality gate passes and
/// the persisted final review decision is clean; all other terminal outcomes
/// return `Ok(1)` or an error.
pub(crate) async fn run_fix_loop(ctx: FixLoopContext<'_>) -> Result<i32> {
    let mut session_id = ctx.initial_session_id;
    let diff_report = ctx.diff_report;
    let noop_probe = FixNoOpProbe::capture(ctx.project_root);
    // Entering the fix loop means the current review is not clean; any existing
    // marker for this SHA is stale until genuine clean convergence rewrites it.
    remove_review_gate_marker_for_current_head(ctx.project_root, &session_id);
    let mut last_fix_output: Option<String> = None;
    let mut last_gate_passed = false;
    let gate_env = crate::build_jobs_env::build_jobs_env(ctx.build_jobs);

    for round in 1..=ctx.max_rounds {
        info!(round, max_rounds = ctx.max_rounds, session_id = %session_id, "Fix round starting");

        let fix_prompt = format!(
            "{ANTI_RECURSION_PREAMBLE}\
             Fix round {round}/{}.\n\
             Fix all issues found in the review. Run formatting and linting commands as needed.\n\
             After applying fixes, verify the changes compile and pass basic checks.\n\
             If no issues remain, emit verdict: CLEAN.",
            ctx.max_rounds,
        );

        let fix_future = super::execute_review_with_tier_filter(
            ctx.effective_tool,
            fix_prompt,
            Some(session_id.clone()),
            ctx.review_model.clone(),
            ctx.effective_tier_model_spec.clone(),
            None,
            false,
            Vec::new(),
            ctx.review_thinking.clone(),
            format!("fix round {round}/{}", ctx.max_rounds),
            ctx.project_root,
            ctx.config,
            ctx.global_config,
            None,
            ctx.review_routing.clone(),
            ctx.stream_mode,
            ctx.idle_timeout_seconds,
            ctx.initial_response_timeout_seconds,
            ctx.force_override_user_config,
            ctx.force_ignore_tier_setting,
            ctx.no_failover,
            ctx.build_jobs,
            ctx.fast_but_more_cost,
            false,
            ctx.no_fs_sandbox,
            false, // fix pass must write — override readonly_project_root
            ctx.extra_writable,
            ctx.extra_readable,
            ctx.error_marker_scan_override,
            ctx.current_depth,
            ctx.startup_env,
        );

        let fix_result = if let Some(timeout_secs) = ctx.timeout {
            match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), fix_future)
                .await
            {
                Ok(inner) => inner?,
                Err(_) => {
                    error!(
                        timeout_secs = timeout_secs,
                        round, "Fix round aborted: wall-clock timeout exceeded"
                    );
                    anyhow::bail!(
                        "Fix round {round}/{} aborted: --timeout {timeout_secs}s exceeded.",
                        ctx.max_rounds,
                    );
                }
            }
        } else {
            fix_future.await?
        };

        let fix_output = fix_result.execution.execution.output.clone();
        print!("{}", sanitize_review_output(&fix_output));
        let fix_empty = is_review_output_empty(&fix_output);
        if fix_empty {
            warn!(
                round,
                "Fix round produced no substantive output — treating as failed"
            );
        }
        last_fix_output = Some(fix_output.clone());
        session_id = fix_result.execution.meta_session_id.clone();

        // Run the quality gate after fix.
        let fix_gate_steps = ctx.global_config.review.effective_gate_steps();
        let fix_gate_timeout = ctx
            .config
            .and_then(|c| c.review.as_ref())
            .map(|r| r.gate_timeout_secs)
            .unwrap_or_else(csa_config::ReviewConfig::default_gate_timeout);
        let fix_gate_mode = &ctx.global_config.review.gate_mode;

        let gate_passed = if fix_gate_steps.is_empty() {
            let gate_command = ctx
                .config
                .and_then(|c| c.review.as_ref())
                .and_then(|r| r.gate_command.as_deref());
            let gate_result = crate::pipeline::gate::evaluate_quality_gate(
                ctx.project_root,
                gate_command,
                fix_gate_timeout,
                fix_gate_mode,
                ctx.current_depth,
                gate_env.as_ref(),
            )
            .await?;

            if !gate_result.passed() {
                warn!(
                    round,
                    max_rounds = ctx.max_rounds,
                    command = %gate_result.command,
                    exit_code = ?gate_result.exit_code,
                    "Quality gate still failing after fix round"
                );
            }
            gate_result.passed()
        } else {
            let pipeline_result = crate::pipeline::gate::evaluate_quality_gates(
                ctx.project_root,
                &fix_gate_steps,
                fix_gate_timeout,
                fix_gate_mode,
                ctx.current_depth,
                gate_env.as_ref(),
            )
            .await?;

            if !pipeline_result.passed {
                warn!(
                    round,
                    max_rounds = ctx.max_rounds,
                    failed_step = ?pipeline_result.failed_step,
                    "Quality gate pipeline still failing after fix round"
                );
            }
            pipeline_result.passed
        };
        last_gate_passed = gate_passed;

        if gate_passed && !fix_empty {
            let review_meta = ReviewSessionMeta {
                session_id: session_id.clone(),
                head_sha: csa_session::detect_git_head(ctx.project_root).unwrap_or_default(),
                decision: "pass".to_string(),
                verdict: CLEAN.to_string(),
                review_mode: ctx.review_mode.clone(),
                status_reason: None,
                routed_to: None,
                primary_failure: None,
                failure_reason: None,
                tool: ctx.effective_tool.to_string(),
                scope: ctx.scope.clone(),
                exit_code: 0,
                fix_attempted: true,
                fix_rounds: u32::from(round),
                review_iterations: ctx.review_iterations,
                timestamp: chrono::Utc::now(),
                diff_fingerprint: None,
                fix_convergence: None,
            };
            let final_decision = persist_fix_final_artifacts_with_current_output(
                ctx.project_root,
                &review_meta,
                true,
                Some(&fix_output),
                diff_report,
                Some(&noop_probe),
            );
            if final_decision == ReviewDecision::Pass {
                info!(
                    round,
                    "Fix round succeeded — quality gate and verdict consistency passed"
                );
            } else {
                warn!(
                    round,
                    decision = final_decision.as_str(),
                    "Final verdict is non-clean"
                );
            }
            return Ok(fix_exit_code_for_convergence(true, true, final_decision));
        }
    }

    // All fix rounds exhausted; gate still fails.
    let review_meta = ReviewSessionMeta {
        session_id,
        head_sha: csa_session::detect_git_head(ctx.project_root).unwrap_or_default(),
        decision: ctx.decision,
        verdict: ctx.verdict,
        review_mode: ctx.review_mode.clone(),
        status_reason: None,
        routed_to: None,
        primary_failure: None,
        failure_reason: None,
        tool: ctx.effective_tool.to_string(),
        scope: ctx.scope,
        exit_code: 1,
        fix_attempted: true,
        fix_rounds: u32::from(ctx.max_rounds),
        review_iterations: ctx.review_iterations,
        timestamp: chrono::Utc::now(),
        diff_fingerprint: None,
        fix_convergence: None,
    };
    let final_decision = persist_fix_final_artifacts_with_current_output(
        ctx.project_root,
        &review_meta,
        last_gate_passed,
        last_fix_output.as_deref(),
        diff_report,
        Some(&noop_probe),
    );
    error!(
        max_rounds = ctx.max_rounds,
        "All fix rounds exhausted — quality gate still failing"
    );
    Ok(fix_exit_code_for_convergence(
        last_gate_passed,
        last_fix_output
            .as_deref()
            .map(|output| !is_review_output_empty(output))
            .unwrap_or(false),
        final_decision,
    ))
}

#[cfg(test)]
fn persist_fix_final_artifacts(
    project_root: &Path,
    review_meta: &ReviewSessionMeta,
    quality_gate_passed: bool,
) -> ReviewDecision {
    persist_fix_final_artifacts_with_current_output(
        project_root,
        review_meta,
        quality_gate_passed,
        None,
        diff_report_artifacts::empty_diff_report(),
        None,
    )
}

fn persist_fix_final_artifacts_with_current_output(
    project_root: &Path,
    review_meta: &ReviewSessionMeta,
    quality_gate_passed: bool,
    current_fix_output: Option<&str>,
    diff_report: super::diff_size::ReviewDiffReport<'_>,
    noop_probe: Option<&FixNoOpProbe>,
) -> ReviewDecision {
    let fix_output_was_substantive = current_fix_output
        .map(|output| !is_review_output_empty(output))
        .unwrap_or(quality_gate_passed);
    let mut meta_for_verdict = review_meta.clone();
    if let Some(reason) =
        pre_verdict_non_convergence_reason(quality_gate_passed, fix_output_was_substantive)
    {
        meta_for_verdict.decision = ReviewDecision::Fail.as_str().to_string();
        meta_for_verdict.verdict = "HAS_ISSUES".to_string();
        meta_for_verdict.exit_code = 1;
        meta_for_verdict.failure_reason = Some(format!("fix_non_convergence:{reason}"));
        meta_for_verdict.fix_convergence = Some(FixConvergenceMeta {
            quality_gate_passed,
            fix_output_was_substantive,
            post_consistency_decision: ReviewDecision::Fail.as_str().to_string(),
            reached_genuine_clean_convergence: false,
            terminal_reason: reason.to_string(),
        });
    }

    diff_report_artifacts::persist_fix_review_meta(project_root, &meta_for_verdict, diff_report);
    if quality_gate_passed && fix_output_was_substantive {
        match csa_session::get_session_dir(project_root, &meta_for_verdict.session_id) {
            Ok(session_dir) => {
                if let Err(error) = write_findings_toml(&session_dir, &FindingsFile::default()) {
                    warn!(
                        session_id = %meta_for_verdict.session_id,
                        error = %error,
                        "Failed to write output/findings.toml after CLEAN convergence"
                    );
                }
                // Clear stale review JSON artifacts so the verdict loader does
                // not override the cleanly-converged empty findings.toml.
                for artifact_file in [
                    SINGLE_REVIEW_ARTIFACT_FILE,
                    CONSOLIDATED_REVIEW_ARTIFACT_FILE,
                ] {
                    let stale_artifact = session_dir.join(artifact_file);
                    if let Err(error) = std::fs::remove_file(&stale_artifact)
                        && error.kind() != std::io::ErrorKind::NotFound
                    {
                        warn!(
                            session_id = %meta_for_verdict.session_id,
                            artifact_file,
                            error = %error,
                            "Failed to remove stale review artifact after CLEAN convergence"
                        );
                    }
                }
                clean_output::clear_clean_convergence_fail_signals(
                    &session_dir,
                    &meta_for_verdict.session_id,
                    current_fix_output,
                );
            }
            Err(error) => {
                warn!(
                    session_id = %meta_for_verdict.session_id,
                    error = %error,
                    "Cannot resolve session dir for final fix artifacts"
                );
            }
        }
    }
    remove_stale_review_verdict(project_root, &meta_for_verdict);
    diff_report_artifacts::persist_fix_review_verdict(project_root, &meta_for_verdict, diff_report);
    let final_verdict = read_persisted_fix_final_verdict(project_root, &meta_for_verdict);
    let outcome = FixTerminalOutcome::new(
        quality_gate_passed,
        fix_output_was_substantive,
        final_verdict.decision,
    );
    let final_meta = review_meta_for_final_verdict(&meta_for_verdict, &final_verdict, &outcome);
    let final_meta = apply_fix_loop_noop_signal(project_root, final_meta, &outcome, noop_probe);
    diff_report_artifacts::persist_fix_review_meta(project_root, &final_meta, diff_report);
    if final_meta
        .failure_reason
        .as_deref()
        .is_some_and(is_fix_loop_noop_failure_reason)
    {
        diff_report_artifacts::persist_fix_review_verdict(project_root, &final_meta, diff_report);
    }
    super::diff_size::persist_review_diff_size_headers(
        project_root,
        &final_meta.session_id,
        diff_report.diff_size,
    );
    if outcome.reached_genuine_clean_convergence() {
        crate::review_gate::maybe_write_review_gate_marker(
            project_root,
            &final_meta.head_sha,
            &final_meta.session_id,
            &final_meta.scope,
            final_meta.review_mode.as_deref(),
        );
    } else {
        remove_review_gate_marker_for_head(project_root, &final_meta);
    }
    super::post_review::persist_review_failure_suggestion(project_root, &final_meta);
    outcome.post_consistency_decision
}

struct FinalFixVerdict {
    decision: ReviewDecision,
    verdict_legacy: String,
}

fn read_persisted_fix_final_verdict(
    project_root: &Path,
    review_meta: &ReviewSessionMeta,
) -> FinalFixVerdict {
    let session_dir = match csa_session::get_session_dir(project_root, &review_meta.session_id) {
        Ok(session_dir) => session_dir,
        Err(error) => {
            warn!(
                session_id = %review_meta.session_id, error = %error, "Cannot resolve verdict dir"
            );
            return fail_closed_final_fix_verdict();
        }
    };
    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let raw = match fs::read_to_string(&verdict_path) {
        Ok(raw) => raw,
        Err(error) => {
            warn!(
                session_id = %review_meta.session_id, path = %verdict_path.display(), error = %error,
                "Cannot read final verdict"
            );
            return fail_closed_final_fix_verdict();
        }
    };
    match serde_json::from_str::<ReviewVerdictArtifact>(&raw) {
        Ok(artifact) => FinalFixVerdict {
            decision: artifact.decision,
            verdict_legacy: artifact.verdict_legacy,
        },
        Err(error) => {
            warn!(
                session_id = %review_meta.session_id, path = %verdict_path.display(), error = %error,
                "Cannot parse final verdict"
            );
            fail_closed_final_fix_verdict()
        }
    }
}

fn fail_closed_final_fix_verdict() -> FinalFixVerdict {
    FinalFixVerdict {
        decision: ReviewDecision::Uncertain,
        verdict_legacy: "UNCERTAIN".to_string(),
    }
}

fn review_meta_for_final_verdict(
    review_meta: &ReviewSessionMeta,
    final_verdict: &FinalFixVerdict,
    outcome: &FixTerminalOutcome,
) -> ReviewSessionMeta {
    let mut final_meta = review_meta.clone();
    let persisted_decision = if outcome.pre_verdict_non_converged() {
        ReviewDecision::Fail
    } else {
        final_verdict.decision
    };
    final_meta.decision = persisted_decision.as_str().to_string();
    final_meta.verdict = if persisted_decision == ReviewDecision::Fail {
        "HAS_ISSUES".to_string()
    } else {
        final_verdict.verdict_legacy.clone()
    };
    final_meta.exit_code = outcome.exit_code();
    final_meta.fix_convergence = Some(outcome.fix_convergence_meta());
    if outcome.pre_verdict_non_converged() {
        final_meta.failure_reason =
            Some(format!("fix_non_convergence:{}", outcome.terminal_reason));
    } else if outcome.reached_genuine_clean_convergence() {
        final_meta.failure_reason = None;
    }
    final_meta
}

fn remove_stale_review_verdict(project_root: &Path, review_meta: &ReviewSessionMeta) {
    let Ok(session_dir) = csa_session::get_session_dir(project_root, &review_meta.session_id)
    else {
        return;
    };
    let verdict_path = session_dir.join("output").join("review-verdict.json");
    if let Err(error) = fs::remove_file(&verdict_path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        warn!(
            session_id = %review_meta.session_id, path = %verdict_path.display(), error = %error,
            "Cannot remove stale verdict"
        );
    }
}

fn remove_review_gate_marker_for_head(project_root: &Path, review_meta: &ReviewSessionMeta) {
    crate::review_gate::remove_review_gate_marker_for_head(
        project_root,
        &review_meta.head_sha,
        Some(&review_meta.session_id),
    );
}

fn remove_review_gate_marker_for_current_head(project_root: &Path, session_id: &str) {
    let head_sha = csa_session::detect_git_head(project_root).unwrap_or_default();
    crate::review_gate::remove_review_gate_marker_for_head(
        project_root,
        &head_sha,
        Some(session_id),
    );
}

#[cfg(test)]
#[path = "review_cmd_fix_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "review_cmd_fix_convergence_tests.rs"]
mod convergence_tests;
