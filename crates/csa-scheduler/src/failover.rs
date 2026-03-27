//! Failover decision logic for 429 / rate-limit situations.

use csa_config::ProjectConfig;
use csa_session::MetaSessionState;
use serde::Serialize;
use tracing::info;

/// What to do when a tool hits a rate limit.
#[derive(Debug, Clone, Serialize)]
pub enum FailoverAction {
    /// Retry in the same session with a different tool (tool slot not occupied).
    RetryInSession {
        new_tool: String,
        new_model_spec: String,
        session_id: String,
    },
    /// Create a sibling session with a different tool.
    RetrySiblingSession {
        new_tool: String,
        new_model_spec: String,
    },
    /// Report the error to the caller (all tools exhausted or context too valuable).
    ReportError {
        reason: String,
        original_error: String,
    },
}

/// Decide what to do after a 429 rate-limit is detected.
///
/// - `failed_tool`: the tool that was rate-limited.
/// - `task_type`: used to find the correct tier (via `tier_mapping`).
/// - `resolved_tier_name`: when the caller already knows the exact tier
///   (e.g. from `--tier`), pass it here to bypass `tier_mapping` lookup.
/// - `task_needs_edit`: whether the task requires file editing.
///   - `Some(true)`: task must be routed to tools that can edit existing files.
///   - `Some(false)`: task does not require edits.
///   - `None`: unknown; do not filter alternatives by edit capability.
/// - `session`: the current session (if any).
/// - `tried_tools`: tools already attempted in this failover chain.
/// - `tried_specs`: model specs already attempted (enables intra-tier failover
///   when the same tool has multiple models in the tier).
/// - `config`: project configuration.
/// - `original_error`: the error message from the rate-limited tool.
#[allow(clippy::too_many_arguments)]
pub fn decide_failover(
    failed_tool: &str,
    task_type: &str,
    resolved_tier_name: Option<&str>,
    task_needs_edit: Option<bool>,
    session: Option<&MetaSessionState>,
    tried_tools: &[String],
    tried_specs: &[String],
    config: &ProjectConfig,
    original_error: &str,
) -> FailoverAction {
    // 1. Find the tier — prefer explicit tier name, fall back to tier_mapping
    let tier_name = resolved_tier_name
        .map(String::from)
        .or_else(|| config.tier_mapping.get(task_type).cloned())
        .unwrap_or_else(|| "tier3".to_string());

    let tier = match config.tiers.get(&tier_name) {
        Some(t) => t,
        None => {
            return FailoverAction::ReportError {
                reason: format!("Tier '{tier_name}' not found in config"),
                original_error: original_error.to_string(),
            };
        }
    };

    // 2. Find eligible alternative models (not tried, enabled, edit-compatible).
    //    When tried_specs is non-empty, filter at spec granularity so that the
    //    same tool with a different model can be selected (intra-tier failover).
    //    When tried_specs is empty, fall back to tool-level filtering for
    //    backward compatibility.
    let use_spec_level = !tried_specs.is_empty();
    let alternatives: Vec<(String, String)> = tier
        .models
        .iter()
        .filter_map(|spec| {
            let tool = spec.split('/').next()?;
            if use_spec_level {
                if tried_specs.iter().any(|s| s == spec) {
                    return None;
                }
            } else if tool == failed_tool || tried_tools.iter().any(|t| t == tool) {
                return None;
            }
            if !config.is_tool_enabled(tool) {
                return None;
            }
            if matches!(task_needs_edit, Some(true)) && !config.can_tool_edit_existing(tool) {
                return None;
            }
            Some((tool.to_string(), spec.clone()))
        })
        .collect();

    if alternatives.is_empty() {
        // Cross-tier fallback: try models from adjacent tiers when current tier
        // is fully exhausted (issue #493). Sort tier names for deterministic order.
        let mut sorted_tiers: Vec<_> = config.tiers.iter().collect();
        sorted_tiers.sort_by_key(|(name, _)| (*name).clone());
        for (other_tier_name, other_tier) in sorted_tiers {
            if other_tier_name == &tier_name {
                continue;
            }
            for spec in &other_tier.models {
                let Some(tool) = spec.split('/').next() else {
                    continue;
                };
                if tried_specs.iter().any(|s| s == spec) || tried_tools.iter().any(|t| t == tool) {
                    continue;
                }
                if !config.is_tool_enabled(tool) {
                    continue;
                }
                if matches!(task_needs_edit, Some(true)) && !config.can_tool_edit_existing(tool) {
                    continue;
                }
                info!(
                    from_tier = %tier_name,
                    to_tier = %other_tier_name,
                    new_tool = %tool,
                    "Cross-tier failover: current tier exhausted, trying adjacent tier"
                );
                return FailoverAction::RetrySiblingSession {
                    new_tool: tool.to_string(),
                    new_model_spec: spec.clone(),
                };
            }
        }
        return FailoverAction::ReportError {
            reason: format!("All tools in tier '{tier_name}' and adjacent tiers exhausted"),
            original_error: original_error.to_string(),
        };
    }

    let (new_tool, new_spec) = alternatives[0].clone();

    // 3. Check if we can reuse the current session
    if let Some(sess) = session {
        if has_valuable_context(sess) {
            if !sess.tools.contains_key(&new_tool) {
                info!(
                    failed = %failed_tool, new = %new_tool,
                    session = %sess.meta_session_id,
                    "Failover: retry in same session (valuable context)"
                );
                return FailoverAction::RetryInSession {
                    new_tool,
                    new_model_spec: new_spec,
                    session_id: sess.meta_session_id.clone(),
                };
            }
            info!(
                failed = %failed_tool, new = %new_tool,
                session = %sess.meta_session_id,
                "Failover: valuable context but slot occupied, using sibling session"
            );
            return FailoverAction::RetrySiblingSession {
                new_tool,
                new_model_spec: new_spec,
            };
        }

        if !sess.tools.contains_key(&new_tool) {
            info!(
                failed = %failed_tool, new = %new_tool,
                session = %sess.meta_session_id,
                "Failover: retry in same session"
            );
            return FailoverAction::RetryInSession {
                new_tool,
                new_model_spec: new_spec,
                session_id: sess.meta_session_id.clone(),
            };
        }
    }

    // 4. Create sibling session
    info!(failed = %failed_tool, new = %new_tool, "Failover: retry in sibling session");
    FailoverAction::RetrySiblingSession {
        new_tool,
        new_model_spec: new_spec,
    }
}

/// Check if a session has accumulated valuable context worth preserving.
pub(crate) fn has_valuable_context(session: &MetaSessionState) -> bool {
    if session.context_status.is_compacted {
        return false;
    }
    let valuable_keywords = [
        "review",
        "analysis",
        "audit",
        "investigation",
        "bug",
        "debug",
    ];
    session.tools.values().any(|tool_state| {
        let summary_lower = tool_state.last_action_summary.to_lowercase();
        valuable_keywords
            .iter()
            .any(|kw| summary_lower.contains(kw))
    })
}
