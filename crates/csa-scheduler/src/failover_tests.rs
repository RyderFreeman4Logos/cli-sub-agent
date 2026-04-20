use super::failover::*;
use chrono::Utc;
use csa_config::{ProjectConfig, ProjectMeta, TierConfig, TierStrategy, ToolConfig};
use csa_session::MetaSessionState;
use std::collections::HashMap;

fn make_config(models: Vec<&str>, disabled: Vec<&str>) -> ProjectConfig {
    let mut tools = HashMap::new();
    for t in disabled {
        tools.insert(
            t.to_string(),
            ToolConfig {
                enabled: false,
                restrictions: None,
                suppress_notify: true,
                ..Default::default()
            },
        );
    }
    let mut tiers = HashMap::new();
    tiers.insert(
        "tier3".to_string(),
        TierConfig {
            description: "test".to_string(),
            models: models.iter().map(|s| s.to_string()).collect(),
            strategy: TierStrategy::default(),
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
        acp: Default::default(),
        tools,
        review: None,
        debate: None,
        tiers,
        tier_mapping,
        aliases: HashMap::new(),
        tool_aliases: HashMap::new(),
        preferences: None,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        execution: Default::default(),
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
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
        branch: None,
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
        sandbox_info: None,
        termination_reason: None,
        is_seed_candidate: false,
        git_head_at_creation: None,
        pre_session_porcelain: None,
        last_return_packet: None,
        change_id: None,
        spec_id: None,
        fork_call_timestamps: Vec::new(),
        vcs_identity: None,
        identity_version: 1,
    }
}

fn make_multi_tier_config(
    tier1_models: Vec<&str>,
    tier3_models: Vec<&str>,
    disabled: Vec<&str>,
) -> ProjectConfig {
    let mut tools = HashMap::new();
    for t in disabled {
        tools.insert(
            t.to_string(),
            ToolConfig {
                enabled: false,
                restrictions: None,
                suppress_notify: true,
                ..Default::default()
            },
        );
    }
    let mut tiers = HashMap::new();
    tiers.insert(
        "tier1".to_string(),
        TierConfig {
            description: "frontier".to_string(),
            models: tier1_models.iter().map(|s| s.to_string()).collect(),
            strategy: TierStrategy::default(),
            token_budget: None,
            max_turns: None,
        },
    );
    tiers.insert(
        "tier3".to_string(),
        TierConfig {
            description: "fast".to_string(),
            models: tier3_models.iter().map(|s| s.to_string()).collect(),
            strategy: TierStrategy::default(),
            token_budget: None,
            max_turns: None,
        },
    );
    let mut tier_mapping = HashMap::new();
    tier_mapping.insert("default".to_string(), "tier3".to_string());
    tier_mapping.insert("review".to_string(), "tier1".to_string());

    ProjectConfig {
        schema_version: 1,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
        resources: Default::default(),
        acp: Default::default(),
        tools,
        review: None,
        debate: None,
        tiers,
        tier_mapping,
        aliases: HashMap::new(),
        tool_aliases: HashMap::new(),
        preferences: None,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        execution: Default::default(),
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
    }
}

#[test]
fn test_failover_to_next_tool() {
    let config = make_config(vec!["gemini-cli/g/m/0", "codex/openai/o4-mini/0"], vec![]);
    let action = decide_failover(
        "gemini-cli",
        "default",
        None,
        Some(false),
        None,
        &[],
        &[],
        &config,
        "429 Resource exhausted",
    );
    match action {
        FailoverAction::RetrySiblingSession { new_tool, .. } => assert_eq!(new_tool, "codex"),
        other => panic!("Expected RetrySiblingSession, got {other:?}"),
    }
}

#[test]
fn test_failover_all_exhausted() {
    let config = make_config(vec!["gemini-cli/g/m/0", "codex/openai/o4-mini/0"], vec![]);
    let action = decide_failover(
        "gemini-cli",
        "default",
        None,
        Some(false),
        None,
        &["codex".to_string()],
        &[],
        &config,
        "429",
    );
    match action {
        FailoverAction::ReportError { reason, .. } => assert!(reason.contains("exhausted")),
        other => panic!("Expected ReportError, got {other:?}"),
    }
}

#[test]
fn test_failover_retry_in_session() {
    let config = make_config(vec!["gemini-cli/g/m/0", "codex/openai/o4-mini/0"], vec![]);
    let session = make_session(vec![("gemini-cli", "code review in progress")], false);
    let action = decide_failover(
        "gemini-cli",
        "default",
        None,
        Some(false),
        Some(&session),
        &[],
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
        other => panic!("Expected RetryInSession, got {other:?}"),
    }
}

#[test]
fn test_valuable_context_detection() {
    let valuable = make_session(vec![("gemini-cli", "Code review analysis complete")], false);
    assert!(has_valuable_context(&valuable));
    let compacted = make_session(vec![("gemini-cli", "Code review analysis complete")], true);
    assert!(!has_valuable_context(&compacted));
    let trivial = make_session(vec![("gemini-cli", "Hello world test")], false);
    assert!(!has_valuable_context(&trivial));
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
        None,
        Some(false),
        None,
        &[],
        &[],
        &config,
        "429 Too Many Requests: cooldown for 60 seconds",
    );
    match action {
        FailoverAction::RetrySiblingSession { new_tool, .. } => assert_eq!(new_tool, "codex"),
        other => panic!("Expected RetrySiblingSession, got {other:?}"),
    }
}

