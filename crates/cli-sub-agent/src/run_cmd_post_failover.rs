//! Rate-limit + transport-error failover evaluation for `csa run`.
//!
//! Extracted from `run_cmd_post.rs` for module-size hygiene. Logic and public
//! surface are unchanged; this is a pure relocation of the failover helpers
//! and the `evaluate_*_failover` entry points.

use std::{path::Path, time::Duration};

use anyhow::Result;
use tracing::{info, warn};

use csa_config::ProjectConfig;
use csa_core::types::{FallbackAttempt, ModelFamily, ToolName, provider_for_tool_name};
use csa_scheduler::FallbackChain;

#[path = "run_cmd_post_failover_availability.rs"]
mod failover_availability;
use failover_availability::decide_available_failover;

/// Outcome of rate-limit failover evaluation.
pub(crate) enum RateLimitAction {
    /// No rate limit detected; break with result.
    NoRateLimit,
    /// Rate limit detected but no failover possible; break with result.
    ExhaustedFailovers { reason: String },
    /// Retry with a different tool.
    Retry {
        new_tool: ToolName,
        new_model_spec: Option<String>,
    },
}

#[derive(Clone, Copy)]
enum TransportErrorFailoverKind {
    RateLimit,
    AcpCrashRetryExhausted,
    GeminiRetryChainExhausted,
    GeminiLegacyInitialStall,
}

struct TransportErrorFailoverSignal {
    kind: TransportErrorFailoverKind,
    matched_pattern: String,
    reason: String,
    quota_exhausted: bool,
    requires_init_failure_window: bool,
}

pub(crate) fn format_tool_exhausted_summary(tool_name: &str, matched_pattern: &str) -> String {
    format!(
        "tool_exhausted: {tool_name} permanent quota exhaustion detected \
         (matched '{matched_pattern}'); no retry or tool fallback attempted. \
         Inspect the tool account billing/quota or choose another tool explicitly."
    )
}

pub(crate) fn detect_permanent_tool_exhaustion_result(
    tool_name_str: &str,
    exec_result: &csa_process::ExecutionResult,
    current_model_spec: Option<&str>,
) -> Option<csa_scheduler::RateLimitDetected> {
    // Only stderr_output is the provider's error channel; `summary`/`output`
    // are agent stdout (reviewed/echoed content) and must not drive a permanent
    // quota verdict (#1736).
    detect_permanent_tool_exhaustion_text(
        tool_name_str,
        &exec_result.stderr_output,
        exec_result.exit_code,
        current_model_spec,
    )
}

pub(crate) fn detect_permanent_tool_exhaustion_text(
    tool_name_str: &str,
    provider_error_channel: &str,
    exit_code: i32,
    current_model_spec: Option<&str>,
) -> Option<csa_scheduler::RateLimitDetected> {
    if exit_code == 0 {
        return None;
    }
    // Permanent self-kill must come only from the provider error channel, never
    // agent stdout/summary that may quote reviewed quota text (#1736).
    csa_scheduler::detect_rate_limit(
        tool_name_str,
        provider_error_channel,
        "",
        exit_code,
        current_model_spec,
    )
    .filter(|detected| detected.quota_exhausted)
    .filter(|detected| {
        is_provider_wide_quota_exhaustion(
            tool_name_str,
            detected.quota_exhausted,
            provider_error_channel,
        )
    })
}

pub(crate) fn is_permanent_tool_exhaustion_error(
    tool_name_str: &str,
    error_message: &str,
    current_model_spec: Option<&str>,
) -> bool {
    detect_transport_error_failover_signal(tool_name_str, error_message, current_model_spec)
        .is_some_and(|signal| signal.quota_exhausted)
}

