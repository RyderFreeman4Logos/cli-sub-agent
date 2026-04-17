//! Fix loop for `csa review --fix`: resumes the review session to apply fixes,
//! then re-gates via quality pipeline after each round.

use std::path::{Path, PathBuf};

use anyhow::Result;
use tracing::{error, info, warn};

use crate::review_routing::ReviewRoutingMetadata;
use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::ToolName;
use csa_session::state::ReviewSessionMeta;

use super::CLEAN;
use super::findings_toml::persist_review_findings_toml;
use super::output::{
    is_review_output_empty, persist_review_meta, persist_review_verdict, sanitize_review_output,
};
use super::resolve::ANTI_RECURSION_PREAMBLE;

/// All context needed to run the fix loop after a review finds issues.
pub(crate) struct FixLoopContext<'a> {
    pub tool: ToolName,
    pub config: Option<&'a ProjectConfig>,
    pub global_config: &'a GlobalConfig,
    pub review_model: Option<String>,
    pub tier_model_spec: Option<String>,
    pub review_thinking: Option<String>,
    pub review_routing: ReviewRoutingMetadata,
    pub stream_mode: csa_process::StreamMode,
    pub idle_timeout_seconds: u64,
    pub initial_response_timeout_seconds: Option<u64>,
    pub force_override_user_config: bool,
    pub force_ignore_tier_setting: bool,
    pub no_failover: bool,
    pub no_fs_sandbox: bool,
    pub extra_writable: &'a [PathBuf],
    pub timeout: Option<u64>,
    pub project_root: &'a Path,
    pub scope: String,
    pub decision: String,
    pub verdict: String,
    pub max_rounds: u8,
    pub initial_session_id: String,
    pub review_iterations: u32,
}

/// Run fix rounds, returning the final exit code.
///
/// Each round resumes the review session with a fix prompt, then re-runs
/// the quality gate.  Returns `Ok(0)` on first passing round, or
/// `Ok(1)` when all rounds are exhausted.
pub(crate) async fn run_fix_loop(ctx: FixLoopContext<'_>) -> Result<i32> {
    let mut session_id = ctx.initial_session_id;

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

        let fix_future = super::execute_review(
            ctx.tool,
            fix_prompt,
            Some(session_id.clone()),
            ctx.review_model.clone(),
            ctx.tier_model_spec.clone(),
            None,
            ctx.review_thinking.clone(),
            format!("fix round {round}/{}", ctx.max_rounds),
            ctx.project_root,
            ctx.config,
            ctx.global_config,
            ctx.review_routing.clone(),
            ctx.stream_mode,
            ctx.idle_timeout_seconds,
            ctx.initial_response_timeout_seconds,
            ctx.force_override_user_config,
            ctx.force_ignore_tier_setting,
            ctx.no_failover,
            ctx.no_fs_sandbox,
            false, // fix pass must write — override readonly_project_root
            ctx.extra_writable,
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

        print!("{}", sanitize_review_output(&fix_result.execution.output));
        let fix_empty = is_review_output_empty(&fix_result.execution.output);
        if fix_empty {
            warn!(
                round,
                "Fix round produced no substantive output — treating as failed"
            );
        }
        session_id = fix_result.meta_session_id.clone();

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

        if gate_passed && !fix_empty {
            info!(round, "Fix round succeeded — quality gate passed");
            let review_meta = ReviewSessionMeta {
                session_id: session_id.clone(),
                head_sha: csa_session::detect_git_head(ctx.project_root).unwrap_or_default(),
                decision: "pass".to_string(),
                verdict: CLEAN.to_string(),
                tool: ctx.tool.to_string(),
                scope: ctx.scope.clone(),
                exit_code: 0,
                fix_attempted: true,
                fix_rounds: u32::from(round),
                review_iterations: ctx.review_iterations,
                timestamp: chrono::Utc::now(),
                diff_fingerprint: None,
            };
            persist_fix_final_artifacts(ctx.project_root, &review_meta);
            return Ok(0);
        }
    }

    // All fix rounds exhausted; gate still fails.
    let review_meta = ReviewSessionMeta {
        session_id,
        head_sha: csa_session::detect_git_head(ctx.project_root).unwrap_or_default(),
        decision: ctx.decision,
        verdict: ctx.verdict,
        tool: ctx.tool.to_string(),
        scope: ctx.scope,
        exit_code: 1,
        fix_attempted: true,
        fix_rounds: u32::from(ctx.max_rounds),
        review_iterations: ctx.review_iterations,
        timestamp: chrono::Utc::now(),
        diff_fingerprint: None,
    };
    persist_fix_final_artifacts(ctx.project_root, &review_meta);
    error!(
        max_rounds = ctx.max_rounds,
        "All fix rounds exhausted — quality gate still failing"
    );
    Ok(1)
}

fn persist_fix_final_artifacts(project_root: &Path, review_meta: &ReviewSessionMeta) {
    persist_review_meta(project_root, review_meta);
    persist_review_verdict(project_root, review_meta, &[], Vec::new());
    persist_review_findings_toml(project_root, review_meta);
}

#[cfg(test)]
#[path = "review_cmd_fix_tests.rs"]
mod tests;