#[test]
fn test_failover_on_quota_error() {
    let config = make_config(vec!["gemini-cli/g/m/0", "codex/openai/o4-mini/0"], vec![]);
    let action = decide_failover(
        "codex",
        "default",
        None,
        Some(false),
        None,
        &[],
        &[],
        &config,
        "Error: quota exceeded for model o4-mini",
    );
    match action {
        FailoverAction::RetrySiblingSession { new_tool, .. } => assert_eq!(new_tool, "gemini-cli"),
        other => panic!("Expected RetrySiblingSession, got {other:?}"),
    }
}

#[test]
fn test_failover_normal_error_no_match() {
    let config = make_config(vec!["gemini-cli/g/m/0", "codex/openai/o4-mini/0"], vec![]);
    let action = decide_failover(
        "gemini-cli",
        "default",
        None,
        Some(false),
        None,
        &[],
        &[],
        &config,
        "Internal server error",
    );
    match action {
        FailoverAction::RetrySiblingSession { new_tool, .. } => assert_eq!(new_tool, "codex"),
        other => panic!("Expected RetrySiblingSession, got {other:?}"),
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
        None,
        Some(false),
        None,
        &[],
        &[],
        &config,
        "429",
    );
    match action {
        FailoverAction::RetrySiblingSession { new_tool, .. } => assert_eq!(new_tool, "claude-code"),
        other => panic!("Expected RetrySiblingSession, got {other:?}"),
    }
}

#[test]
fn test_failover_missing_tier_returns_error() {
    let mut config = make_config(vec!["gemini-cli/g/m/0"], vec![]);
    config
        .tier_mapping
        .insert("special".to_string(), "tier99".to_string());
    let action = decide_failover(
        "gemini-cli",
        "special",
        None,
        Some(false),
        None,
        &[],
        &[],
        &config,
        "429",
    );
    match action {
        FailoverAction::ReportError { reason, .. } => {
            assert!(reason.contains("tier99"), "reason: {reason}");
            assert!(reason.contains("not found"), "reason: {reason}");
        }
        other => panic!("Expected ReportError, got {other:?}"),
    }
}

#[test]
fn test_failover_valuable_session_tool_slot_occupied_uses_sibling() {
    let config = make_config(vec!["gemini-cli/g/m/0", "codex/openai/o4-mini/0"], vec![]);
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
        None,
        Some(false),
        Some(&session),
        &[],
        &[],
        &config,
        "429",
    );
    match action {
        FailoverAction::RetrySiblingSession { new_tool, .. } => assert_eq!(new_tool, "codex"),
        other => panic!("Expected RetrySiblingSession, got {other:?}"),
    }
}

#[test]
fn test_failover_no_session_no_valuable_context() {
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
        None,
        Some(false),
        Some(&session),
        &[],
        &[],
        &config,
        "429",
    );
    match action {
        FailoverAction::RetrySiblingSession { new_tool, .. } => assert_eq!(new_tool, "codex"),
        other => panic!("Expected RetrySiblingSession, got {other:?}"),
    }
}

#[test]
fn test_failover_needs_edit_none_skips_filter() {
    let config = make_config(vec!["gemini-cli/g/m/0", "codex/openai/o4-mini/0"], vec![]);
    let action = decide_failover(
        "gemini-cli",
        "default",
        None,
        None,
        None,
        &[],
        &[],
        &config,
        "429",
    );
    match action {
        FailoverAction::RetrySiblingSession { new_tool, .. } => assert_eq!(new_tool, "codex"),
        other => panic!("Expected RetrySiblingSession, got {other:?}"),
    }
}

// --- Quota failover transparency tests ---