fn detect_transport_error_failover_signal(
    tool_name_str: &str,
    error_message: &str,
    current_model_spec: Option<&str>,
) -> Option<TransportErrorFailoverSignal> {
    let error_lower = error_message.to_ascii_lowercase();

    if error_lower.contains("acp crash retry exhausted")
        || error_lower.contains("crash retry exhausted")
    {
        let matched_pattern = if error_lower.contains("acp crash retry exhausted") {
            "acp crash retry exhausted"
        } else {
            "crash retry exhausted"
        };
        return Some(TransportErrorFailoverSignal {
            kind: TransportErrorFailoverKind::AcpCrashRetryExhausted,
            matched_pattern: matched_pattern.to_string(),
            reason: "acp_crash_retry_exhausted".to_string(),
            quota_exhausted: false,
            requires_init_failure_window: false,
        });
    }

    if error_lower.contains("gemini acp retry chain exhausted")
        || error_lower.contains("retry chain exhausted")
    {
        let matched_pattern = if error_lower.contains("gemini acp retry chain exhausted") {
            "gemini acp retry chain exhausted"
        } else {
            "retry chain exhausted"
        };
        return Some(TransportErrorFailoverSignal {
            kind: TransportErrorFailoverKind::GeminiRetryChainExhausted,
            matched_pattern: matched_pattern.to_string(),
            reason: "gemini_retry_chain_exhausted".to_string(),
            quota_exhausted: csa_core::gemini::detect_permanent_quota_exhaustion_pattern(
                error_message,
            )
            .is_some(),
            requires_init_failure_window: false,
        });
    }

    if tool_name_str == "gemini-cli" && error_lower.contains("gemini_legacy_initial_stall") {
        return Some(TransportErrorFailoverSignal {
            kind: TransportErrorFailoverKind::GeminiLegacyInitialStall,
            matched_pattern: "gemini_legacy_initial_stall".to_string(),
            reason: "gemini_legacy_initial_stall".to_string(),
            quota_exhausted: false,
            requires_init_failure_window: false,
        });
    }

    csa_scheduler::detect_rate_limit(
        tool_name_str,
        error_message,
        "",
        1, // synthetic non-zero exit code
        current_model_spec,
    )
    .map(|rate_limit| {
        let requires_init_failure_window = csa_scheduler::requires_init_failure_window(&rate_limit);
        TransportErrorFailoverSignal {
            kind: TransportErrorFailoverKind::RateLimit,
            matched_pattern: rate_limit.matched_pattern,
            reason: rate_limit.reason,
            quota_exhausted: rate_limit.quota_exhausted,
            requires_init_failure_window,
        }
    })
}

fn allows_init_failure_failover(
    tool_name: &str,
    reason: &str,
    requires_init_failure_window: bool,
    attempt_elapsed: Option<Duration>,
) -> bool {
    if !requires_init_failure_window {
        return true;
    }
    let Some(elapsed) = attempt_elapsed else {
        return true;
    };
    if csa_scheduler::within_init_failure_window(elapsed) {
        warn!(
            tool = %tool_name,
            reason = %reason,
            elapsed_ms = elapsed.as_millis(),
            "[csa-failover] {tool_name} failed with {reason}, falling back to next tier model"
        );
        return true;
    }
    warn!(
        tool = %tool_name,
        reason = %reason,
        elapsed_ms = elapsed.as_millis(),
        "[csa-failover] HTTP failure occurred after initialization window; not attempting automatic tier fallback"
    );
    false
}

fn is_provider_wide_quota_exhaustion(
    tool_name_str: &str,
    quota_exhausted: bool,
    provider_error_channel: &str,
) -> bool {
    quota_exhausted && !is_codex_model_scoped_usage_limit(tool_name_str, provider_error_channel)
}

fn is_codex_model_scoped_usage_limit(tool_name_str: &str, provider_error_channel: &str) -> bool {
    if tool_name_str != "codex" {
        return false;
    }

    let lower = provider_error_channel.to_ascii_lowercase();
    lower.contains("usage limit") && lower.contains("switch to another model")
}

