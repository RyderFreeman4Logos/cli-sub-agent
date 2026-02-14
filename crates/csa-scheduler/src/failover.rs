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
/// - `task_type`: used to find the correct tier.
/// - `task_needs_edit`: whether the task requires file editing.
///   - `Some(true)`: task must be routed to tools that can edit existing files.
///   - `Some(false)`: task does not require edits.
///   - `None`: unknown; do not filter alternatives by edit capability.
/// - `session`: the current session (if any).
/// - `tried_tools`: tools already attempted in this failover chain.
/// - `config`: project configuration.
/// - `original_error`: the error message from the rate-limited tool.
pub fn decide_failover(
    failed_tool: &str,
    task_type: &str,
    task_needs_edit: Option<bool>,
    session: Option<&MetaSessionState>,
    tried_tools: &[String],
    config: &ProjectConfig,
    original_error: &str,
) -> FailoverAction {
    // 1. Find the tier for this task_type
    let tier_name = config
        .tier_mapping
        .get(task_type)
        .cloned()
        .unwrap_or_else(|| "tier3".to_string());

    let tier = match config.tiers.get(&tier_name) {
        Some(t) => t,
        None => {
            return FailoverAction::ReportError {
                reason: format!("Tier '{}' not found in config", tier_name),
                original_error: original_error.to_string(),
            };
        }
    };

    // 2. Find eligible alternative tools (not tried, enabled, edit-compatible)
    let alternatives: Vec<(String, String)> = tier
        .models
        .iter()
        .filter_map(|spec| {
            let tool = spec.split('/').next()?;
            // Skip the failed tool and already-tried tools
            if tool == failed_tool || tried_tools.iter().any(|t| t == tool) {
                return None;
            }
            // Skip disabled
            if !config.is_tool_enabled(tool) {
                return None;
            }
            // Only enforce edit-capable tools when task requirement is known.
            if matches!(task_needs_edit, Some(true)) && !config.can_tool_edit_existing(tool) {
                return None;
            }
            Some((tool.to_string(), spec.clone()))
        })
        .collect();

    if alternatives.is_empty() {
        return FailoverAction::ReportError {
            reason: format!("All tools in tier '{}' exhausted", tier_name),
            original_error: original_error.to_string(),
        };
    }

    let (new_tool, new_spec) = alternatives[0].clone();

    // 3. Check if we can reuse the current session
    if let Some(sess) = session {
        // If session has valuable context (not compacted + has meaningful work),
        // try to stay in the same session
        if has_valuable_context(sess) {
            // Check if the new tool's slot is available in this session
            if !sess.tools.contains_key(&new_tool) {
                info!(
                    failed = %failed_tool,
                    new = %new_tool,
                    session = %sess.meta_session_id,
                    "Failover: retry in same session (valuable context)"
                );
                return FailoverAction::RetryInSession {
                    new_tool,
                    new_model_spec: new_spec,
                    session_id: sess.meta_session_id.clone(),
                };
            }

            // Tool slot occupied → report error to preserve context
            return FailoverAction::ReportError {
                reason: format!(
                    "Session has valuable context and tool '{}' slot is occupied",
                    new_tool,
                ),
                original_error: original_error.to_string(),
            };
        }

        // No valuable context → try same session first, fall back to sibling
        if !sess.tools.contains_key(&new_tool) {
            info!(
                failed = %failed_tool,
                new = %new_tool,
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
    info!(
        failed = %failed_tool,
        new = %new_tool,
        "Failover: retry in sibling session"
    );
    FailoverAction::RetrySiblingSession {
        new_tool,
        new_model_spec: new_spec,
    }
}

/// Check if a session has accumulated valuable context worth preserving.
///
/// A session is considered "valuable" if:
/// - It is not compacted (active work in progress)
/// - It has a summary containing keywords suggesting deep analysis
fn has_valuable_context(session: &MetaSessionState) -> bool {
    if session.context_status.is_compacted {
        return false;
    }

    // Check if any tool has done meaningful work
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use csa_config::{ProjectMeta, TierConfig, ToolConfig};
    use std::collections::HashMap;

    fn make_config(models: Vec<&str>, disabled: Vec<&str>) -> ProjectConfig {
        let mut tools = HashMap::new();
        for t in disabled {
            tools.insert(
                t.to_string(),
                ToolConfig {
                    enabled: false,
                    restrictions: None,
                },
            );
        }
        let mut tiers = HashMap::new();
        tiers.insert(
            "tier3".to_string(),
            TierConfig {
                description: "test".to_string(),
                models: models.iter().map(|s| s.to_string()).collect(),
                token_budget: None,
                max_turns: None,
            },
        );
        let mut tier_mapping = HashMap::new();
        tier_mapping.insert("default".to_string(), "tier3".to_string());

        ProjectConfig {
            schema_version: 1,
            project: ProjectMeta {
                name: "test".to_string(),
                created_at: Utc::now(),
                max_recursion_depth: 5,
            },
            resources: Default::default(),
            tools,
            review: None,
            debate: None,
            tiers,
            tier_mapping,
            aliases: HashMap::new(),
        }
    }

    fn make_session(tools: Vec<(&str, &str)>, compacted: bool) -> MetaSessionState {
        let mut tool_map = HashMap::new();
        for (name, summary) in tools {
            tool_map.insert(
                name.to_string(),
                csa_session::ToolState {
                    provider_session_id: None,
                    last_action_summary: summary.to_string(),
                    last_exit_code: 0,
                    updated_at: Utc::now(),
                    token_usage: None,
                },
            );
        }
        MetaSessionState {
            meta_session_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".to_string(),
            description: None,
            project_path: "/tmp".to_string(),
            created_at: Utc::now(),
            last_accessed: Utc::now(),
            genealogy: Default::default(),
            tools: tool_map,
            context_status: csa_session::ContextStatus {
                is_compacted: compacted,
                last_compacted_at: None,
            },
            total_token_usage: None,
            phase: Default::default(),
            task_context: Default::default(),
            turn_count: 0,
            token_budget: None,
        }
    }

    #[test]
    fn test_failover_to_next_tool() {
        let config = make_config(vec!["gemini-cli/g/m/0", "codex/openai/o4-mini/0"], vec![]);
        let action = decide_failover(
            "gemini-cli",
            "default",
            Some(false),
            None,
            &[],
            &config,
            "429 Resource exhausted",
        );
        match action {
            FailoverAction::RetrySiblingSession { new_tool, .. } => {
                assert_eq!(new_tool, "codex");
            }
            other => panic!("Expected RetrySiblingSession, got {:?}", other),
        }
    }

    #[test]
    fn test_failover_all_exhausted() {
        let config = make_config(vec!["gemini-cli/g/m/0", "codex/openai/o4-mini/0"], vec![]);
        let action = decide_failover(
            "gemini-cli",
            "default",
            Some(false),
            None,
            &["codex".to_string()],
            &config,
            "429",
        );
        match action {
            FailoverAction::ReportError { reason, .. } => {
                assert!(reason.contains("exhausted"));
            }
            other => panic!("Expected ReportError, got {:?}", other),
        }
    }

    #[test]
    fn test_failover_retry_in_session() {
        let config = make_config(vec!["gemini-cli/g/m/0", "codex/openai/o4-mini/0"], vec![]);
        let session = make_session(vec![("gemini-cli", "code review in progress")], false);

        let action = decide_failover(
            "gemini-cli",
            "default",
            Some(false),
            Some(&session),
            &[],
            &config,
            "429",
        );
        match action {
            FailoverAction::RetryInSession {
                new_tool,
                session_id,
                ..
            } => {
                assert_eq!(new_tool, "codex");
                assert_eq!(session_id, session.meta_session_id);
            }
            other => panic!("Expected RetryInSession, got {:?}", other),
        }
    }

    #[test]
    fn test_valuable_context_detection() {
        let session_valuable =
            make_session(vec![("gemini-cli", "Code review analysis complete")], false);
        assert!(has_valuable_context(&session_valuable));

        let session_compacted =
            make_session(vec![("gemini-cli", "Code review analysis complete")], true);
        assert!(!has_valuable_context(&session_compacted));

        let session_trivial = make_session(vec![("gemini-cli", "Hello world test")], false);
        assert!(!has_valuable_context(&session_trivial));
    }

    #[test]
    fn test_failover_on_cooldown_error() {
        let config = make_config(
            vec![
                "gemini-cli/g/m/0",
                "codex/openai/o4-mini/0",
                "claude-code/anthropic/sonnet/0",
            ],
            vec![],
        );
        let action = decide_failover(
            "gemini-cli",
            "default",
            Some(false),
            None,
            &[],
            &config,
            "429 Too Many Requests: cooldown for 60 seconds",
        );
        match action {
            FailoverAction::RetrySiblingSession { new_tool, .. } => {
                assert_eq!(new_tool, "codex");
            }
            other => panic!("Expected RetrySiblingSession, got {:?}", other),
        }
    }

    #[test]
    fn test_failover_on_quota_error() {
        let config = make_config(vec!["gemini-cli/g/m/0", "codex/openai/o4-mini/0"], vec![]);
        let action = decide_failover(
            "codex",
            "default",
            Some(false),
            None,
            &[],
            &config,
            "Error: quota exceeded for model o4-mini",
        );
        match action {
            FailoverAction::RetrySiblingSession { new_tool, .. } => {
                assert_eq!(new_tool, "gemini-cli");
            }
            other => panic!("Expected RetrySiblingSession, got {:?}", other),
        }
    }

    #[test]
    fn test_failover_normal_error_no_match() {
        // A non-rate-limit error should still go through failover logic
        // (the caller decides whether to invoke failover; this function just picks next tool)
        let config = make_config(vec!["gemini-cli/g/m/0", "codex/openai/o4-mini/0"], vec![]);
        let action = decide_failover(
            "gemini-cli",
            "default",
            Some(false),
            None,
            &[],
            &config,
            "Internal server error",
        );
        match action {
            FailoverAction::RetrySiblingSession { new_tool, .. } => {
                assert_eq!(new_tool, "codex");
            }
            other => panic!("Expected RetrySiblingSession, got {:?}", other),
        }
    }

    #[test]
    fn test_failover_disabled_tool_skipped() {
        let config = make_config(
            vec![
                "gemini-cli/g/m/0",
                "codex/openai/o4-mini/0",
                "claude-code/anthropic/sonnet/0",
            ],
            vec!["codex"],
        );
        let action = decide_failover(
            "gemini-cli",
            "default",
            Some(false),
            None,
            &[],
            &config,
            "429",
        );
        match action {
            FailoverAction::RetrySiblingSession { new_tool, .. } => {
                assert_eq!(new_tool, "claude-code");
            }
            other => panic!("Expected RetrySiblingSession, got {:?}", other),
        }
    }

    #[test]
    fn test_failover_missing_tier_returns_error() {
        let config = make_config(vec!["gemini-cli/g/m/0"], vec![]);
        // Use a task_type that maps to a non-existent tier
        let mut config_with_bad_mapping = config;
        config_with_bad_mapping
            .tier_mapping
            .insert("special".to_string(), "tier99".to_string());
        let action = decide_failover(
            "gemini-cli",
            "special",
            Some(false),
            None,
            &[],
            &config_with_bad_mapping,
            "429",
        );
        match action {
            FailoverAction::ReportError { reason, .. } => {
                assert!(reason.contains("tier99"), "reason: {}", reason);
                assert!(reason.contains("not found"), "reason: {}", reason);
            }
            other => panic!("Expected ReportError, got {:?}", other),
        }
    }

    #[test]
    fn test_failover_valuable_session_tool_slot_occupied() {
        let config = make_config(vec!["gemini-cli/g/m/0", "codex/openai/o4-mini/0"], vec![]);
        // Session has valuable context (review keyword) + codex slot already occupied
        let session = make_session(
            vec![
                ("gemini-cli", "deep security audit in progress"),
                ("codex", "prior audit run"),
            ],
            false,
        );
        let action = decide_failover(
            "gemini-cli",
            "default",
            Some(false),
            Some(&session),
            &[],
            &config,
            "429",
        );
        match action {
            FailoverAction::ReportError { reason, .. } => {
                assert!(reason.contains("valuable context"), "reason: {}", reason);
                assert!(reason.contains("occupied"), "reason: {}", reason);
            }
            other => panic!("Expected ReportError, got {:?}", other),
        }
    }

    #[test]
    fn test_failover_no_session_no_valuable_context() {
        // Session exists but has no valuable context (trivial summary, not compacted)
        // and tool slot is occupied → falls through to sibling session
        let config = make_config(vec!["gemini-cli/g/m/0", "codex/openai/o4-mini/0"], vec![]);
        let session = make_session(
            vec![
                ("gemini-cli", "simple hello world"),
                ("codex", "simple run"),
            ],
            false,
        );
        let action = decide_failover(
            "gemini-cli",
            "default",
            Some(false),
            Some(&session),
            &[],
            &config,
            "429",
        );
        match action {
            FailoverAction::RetrySiblingSession { new_tool, .. } => {
                assert_eq!(new_tool, "codex");
            }
            other => panic!("Expected RetrySiblingSession, got {:?}", other),
        }
    }

    #[test]
    fn test_failover_needs_edit_none_skips_filter() {
        // When task_needs_edit is None, no edit capability filtering happens
        let config = make_config(vec!["gemini-cli/g/m/0", "codex/openai/o4-mini/0"], vec![]);
        let action = decide_failover(
            "gemini-cli",
            "default",
            None, // unknown edit requirement
            None,
            &[],
            &config,
            "429",
        );
        match action {
            FailoverAction::RetrySiblingSession { new_tool, .. } => {
                assert_eq!(new_tool, "codex");
            }
            other => panic!("Expected RetrySiblingSession, got {:?}", other),
        }
    }
}