#[test]
fn test_gemini_pro_failover_to_claude_in_same_tier() {
    let config = make_multi_tier_config(
        vec![
            "gemini-cli/google/gemini-2.5-pro/high",
            "claude-code/anthropic/claude-sonnet-4-20250514/high",
        ],
        vec!["gemini-cli/google/gemini-2.5-flash/0"],
        vec![],
    );
    let action = decide_failover(
        "gemini-cli",
        "review",
        None,
        Some(false),
        None,
        &[],
        &[],
        &config,
        "Resource exhausted",
    );
    match action {
        FailoverAction::RetrySiblingSession {
            new_tool,
            new_model_spec,
        } => {
            assert_eq!(new_tool, "claude-code");
            assert!(new_model_spec.contains("claude"), "spec: {new_model_spec}");
        }
        other => panic!("Expected RetrySiblingSession to claude, got {other:?}"),
    }
}

#[test]
fn test_gemini_flash_failover_to_claude_in_same_tier() {
    let config = make_multi_tier_config(
        vec!["gemini-cli/google/gemini-2.5-pro/high"],
        vec![
            "gemini-cli/google/gemini-2.5-flash/0",
            "claude-code/anthropic/claude-haiku-4-5-20251001/0",
        ],
        vec![],
    );
    let action = decide_failover(
        "gemini-cli",
        "default",
        None,
        Some(false),
        None,
        &[],
        &[],
        &config,
        "quota exceeded",
    );
    match action {
        FailoverAction::RetrySiblingSession {
            new_tool,
            new_model_spec,
        } => {
            assert_eq!(new_tool, "claude-code");
            assert!(new_model_spec.contains("haiku"), "spec: {new_model_spec}");
        }
        other => panic!("Expected RetrySiblingSession to claude-haiku, got {other:?}"),
    }
}

// --- Cross-tier failover tests ---

#[test]
fn test_cross_tier_fallback_when_current_tier_exhausted() {
    let config = make_multi_tier_config(
        vec!["claude-code/anthropic/claude-sonnet-4-20250514/high"],
        vec!["gemini-cli/google/gemini-2.5-flash/0"],
        vec![],
    );
    let action = decide_failover(
        "gemini-cli",
        "default",
        None,
        Some(false),
        None,
        &[],
        &["gemini-cli/google/gemini-2.5-flash/0".to_string()],
        &config,
        "429 MODEL_CAPACITY_EXHAUSTED",
    );
    match action {
        FailoverAction::RetrySiblingSession {
            new_tool,
            new_model_spec,
        } => {
            assert_eq!(new_tool, "claude-code");
            assert!(new_model_spec.contains("claude"), "spec: {new_model_spec}");
        }
        other => panic!("Expected cross-tier failover to claude-code, got {other:?}"),
    }
}

#[test]
fn test_cross_tier_all_tiers_exhausted_reports_error() {
    let config = make_multi_tier_config(
        vec!["claude-code/anthropic/claude-sonnet-4-20250514/high"],
        vec!["gemini-cli/google/gemini-2.5-flash/0"],
        vec![],
    );
    let action = decide_failover(
        "gemini-cli",
        "default",
        None,
        Some(false),
        None,
        &[],
        &[
            "gemini-cli/google/gemini-2.5-flash/0".to_string(),
            "claude-code/anthropic/claude-sonnet-4-20250514/high".to_string(),
        ],
        &config,
        "429",
    );
    match action {
        FailoverAction::ReportError { reason, .. } => {
            assert!(reason.contains("adjacent tiers"), "reason: {reason}")
        }
        other => panic!("Expected all-exhausted error, got {other:?}"),
    }
}

#[test]
fn test_failover_never_reports_error_when_alternatives_exist() {
    let config = make_multi_tier_config(
        vec![
            "gemini-cli/google/gemini-2.5-pro/high",
            "claude-code/anthropic/claude-sonnet-4-20250514/high",
        ],
        vec![],
        vec![],
    );
    let session = make_session(
        vec![
            ("gemini-cli", "deep code review analysis"),
            ("claude-code", "prior run"),
        ],
        false,
    );
    let action = decide_failover(
        "gemini-cli",
        "review",
        None,
        Some(false),
        Some(&session),
        &[],
        &[],
        &config,
        "RESOURCE_EXHAUSTED",
    );
    match action {
        FailoverAction::RetrySiblingSession { new_tool, .. } => assert_eq!(new_tool, "claude-code"),
        FailoverAction::RetryInSession { .. } => {} // acceptable
        FailoverAction::ReportError { reason, .. } => {
            panic!("Should not report error when alternatives exist: {reason}");
        }
    }
}