/// Check for 429 rate-limit signals and decide whether to failover.
///
/// Returns `RateLimitAction` to drive `continue`/`break` in the caller loop.
/// Appends a `FallbackAttempt` to `fallback_chain` when a retry is triggered.
///
/// `resolved_tier_name` and `tried_specs` enable intra-tier failover: when the
/// caller is running under a named tier, we pass spec-level exclusion so that
/// a different model within the same tier can be selected.
#[allow(clippy::too_many_arguments)]
pub(crate) fn evaluate_rate_limit_failover(
    tool_name_str: &str,
    exec_result: &csa_process::ExecutionResult,
    attempts: usize,
    max_failover_attempts: usize,
    tried_tools: &mut Vec<String>,
    tried_specs: &mut Vec<String>,
    tier_auto_select: bool,
    resolved_tier_name: Option<&str>,
    executed_session_id: Option<&str>,
    effective_session_arg: Option<&str>,
    ephemeral: bool,
    prompt_text: &str,
    project_root: &Path,
    config: Option<&ProjectConfig>,
    task_needs_edit: Option<bool>,
    current_model_spec: Option<&str>,
    fallback_chain: &mut FallbackChain,
    attempt_elapsed: Option<Duration>,
) -> Result<RateLimitAction> {
    let rate_limit = match csa_scheduler::detect_rate_limit(
        tool_name_str,
        &exec_result.stderr_output,
        &format!("{}\n{}", exec_result.summary, exec_result.output),
        exec_result.exit_code,
        current_model_spec,
    ) {
        Some(rl) => rl,
        None => return Ok(RateLimitAction::NoRateLimit),
    };

    if !allows_init_failure_failover(
        tool_name_str,
        &rate_limit.reason,
        csa_scheduler::requires_init_failure_window(&rate_limit),
        attempt_elapsed,
    ) {
        return Ok(RateLimitAction::NoRateLimit);
    }

    if !tier_auto_select {
        return Ok(RateLimitAction::NoRateLimit);
    }

    info!(
        tool = %tool_name_str,
        pattern = %rate_limit.matched_pattern,
        quota_exhausted = rate_limit.quota_exhausted,
        attempt = attempts,
        max = max_failover_attempts,
        "Rate limit detected, attempting failover"
    );

    if attempts >= max_failover_attempts {
        warn!(
            "Max failover attempts ({}) reached, returning error",
            max_failover_attempts
        );
        return Ok(RateLimitAction::ExhaustedFailovers {
            reason: format!("max failover attempts ({max_failover_attempts}) reached"),
        });
    }

    tried_tools.push(tool_name_str.to_string());
    if let Some(spec) = current_model_spec {
        tried_specs.push(spec.to_string());
    }

    // Prefer the actually-executed session (important for forks where
    // effective_session_arg starts as None) so decide_failover evaluates
    // the fork session's context, not the parent session.
    let failover_session_ref = executed_session_id.or(effective_session_arg);
    let session_state = if !ephemeral {
        failover_session_ref.and_then(|sid| {
            let sessions_dir = csa_session::get_session_root(project_root)
                .ok()?
                .join("sessions");
            let resolved_id = csa_session::resolve_session_prefix(&sessions_dir, sid).ok()?;
            csa_session::load_session(project_root, &resolved_id).ok()
        })
    } else {
        None
    };

    let task_needs_edit = task_needs_edit
        .or_else(|| crate::run_helpers::infer_task_edit_requirement(prompt_text))
        .or_else(|| config.map(|cfg| cfg.can_tool_edit_existing(tool_name_str)));

    let Some(cfg) = config else {
        return Ok(RateLimitAction::ExhaustedFailovers {
            reason: "project config unavailable; cannot resolve tier fallback candidates"
                .to_string(),
        });
    };

    let provider_wide_quota_exhaustion = is_provider_wide_quota_exhaustion(
        tool_name_str,
        rate_limit.quota_exhausted,
        &exec_result.stderr_output,
    );

    // Provider-wide quota skips shared quota pools (#1629); Codex model-scoped
    // limits must still allow another `codex/...` tier candidate (#1985).
    let exhausted_providers = collect_exhausted_providers(
        fallback_chain,
        Some(tool_name_str).filter(|_| provider_wide_quota_exhaustion),
    );

    let action = decide_available_failover(
        tool_name_str,
        "default",
        resolved_tier_name,
        task_needs_edit,
        session_state.as_ref(),
        tried_tools,
        tried_specs,
        &exhausted_providers,
        cfg,
        &rate_limit.matched_pattern,
    )?;

    match action {
        RateLimitAction::Retry {
            new_tool,
            new_model_spec,
        } => {
            warn!(
                from_tool = %tool_name_str,
                from_spec = %current_model_spec.unwrap_or("none"),
                to_tool = %new_tool.as_str(),
                to_spec = %new_model_spec.as_deref().unwrap_or("none"),
                quota_exhausted = rate_limit.quota_exhausted,
                reason = %rate_limit.reason,
                "[csa-failover] intra-tier failover"
            );
            fallback_chain.push(FallbackAttempt {
                tool: tool_name_str.to_string(),
                model_spec: current_model_spec.map(String::from),
                skip_reason: rate_limit.matched_pattern.clone(),
                quota_exhausted: provider_wide_quota_exhaustion,
                timestamp: chrono::Utc::now(),
            });
            Ok(RateLimitAction::Retry {
                new_tool,
                new_model_spec,
            })
        }
        RateLimitAction::ExhaustedFailovers { reason } => {
            warn!(
                reason = %reason,
                quota_exhausted = rate_limit.quota_exhausted,
                "Failover not possible, returning original result"
            );
            // Record only provider-wide quota exhaustion as permanent pool state.
            if provider_wide_quota_exhaustion {
                fallback_chain.push(FallbackAttempt {
                    tool: tool_name_str.to_string(),
                    model_spec: current_model_spec.map(String::from),
                    skip_reason: rate_limit.matched_pattern.clone(),
                    quota_exhausted: true,
                    timestamp: chrono::Utc::now(),
                });
            }
            Ok(RateLimitAction::ExhaustedFailovers { reason })
        }
        RateLimitAction::NoRateLimit => Ok(RateLimitAction::NoRateLimit),
    }
}

