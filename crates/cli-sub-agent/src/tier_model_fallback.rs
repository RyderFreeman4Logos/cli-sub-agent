use csa_config::ProjectConfig;
use csa_core::types::ToolName;
use csa_scheduler::RateLimitDetected;

#[derive(Debug, Clone)]
pub(crate) struct TierAttemptFailure {
    pub(crate) model_spec: String,
    pub(crate) reason: String,
}

pub(crate) fn ordered_tier_candidates(
    initial_tool: ToolName,
    initial_model_spec: Option<&str>,
    tier_name: Option<&str>,
    config: Option<&ProjectConfig>,
    tier_fallback_enabled: bool,
) -> Vec<(ToolName, Option<String>)> {
    if !tier_fallback_enabled {
        return vec![(initial_tool, initial_model_spec.map(str::to_string))];
    }

    let Some(tier_name) = tier_name else {
        return vec![(initial_tool, initial_model_spec.map(str::to_string))];
    };
    let Some(cfg) = config else {
        return vec![(initial_tool, initial_model_spec.map(str::to_string))];
    };

    let mut ordered = Vec::new();
    if let Some(spec) = initial_model_spec {
        ordered.push((initial_tool, Some(spec.to_string())));
    }

    for resolution in crate::run_helpers::collect_available_tier_models(tier_name, cfg, None, &[]) {
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

pub(crate) fn classify_next_model_failure(
    tool_name: &str,
    stderr: &str,
    stdout: &str,
    exit_code: i32,
    model_spec: Option<&str>,
) -> Option<RateLimitDetected> {
    csa_scheduler::detect_rate_limit(tool_name, stderr, stdout, exit_code, model_spec)
        .filter(|detected| detected.advance_to_next_model)
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
