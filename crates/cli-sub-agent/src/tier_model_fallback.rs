use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::{FallbackAttempt, ToolName};
use csa_scheduler::RateLimitDetected;
use std::path::Path;
use std::time::Duration;

#[derive(Debug, Clone)]
pub(crate) struct TierAttemptFailure {
    pub(crate) model_spec: String,
    pub(crate) reason: String,
    /// Authoritative permanent-quota flag, when this failure came from a runtime
    /// [`RateLimitDetected`]. The scheduler's `rate_limit` table already decided
    /// permanent (monthly / spending-cap) vs. transient exhaustion BEFORE
    /// normalizing the human-readable `reason` (e.g. it maps "monthly spending
    /// cap" to `reason = "QUOTA_EXHAUSTED"` while keeping `quota_exhausted =
    /// true`). Re-deriving from that lossy normalized `reason` via
    /// [`crate::failover_trace::FailoverSkipKind::classify`] would mislabel it as
    /// transient (#1714), so the structured boolean is carried straight through
    /// to [`csa_core::types::FallbackAttempt::quota_exhausted`]. `None` for
    /// build-time exclusions (disabled / undetected specs that never produced a
    /// `RateLimitDetected`); those keep classifying from `reason`.
    pub(crate) quota_exhausted: Option<bool>,
}

impl TierAttemptFailure {
    /// Construct from a runtime [`RateLimitDetected`], capturing the scheduler's
    /// authoritative `quota_exhausted` flag alongside the normalized reason.
    pub(crate) fn from_rate_limit(model_spec: String, detected: &RateLimitDetected) -> Self {
        Self {
            model_spec,
            reason: detected.reason.clone(),
            quota_exhausted: Some(detected.quota_exhausted),
        }
    }
}

pub(crate) fn ordered_tier_candidates(
    initial_tool: ToolName,
    initial_model_spec: Option<&str>,
    tier_name: Option<&str>,
    config: Option<&ProjectConfig>,
    global_config: Option<&GlobalConfig>,
    tier_fallback_enabled: bool,
    tier_preference_order: &[String],
) -> Vec<(ToolName, Option<String>)> {
    if !tier_fallback_enabled {
        return vec![(initial_tool, initial_model_spec.map(str::to_string))];
    }

    let Some(tier_name) = tier_name else {
        return ordered_global_candidates(initial_tool, initial_model_spec, config, global_config);
    };
    let Some(cfg) = config else {
        return vec![(initial_tool, initial_model_spec.map(str::to_string))];
    };

    let mut ordered = Vec::new();
    if let Some(spec) = initial_model_spec {
        ordered.push((initial_tool, Some(spec.to_string())));
    }

    for resolution in crate::run_helpers::collect_preferred_tier_models(
        tier_name,
        cfg,
        tier_preference_order,
        &[],
    ) {
        if ordered.iter().any(|(_, existing_spec)| {
            existing_spec.as_deref() == Some(resolution.model_spec.as_str())
        }) {
            continue;
        }
        ordered.push((resolution.tool, Some(resolution.model_spec)));
    }

    if ordered.is_empty() {
        ordered.push((initial_tool, initial_model_spec.map(str::to_string)));
    }

    ordered
}

