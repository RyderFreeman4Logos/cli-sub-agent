use std::collections::HashMap;

use chrono::Utc;

use crate::config::{
    CURRENT_SCHEMA_VERSION, EnforcementMode, ProjectConfig, ProjectMeta, ResourcesConfig,
    ToolConfig, ToolResourceProfile,
};
use crate::config_runtime::default_sandbox_for_tool;

fn empty_config() -> ProjectConfig {
    ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
        resources: ResourcesConfig::default(),
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        preferences: None,
    }
}

// ── Profile auto-detection ─────────────────────────────────────────────

#[test]
fn profile_codex_is_lightweight() {
    let cfg = empty_config();
    assert_eq!(
        cfg.tool_resource_profile("codex"),
        ToolResourceProfile::Lightweight
    );
}

#[test]
fn profile_opencode_is_lightweight() {
    let cfg = empty_config();
    assert_eq!(
        cfg.tool_resource_profile("opencode"),
        ToolResourceProfile::Lightweight
    );
}

#[test]
fn profile_claude_code_is_heavyweight() {
    let cfg = empty_config();
    assert_eq!(
        cfg.tool_resource_profile("claude-code"),
        ToolResourceProfile::Heavyweight
    );
}

#[test]
fn profile_gemini_cli_is_heavyweight() {
    let cfg = empty_config();
    assert_eq!(
        cfg.tool_resource_profile("gemini-cli"),
        ToolResourceProfile::Heavyweight
    );
}

#[test]
fn profile_unknown_tool_defaults_to_heavyweight() {
    let cfg = empty_config();
    assert_eq!(
        cfg.tool_resource_profile("unknown-tool"),
        ToolResourceProfile::Heavyweight
    );
}

#[test]
fn profile_becomes_custom_when_tool_has_memory_override() {
    let mut cfg = empty_config();
    cfg.tools.insert(
        "codex".to_string(),
        ToolConfig {
            memory_max_mb: Some(512),
            ..Default::default()
        },
    );
    assert_eq!(
        cfg.tool_resource_profile("codex"),
        ToolResourceProfile::Custom
    );
}

#[test]
fn profile_becomes_custom_when_tool_has_enforcement_override() {
    let mut cfg = empty_config();
    cfg.tools.insert(
        "codex".to_string(),
        ToolConfig {
            enforcement_mode: Some(EnforcementMode::BestEffort),
            ..Default::default()
        },
    );
    assert_eq!(
        cfg.tool_resource_profile("codex"),
        ToolResourceProfile::Custom
    );
}

// ── Enforcement mode resolution ────────────────────────────────────────

#[test]
fn enforcement_lightweight_defaults_to_off() {
    let cfg = empty_config();
    assert_eq!(
        cfg.tool_enforcement_mode("codex"),
        EnforcementMode::Off,
        "Lightweight tools should default to Off"
    );
}

#[test]
fn enforcement_heavyweight_defaults_to_best_effort() {
    let cfg = empty_config();
    assert_eq!(
        cfg.tool_enforcement_mode("claude-code"),
        EnforcementMode::BestEffort,
        "Heavyweight tools should default to BestEffort"
    );
}

#[test]
fn enforcement_tool_override_wins_over_profile() {
    let mut cfg = empty_config();
    cfg.tools.insert(
        "claude-code".to_string(),
        ToolConfig {
            enforcement_mode: Some(EnforcementMode::Off),
            ..Default::default()
        },
    );
    assert_eq!(
        cfg.tool_enforcement_mode("claude-code"),
        EnforcementMode::Off,
        "Per-tool override should win over profile default"
    );
}

#[test]
fn enforcement_project_level_wins_over_profile_default() {
    let mut cfg = empty_config();
    cfg.resources.enforcement_mode = Some(EnforcementMode::Required);
    assert_eq!(
        cfg.tool_enforcement_mode("codex"),
        EnforcementMode::Required,
        "Project-level enforcement should override profile default"
    );
}

#[test]
fn enforcement_tool_override_wins_over_project_level() {
    let mut cfg = empty_config();
    cfg.resources.enforcement_mode = Some(EnforcementMode::Required);
    cfg.tools.insert(
        "codex".to_string(),
        ToolConfig {
            enforcement_mode: Some(EnforcementMode::Off),
            ..Default::default()
        },
    );
    assert_eq!(
        cfg.tool_enforcement_mode("codex"),
        EnforcementMode::Off,
        "Per-tool override should win over project-level"
    );
}

// ── Memory limits with profile defaults ────────────────────────────────

#[test]
fn memory_max_heavyweight_gets_profile_default() {
    let cfg = empty_config();
    assert_eq!(
        cfg.sandbox_memory_max_mb("claude-code"),
        Some(2048),
        "Heavyweight profile should provide 2048 MB default"
    );
}

#[test]
fn memory_max_lightweight_gets_none() {
    let cfg = empty_config();
    assert_eq!(
        cfg.sandbox_memory_max_mb("codex"),
        None,
        "Lightweight profile should not set memory limits"
    );
}

#[test]
fn memory_max_tool_override_wins_over_profile() {
    let mut cfg = empty_config();
    cfg.tools.insert(
        "claude-code".to_string(),
        ToolConfig {
            memory_max_mb: Some(8192),
            ..Default::default()
        },
    );
    assert_eq!(cfg.sandbox_memory_max_mb("claude-code"), Some(8192));
}