/// Compute the set of provider quota pools that are known exhausted, based on
/// the prior `fallback_chain` entries (any entry with `quota_exhausted=true`)
/// plus an optional "current failure" tool whose quota exhaustion has just
/// been detected but not yet appended to the chain.
fn collect_exhausted_providers(
    fallback_chain: &FallbackChain,
    current_failure_tool: Option<&str>,
) -> Vec<ModelFamily> {
    let mut providers: Vec<ModelFamily> = Vec::new();
    let mut add = |tool: &str| {
        if let Some(provider) = provider_for_tool_name(tool)
            && !providers.contains(&provider)
        {
            providers.push(provider);
        }
    };
    for attempt in fallback_chain {
        if attempt.quota_exhausted {
            add(&attempt.tool);
        }
    }
    if let Some(tool) = current_failure_tool {
        add(tool);
    }
    providers
}

/// Check an anyhow error message for rate-limit signals and attempt failover.
///
/// This handles the case where the execution returned `Err(e)` (e.g. ACP
/// `PromptFailed` with `usage_limit_exceeded`) instead of a non-zero
/// `ExecutionResult`. The error text is tested against the same rate-limit
/// patterns used for normal exit-code-based detection.
/// Appends a `FallbackAttempt` to `fallback_chain` when a retry is triggered.
#[allow(clippy::too_many_arguments)]
pub(crate) fn evaluate_error_rate_limit_failover(
    tool_name_str: &str,
    error_message: &str,
    attempts: usize,
    max_failover_attempts: usize,
    tried_tools: &mut Vec<String>,
    tried_specs: &mut Vec<String>,
    tier_auto_select: bool,
    failover_on_crash_enabled: bool,
    resolved_tier_name: Option<&str>,
    executed_session_id: Option<&str>,
    effective_session_arg: Option<&str>,
    ephemeral: bool,
    prompt_text: &str,
    project_root: &Path,
    config: Option<&ProjectConfig>,
    task_needs_edit: Option<bool>,
    current_model_spec: Option<&str>,
    fallback_chain: &mut FallbackChain,
    attempt_elapsed: Option<Duration>,
) -> Result<RateLimitAction> {
    let failover_signal = match detect_transport_error_failover_signal(
        tool_name_str,
        error_message,
        current_model_spec,
    ) {
        Some(signal) => signal,
        None => return Ok(RateLimitAction::NoRateLimit),
    };

    if !allows_init_failure_failover(
        tool_name_str,
        &failover_signal.reason,
        failover_signal.requires_init_failure_window,
        attempt_elapsed,
    ) {
        return Ok(RateLimitAction::NoRateLimit);
    }

    match failover_signal.kind {
        TransportErrorFailoverKind::RateLimit => {
            if !tier_auto_select {
                return Ok(RateLimitAction::NoRateLimit);
            }
            info!(
                tool = %tool_name_str,
                pattern = %failover_signal.matched_pattern,
                quota_exhausted = failover_signal.quota_exhausted,
                attempt = attempts,
                max = max_failover_attempts,
                "Rate limit detected in transport error, attempting failover"
            );
        }
        TransportErrorFailoverKind::AcpCrashRetryExhausted => {
            if !failover_on_crash_enabled {
                return Ok(RateLimitAction::NoRateLimit);
            }
            warn!(
                tool = %tool_name_str,
                pattern = %failover_signal.matched_pattern,
                attempt = attempts,
                max = max_failover_attempts,
                "[csa-failover] ACP crash retry exhaustion detected in transport error; attempting tier failover"
            );
        }
        TransportErrorFailoverKind::GeminiRetryChainExhausted => {
            if !failover_on_crash_enabled {
                return Ok(RateLimitAction::NoRateLimit);
            }
            warn!(
                tool = %tool_name_str,
                pattern = %failover_signal.matched_pattern,
                attempt = attempts,
                max = max_failover_attempts,
                "[csa-failover] Gemini retry chain exhaustion detected in transport error; attempting tier failover"
            );
        }
        TransportErrorFailoverKind::GeminiLegacyInitialStall => {
            if !failover_on_crash_enabled {
                return Ok(RateLimitAction::NoRateLimit);
            }
            warn!(
                tool = %tool_name_str,
                pattern = %failover_signal.matched_pattern,
                attempt = attempts,
                max = max_failover_attempts,
                "[csa-failover] Gemini legacy initial stall detected in transport error; attempting tier failover"
            );
        }
    }

    if attempts >= max_failover_attempts {
        warn!(
            "Max failover attempts ({}) reached for error-path rate limit",
            max_failover_attempts
        );
        return Ok(RateLimitAction::ExhaustedFailovers {
            reason: format!("max failover attempts ({max_failover_attempts}) reached"),
        });
    }

    tried_tools.push(tool_name_str.to_string());
    if let Some(spec) = current_model_spec {
        tried_specs.push(spec.to_string());
    }

    let failover_session_ref = executed_session_id.or(effective_session_arg);
    let session_state = if !ephemeral {
        failover_session_ref.and_then(|sid| {
            let sessions_dir = csa_session::get_session_root(project_root)
                .ok()?
                .join("sessions");
            let resolved_id = csa_session::resolve_session_prefix(&sessions_dir, sid).ok()?;
            csa_session::load_session(project_root, &resolved_id).ok()
        })
    } else {
        None
    };

    let task_needs_edit = task_needs_edit
        .or_else(|| crate::run_helpers::infer_task_edit_requirement(prompt_text))
        .or_else(|| config.map(|cfg| cfg.can_tool_edit_existing(tool_name_str)));

    let Some(cfg) = config else {
        return Ok(RateLimitAction::ExhaustedFailovers {
            reason: "project config unavailable; cannot resolve tier fallback candidates"
                .to_string(),
        });
    };

    let provider_wide_quota_exhaustion = is_provider_wide_quota_exhaustion(
        tool_name_str,
        failover_signal.quota_exhausted,
        error_message,
    );

    // Same provider-pool semantics as the ExecutionResult path above.
    let exhausted_providers = collect_exhausted_providers(
        fallback_chain,
        Some(tool_name_str).filter(|_| provider_wide_quota_exhaustion),
    );

    let action = decide_available_failover(
        tool_name_str,
        "default",
        resolved_tier_name,
        task_needs_edit,
        session_state.as_ref(),
        tried_tools,
        tried_specs,
        &exhausted_providers,
        cfg,
        &failover_signal.matched_pattern,
    )?;

    match action {
        RateLimitAction::Retry {
            new_tool,
            new_model_spec,
        } => {
            warn!(
                from_tool = %tool_name_str,
                from_spec = %current_model_spec.unwrap_or("none"),
                to_tool = %new_tool.as_str(),
                to_spec = %new_model_spec.as_deref().unwrap_or("none"),
                quota_exhausted = failover_signal.quota_exhausted,
                reason = %failover_signal.reason,
                "[csa-failover] intra-tier failover (transport error)"
            );
            fallback_chain.push(FallbackAttempt {
                tool: tool_name_str.to_string(),
                model_spec: current_model_spec.map(String::from),
                skip_reason: failover_signal.matched_pattern.clone(),
                quota_exhausted: provider_wide_quota_exhaustion,
                timestamp: chrono::Utc::now(),
            });
            Ok(RateLimitAction::Retry {
                new_tool,
                new_model_spec,
            })
        }
        RateLimitAction::ExhaustedFailovers { reason } => {
            warn!(
                reason = %reason,
                quota_exhausted = failover_signal.quota_exhausted,
                "Error-path failover not possible"
            );
            // See parity comment in `evaluate_rate_limit_failover` (#1629).
            if provider_wide_quota_exhaustion {
                fallback_chain.push(FallbackAttempt {
                    tool: tool_name_str.to_string(),
                    model_spec: current_model_spec.map(String::from),
                    skip_reason: failover_signal.matched_pattern.clone(),
                    quota_exhausted: true,
                    timestamp: chrono::Utc::now(),
                });
            }
            Ok(RateLimitAction::ExhaustedFailovers { reason })
        }
        RateLimitAction::NoRateLimit => Ok(RateLimitAction::NoRateLimit),
    }
}

#[cfg(test)]
#[path = "run_cmd_post_failover_tests.rs"]
mod permanent_exhaustion_tests;
