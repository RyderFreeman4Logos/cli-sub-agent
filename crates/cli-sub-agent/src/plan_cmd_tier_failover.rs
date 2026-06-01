use std::time::Instant;

use anyhow::Result;
use tracing::{info, warn};

use csa_core::types::ToolName;

use super::plan_cmd_exec::{
    CsaStepExecutionOptions, StepExecutionOutcome, execute_csa_step, run_with_heartbeat,
};
use super::plan_cmd_steps::StepExecutionContext;
use crate::tier_model_fallback::{
    TierAttemptFailure, classify_next_model_failure_with_elapsed, format_all_models_failed_reason,
    ordered_tier_candidates,
};

pub(super) struct TierFailoverParams<'a> {
    pub(super) initial_tool: &'a ToolName,
    pub(super) initial_model_spec: Option<&'a str>,
    pub(super) tier_name: Option<&'a str>,
    pub(super) forwarded_session: Option<&'a str>,
    pub(super) readonly_project_root: bool,
}

/// Execute a CSA step with tier-level failover.
///
/// When a tier is configured, builds the ordered candidate list and tries each
/// model in sequence. On a failover-eligible failure (HTTP 400/429, rate limit,
/// etc.), advances to the next model in the tier. Without a tier, behaves
/// identically to a single `execute_csa_step` call.
pub(super) async fn execute_csa_step_with_tier_failover(
    label: &str,
    prompt: &str,
    params: &TierFailoverParams<'_>,
    step_ctx: &StepExecutionContext<'_>,
    step_started_at: Instant,
) -> Result<StepExecutionOutcome> {
    let global_config = csa_config::GlobalConfig::load().ok();
    let candidates = ordered_tier_candidates(
        *params.initial_tool,
        params.initial_model_spec,
        params.tier_name,
        step_ctx.config,
        global_config.as_ref(),
        params.tier_name.is_some(),
        &[],
    );

    let mut failures: Vec<TierAttemptFailure> = Vec::new();

    for (idx, (tool, model_spec)) in candidates.iter().enumerate() {
        let attempt_start = Instant::now();
        let is_fallback = idx > 0;
        if is_fallback {
            let spec_label = model_spec.as_deref().unwrap_or(tool.as_str());
            info!(
                "{label} - Tier failover: trying {spec_label} (candidate {}/{})…",
                idx + 1,
                candidates.len()
            );
            eprintln!(
                "{label} - TIER FAILOVER → {spec_label} ({}/{})",
                idx + 1,
                candidates.len()
            );
        }

        let result = run_with_heartbeat(
            label,
            execute_csa_step(
                label,
                prompt,
                tool,
                step_ctx.project_root,
                step_ctx.config,
                CsaStepExecutionOptions {
                    model_spec: model_spec.as_deref(),
                    forwarded_session: if is_fallback {
                        None
                    } else {
                        params.forwarded_session
                    },
                    no_fs_sandbox: step_ctx.no_fs_sandbox,
                    readonly_project_root: params.readonly_project_root,
                },
            ),
            step_started_at,
        )
        .await;

        match result {
            Ok(outcome) if outcome.exit_code == 0 => return Ok(outcome),
            Ok(outcome) => {
                let elapsed = attempt_start.elapsed();
                let detected = classify_next_model_failure_with_elapsed(
                    tool.as_str(),
                    &outcome.stderr,
                    &outcome.output,
                    outcome.exit_code,
                    model_spec.as_deref(),
                    Some(elapsed),
                );
                let spec_label = model_spec.as_deref().unwrap_or(tool.as_str());
                if let Some(rate_limit) = detected
                    && idx + 1 < candidates.len()
                {
                    warn!(
                        "{label} - {spec_label} failed ({}); advancing to next tier model",
                        rate_limit.reason
                    );
                    eprintln!("{label} - {spec_label} FAILOVER ({})", rate_limit.reason);
                    failures.push(TierAttemptFailure::from_rate_limit(
                        spec_label.to_string(),
                        &rate_limit,
                    ));
                    continue;
                }
                if let Some(reason) = format_all_models_failed_reason(params.tier_name, &failures) {
                    warn!("{label} - {reason}");
                }
                return Ok(outcome);
            }
            Err(err) => {
                if idx + 1 < candidates.len() {
                    let spec_label = model_spec.as_deref().unwrap_or(tool.as_str());
                    warn!("{label} - {spec_label} error: {err}; trying next tier model");
                    // Spawn/transport error before any RateLimitDetected: the
                    // scheduler has no authoritative quota signal here, so leave
                    // `quota_exhausted` unset and let the failover trace classify
                    // the reason string (build-time path, unchanged).
                    failures.push(TierAttemptFailure {
                        model_spec: spec_label.to_string(),
                        reason: format!("error: {err}"),
                        quota_exhausted: None,
                    });
                    continue;
                }
                return Err(err);
            }
        }
    }

    unreachable!("candidates list is never empty")
}