#[test]
fn memory_max_project_level_wins_over_profile() {
    let mut cfg = empty_config();
    cfg.resources.memory_max_mb = Some(1024);
    assert_eq!(
        cfg.sandbox_memory_max_mb("claude-code"),
        Some(1024),
        "Project-level memory_max_mb should override profile default"
    );
}

#[test]
fn memory_swap_heavyweight_gets_profile_default() {
    let cfg = empty_config();
    assert_eq!(
        cfg.sandbox_memory_swap_max_mb("claude-code"),
        Some(0),
        "Heavyweight profile should provide 0 MB swap default (no swap)"
    );
}

#[test]
fn memory_swap_lightweight_gets_none() {
    let cfg = empty_config();
    assert_eq!(
        cfg.sandbox_memory_swap_max_mb("codex"),
        None,
        "Lightweight profile should not set swap limits"
    );
}

// ── P1: Custom profile inherits inherent enforcement ──────────────────

#[test]
fn enforcement_memory_override_inherits_heavyweight_best_effort() {
    let mut cfg = empty_config();
    cfg.tools.insert(
        "claude-code".to_string(),
        ToolConfig {
            memory_max_mb: Some(8192),
            ..Default::default()
        },
    );
    // Profile resolves to Custom, but enforcement should inherit from
    // Heavyweight (BestEffort), not Custom's Off.
    assert_eq!(
        cfg.tool_resource_profile("claude-code"),
        ToolResourceProfile::Custom,
        "Profile should be Custom due to memory override"
    );
    assert_eq!(
        cfg.tool_enforcement_mode("claude-code"),
        EnforcementMode::BestEffort,
        "Custom profile must inherit inherent Heavyweight enforcement, not default to Off"
    );
}

#[test]
fn enforcement_memory_override_inherits_lightweight_off() {
    let mut cfg = empty_config();
    cfg.tools.insert(
        "codex".to_string(),
        ToolConfig {
            memory_max_mb: Some(512),
            ..Default::default()
        },
    );
    assert_eq!(
        cfg.tool_resource_profile("codex"),
        ToolResourceProfile::Custom,
    );
    assert_eq!(
        cfg.tool_enforcement_mode("codex"),
        EnforcementMode::Off,
        "Custom profile on Lightweight tool should inherit Off"
    );
}

#[test]
fn enforcement_explicit_off_on_tool_disables_sandbox() {
    let mut cfg = empty_config();
    cfg.resources.enforcement_mode = Some(EnforcementMode::BestEffort);
    cfg.tools.insert(
        "claude-code".to_string(),
        ToolConfig {
            enforcement_mode: Some(EnforcementMode::Off),
            memory_max_mb: Some(8192),
            ..Default::default()
        },
    );
    assert_eq!(
        cfg.tool_enforcement_mode("claude-code"),
        EnforcementMode::Off,
        "Explicit enforcement_mode = off on tool should override everything"
    );
}

// ── Backward compatibility ─────────────────────────────────────────────

#[test]
fn legacy_enforcement_mode_still_works() {
    let mut cfg = empty_config();
    cfg.resources.enforcement_mode = Some(EnforcementMode::BestEffort);
    assert_eq!(cfg.enforcement_mode(), EnforcementMode::BestEffort);
}

#[test]
fn legacy_enforcement_mode_defaults_to_off() {
    let cfg = empty_config();
    assert_eq!(cfg.enforcement_mode(), EnforcementMode::Off);
}

// ── Lean mode defaults ─────────────────────────────────────────────────

#[test]
fn lean_mode_heavyweight_defaults_to_true() {
    let cfg = empty_config();
    assert!(
        cfg.tool_lean_mode("claude-code"),
        "Heavyweight tools should default lean_mode to true"
    );
}

#[test]
fn lean_mode_lightweight_defaults_to_false() {
    let cfg = empty_config();
    assert!(
        !cfg.tool_lean_mode("codex"),
        "Lightweight tools should default lean_mode to false"
    );
}

#[test]
fn lean_mode_explicit_false_overrides_default() {
    let mut cfg = empty_config();
    cfg.tools.insert(
        "claude-code".to_string(),
        ToolConfig {
            lean_mode: Some(false),
            ..Default::default()
        },
    );
    assert!(
        !cfg.tool_lean_mode("claude-code"),
        "Explicit lean_mode=false should override Heavyweight default"
    );
}

// ── Node heap limit defaults ───────────────────────────────────────────

#[test]
fn node_heap_limit_heavyweight_defaults_to_2048() {
    let cfg = empty_config();
    assert_eq!(
        cfg.sandbox_node_heap_limit_mb("claude-code"),
        Some(2048),
        "Heavyweight tools should default node_heap_limit_mb to 2048"
    );
}

// ── default_sandbox_for_tool pub API ───────────────────────────────────

#[test]
fn default_sandbox_for_tool_claude_code() {
    let opts = default_sandbox_for_tool("claude-code");
    assert_eq!(opts.enforcement, EnforcementMode::BestEffort);
    assert_eq!(opts.memory_max_mb, Some(2048));
    assert_eq!(opts.memory_swap_max_mb, Some(0));
    assert!(opts.lean_mode);
    assert_eq!(opts.node_heap_limit_mb, Some(2048));
}

#[test]
fn default_sandbox_for_tool_codex() {
    let opts = default_sandbox_for_tool("codex");
    assert_eq!(opts.enforcement, EnforcementMode::Off);
    assert_eq!(opts.memory_max_mb, None);
    assert_eq!(opts.memory_swap_max_mb, None);
    assert!(!opts.lean_mode);
    assert_eq!(opts.node_heap_limit_mb, None);
}