/// Build the persisted per-tool failover chain for a tier result.
///
/// Includes both candidate-build exclusions and runtime attempt failures so a
/// successful later candidate still leaves a trace for earlier build-time skips.
///
/// `selected_model_spec` is the WINNING model (when the run succeeded). It bounds
/// the emitted build-time exclusions to tier specs BEFORE the winner in the
/// actual preference-aware execution order, so a first-choice success does not
/// falsely persist later, never-reached tier models as skips (#1714, #1749).
/// Pass `None` on the all-models-failed path (no winner) to keep the full chain.
pub(crate) fn build_fallback_chain_for_result(
    project_config: Option<&ProjectConfig>,
    tier_name: Option<&str>,
    failures: &[TierAttemptFailure],
    selected_model_spec: Option<&str>,
    tier_preference_order: &[String],
) -> Vec<FallbackAttempt> {
    let ordered_specs: Vec<String> = match (tier_name, project_config) {
        (Some(name), Some(cfg)) => cfg
            .tiers
            .get(name)
            .map(|tier| preference_ordered_specs(&tier.models, tier_preference_order))
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    let exclusions = match (tier_name, project_config) {
        (Some(name), Some(cfg)) => crate::run_helpers::evaluate_tier_models(name, cfg, &[]).1,
        _ => Vec::new(),
    };
    let attempt_failures: Vec<crate::failover_trace::AttemptFailure> = failures
        .iter()
        .map(|failure| crate::failover_trace::AttemptFailure {
            model_spec: failure.model_spec.clone(),
            reason: failure.reason.clone(),
            quota_exhausted: failure.quota_exhausted,
        })
        .collect();
    crate::failover_trace::build_review_fallback_chain(
        &ordered_specs,
        &exclusions,
        &attempt_failures,
        selected_model_spec,
    )
}

fn preference_ordered_specs(
    tier_models: &[String],
    tier_preference_order: &[String],
) -> Vec<String> {
    if tier_preference_order.is_empty() {
        return tier_models.to_vec();
    }

    let mut ordered = Vec::with_capacity(tier_models.len());
    let mut remaining: Vec<&String> = tier_models.iter().collect();

    for preferred_tool in tier_preference_order {
        let mut next_remaining = Vec::new();
        for spec in remaining {
            if model_spec_tool(spec) == preferred_tool {
                ordered.push(spec.clone());
            } else {
                next_remaining.push(spec);
            }
        }
        remaining = next_remaining;
    }

    ordered.extend(remaining.into_iter().cloned());
    ordered
}

fn model_spec_tool(spec: &str) -> &str {
    spec.split('/').next().unwrap_or(spec)
}

fn ordered_global_candidates(
    initial_tool: ToolName,
    initial_model_spec: Option<&str>,
    config: Option<&ProjectConfig>,
    global_config: Option<&GlobalConfig>,
) -> Vec<(ToolName, Option<String>)> {
    let mut ordered = vec![(initial_tool, initial_model_spec.map(str::to_string))];
    let Some(global_config) = global_config else {
        return ordered;
    };

    for tool in csa_config::global::sort_tools_by_effective_priority(
        csa_config::global::all_known_tools(),
        config,
        global_config,
    ) {
        if tool == initial_tool {
            continue;
        }
        if config.is_some_and(|cfg| !cfg.is_tool_auto_selectable(tool.as_str())) {
            continue;
        }
        if !crate::run_helpers::is_tool_runtime_available_for_config(tool.as_str(), config, None) {
            continue;
        }
        ordered.push((tool, None));
    }

    ordered
}

pub(crate) fn classify_next_model_failure_with_elapsed(
    tool_name: &str,
    stderr: &str,
    stdout: &str,
    exit_code: i32,
    model_spec: Option<&str>,
    attempt_elapsed: Option<Duration>,
) -> Option<RateLimitDetected> {
    csa_scheduler::detect_rate_limit(tool_name, stderr, stdout, exit_code, model_spec).filter(
        |detected| {
            detected.advance_to_next_model
                && (!csa_scheduler::requires_init_failure_window(detected)
                    || attempt_elapsed
                        .map(csa_scheduler::within_init_failure_window)
                        .unwrap_or(true))
        },
    )
}

pub(crate) fn chain_failure_reasons(failures: &[TierAttemptFailure]) -> Option<String> {
    (!failures.is_empty()).then(|| {
        failures
            .iter()
            .map(|failure| failure.reason.as_str())
            .collect::<Vec<_>>()
            .join("; ")
    })
}

pub(crate) fn format_all_models_failed_reason(
    tier_name: Option<&str>,
    failures: &[TierAttemptFailure],
) -> Option<String> {
    (!failures.is_empty()).then(|| {
        let tier_label = tier_name.unwrap_or("tier");
        let details = failures
            .iter()
            .map(|failure| format!("{}={}", failure.model_spec, failure.reason))
            .collect::<Vec<_>>()
            .join(", ");
        format!("all {tier_label} models failed: {details}")
    })
}

pub(crate) fn format_all_models_failed_reason_with_reset(
    tier_name: Option<&str>,
    failures: &[TierAttemptFailure],
    reset_after: Option<Duration>,
) -> Option<String> {
    let mut reason = format_all_models_failed_reason(tier_name, failures)?;
    if let Some(reset_after) = reset_after {
        reason.push_str("; earliest_reset=");
        reason.push_str(&format_reset_duration(reset_after));
    }
    Some(reason)
}

pub(crate) fn earliest_backend_reset_window(reset_windows: &[Duration]) -> Option<Duration> {
    reset_windows.iter().copied().min()
}

pub(crate) fn opaque_total_exhaustion_message(
    primary_failure: Option<&str>,
    failure_reason: Option<&str>,
) -> Option<String> {
    let primary_failure = primary_failure?.trim();
    if primary_failure.is_empty() {
        return None;
    }

    let mut reason_count = 0;
    for reason in primary_failure.split(';').map(str::trim) {
        if reason.is_empty() {
            continue;
        }
        reason_count += 1;
        if !is_quota_rate_auth_reason(reason) {
            return None;
        }
    }
    if reason_count == 0 {
        return None;
    }

    let tier_label = parse_all_models_failed_tier_label(failure_reason)?;
    Some(format_opaque_total_exhaustion_message(
        tier_label,
        failure_reason.and_then(parse_backend_reset_duration),
    ))
}

pub(crate) fn parse_backend_reset_duration(text: &str) -> Option<Duration> {
    let lower = text.to_ascii_lowercase();
    let mut earliest_reset: Option<Duration> = None;
    for marker in [
        "earliest_reset=",
        "earliest reset",
        "quota will reset after",
        "quota resets after",
        "reset after",
        "reset in",
    ] {
        for (index, _) in lower.match_indices(marker) {
            let Some(fragment) = lower.get(index + marker.len()..) else {
                continue;
            };
            if let Some(duration) = parse_duration_units(fragment) {
                earliest_reset =
                    Some(earliest_reset.map_or(duration, |current| current.min(duration)));
            }
        }
    }
    earliest_reset
}

fn parse_duration_units(fragment: &str) -> Option<Duration> {
    let group_re = regex::Regex::new(r"((?:[0-9]+\s*[dhms]\s*)+)").ok()?;
    let bounded = fragment.chars().take(80).collect::<String>();
    let duration_text = group_re.captures(&bounded)?.get(1)?.as_str();
    let unit_re = regex::Regex::new(r"([0-9]+)\s*([dhms])").ok()?;
    let mut seconds = 0_u64;
    let mut matched = false;
    for captures in unit_re.captures_iter(duration_text) {
        let value = captures.get(1)?.as_str().parse::<u64>().ok()?;
        let unit_seconds = match captures.get(2)?.as_str() {
            "d" => 86_400,
            "h" => 3_600,
            "m" => 60,
            "s" => 1,
            _ => return None,
        };
        seconds = seconds.checked_add(value.checked_mul(unit_seconds)?)?;
        matched = true;
    }
    matched.then(|| Duration::from_secs(seconds))
}

fn parse_all_models_failed_tier_label(failure_reason: Option<&str>) -> Option<&str> {
    let reason = failure_reason?.trim();
    let rest = reason.strip_prefix("all ")?;
    let (tier_label, _) = rest.split_once(" models failed:")?;
    (!tier_label.trim().is_empty()).then_some(tier_label.trim())
}

fn is_quota_rate_auth_reason(reason: &str) -> bool {
    let lower = reason.to_ascii_lowercase();
    lower.contains("auth_unavailable")
        || lower.contains("gemini_auth_prompt")
        || lower.contains("429")
        || lower.contains("quota")
        || lower.contains("rate-limit")
        || lower.contains("rate limit")
        || lower.contains("resource_exhausted")
        || lower.contains("resource exhausted")
        || lower.contains("usage limit")
        || lower.contains("http 401")
        || lower.contains("http 403")
}

fn format_opaque_total_exhaustion_message(
    tier_label: &str,
    reset_after: Option<Duration>,
) -> String {
    let mut message = format!("review unavailable: all {tier_label} backends rate-limited");
    if let Some(reset_after) = reset_after {
        message.push_str("; earliest reset ~");
        message.push_str(&format_reset_duration(reset_after));
    }
    message
}

fn format_reset_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    let mut minutes = seconds / 60;
    if !seconds.is_multiple_of(60) {
        minutes += 1;
    }
    let hours = minutes / 60;
    let minutes = minutes % 60;
    if hours > 0 {
        return format!("{hours}h {minutes}m");
    }
    format!("{minutes}m")
}

pub(crate) fn fallback_reason_for_result(failures: &[TierAttemptFailure]) -> Option<&'static str> {
    failures
        .iter()
        .any(|failure| {
            let reason = failure.reason.to_ascii_lowercase();
            reason.contains("429") || reason.contains("quota")
        })
        .then_some("429_quota_exhausted")
}

pub(crate) fn persist_fallback_result_fields(
    project_root: &Path,
    session_id: &str,
    original_tool: ToolName,
    fallback_tool: ToolName,
    fallback_reason: Option<&str>,
) {
    let Some(reason) = fallback_reason else {
        return;
    };
    let Ok(Some(mut result)) = csa_session::load_result(project_root, session_id) else {
        return;
    };
    result.original_tool = Some(original_tool.as_str().to_string());
    result.fallback_tool = Some(fallback_tool.as_str().to_string());
    result.fallback_reason = Some(reason.to_string());
    if let Err(err) = csa_session::save_result(project_root, session_id, &result) {
        tracing::warn!(
            session_id,
            error = %err,
            "Failed to persist runtime fallback fields in result.toml"
        );
    }
}

/// Persist the per-tool failover chain into `result.toml` (#1714).
///
/// Complements [`persist_fallback_result_fields`] (which records the collapsed
/// `fallback_reason` for backward compatibility) by writing the full
/// `[[fallback_chain]]` of categorised per-tool skip reasons. Also records
/// `original_tool`/`fallback_tool` so a NON-quota failover (e.g. an
/// all-disabled tier falling back to claude-code) is still attributed, not just
/// the legacy quota path. No-op when the chain is empty (no failover occurred).
pub(crate) fn persist_fallback_chain(
    project_root: &Path,
    session_id: &str,
    original_tool: ToolName,
    fallback_tool: ToolName,
    fallback_chain: Vec<FallbackAttempt>,
) {
    if fallback_chain.is_empty() {
        return;
    }
    let Ok(Some(mut result)) = csa_session::load_result(project_root, session_id) else {
        return;
    };
    result.original_tool = Some(original_tool.as_str().to_string());
    result.fallback_tool = Some(fallback_tool.as_str().to_string());
    result.fallback_chain = Some(fallback_chain);
    if let Err(err) = csa_session::save_result(project_root, session_id, &result) {
        tracing::warn!(
            session_id,
            error = %err,
            "Failed to persist failover chain in result.toml"
        );
    }
}

/// Append a non-fatal warning to `result.toml`'s `warnings` (#1714 Ask 3).
///
/// Used by the writer-family diversity guard: a same-family failover is
/// surfaced as a warning, NOT a verdict change, so a clean single-family review
/// stays merge-able (preserving #1657). Skips writing if the identical warning
/// is already present.
pub(crate) fn persist_result_warning(project_root: &Path, session_id: &str, warning: &str) {
    let Ok(Some(mut result)) = csa_session::load_result(project_root, session_id) else {
        return;
    };
    if result.warnings.iter().any(|existing| existing == warning) {
        return;
    }
    result.warnings.push(warning.to_string());
    if let Err(err) = csa_session::save_result(project_root, session_id, &result) {
        tracing::warn!(
            session_id,
            error = %err,
            "Failed to persist review diversity warning in result.toml"
        );
    }
}

#[cfg(test)]
#[path = "tier_model_fallback_tests.rs"]
mod tests;
