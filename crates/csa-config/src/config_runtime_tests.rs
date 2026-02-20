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
        acp: Default::default(),
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

// ── P1: Custom profile inherits inherent memory defaults ───────────────

#[test]
fn memory_max_enforcement_only_inherits_heavyweight_defaults() {
    let mut cfg = empty_config();
    cfg.tools.insert(
        "claude-code".to_string(),
        ToolConfig {
            enforcement_mode: Some(EnforcementMode::Required),
            ..Default::default()
        },
    );
    // Profile resolves to Custom (enforcement_mode set), but memory
    // should fall back to inherent Heavyweight defaults (2048), not None.
    assert_eq!(
        cfg.tool_resource_profile("claude-code"),
        ToolResourceProfile::Custom,
    );
    assert_eq!(
        cfg.sandbox_memory_max_mb("claude-code"),
        Some(2048),
        "Custom profile with enforcement set must inherit Heavyweight memory_max_mb"
    );
    assert_eq!(
        cfg.sandbox_memory_swap_max_mb("claude-code"),
        Some(0),
        "Custom profile with enforcement set must inherit Heavyweight memory_swap_max_mb"
    );
}

#[test]
fn memory_max_enforcement_only_lightweight_stays_none() {
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
        ToolResourceProfile::Custom,
    );
    assert_eq!(
        cfg.sandbox_memory_max_mb("codex"),
        None,
        "Lightweight inherent profile should still return None for memory"
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

// ── Lean mode defaults (deprecated, kept for backward compat) ──────────

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

// ── setting_sources resolution ──────────────────────────────────────────

#[test]
fn setting_sources_heavyweight_defaults_to_empty_vec() {
    let cfg = empty_config();
    assert_eq!(
        cfg.tool_setting_sources("claude-code"),
        Some(vec![]),
        "Heavyweight tools should default to Some(vec![]) (lean mode)"
    );
}

#[test]
fn setting_sources_lightweight_defaults_to_none() {
    let cfg = empty_config();
    assert_eq!(
        cfg.tool_setting_sources("codex"),
        None,
        "Lightweight tools should default to None (load everything)"
    );
}

#[test]
fn setting_sources_explicit_wins_over_lean_mode() {
    let mut cfg = empty_config();
    cfg.tools.insert(
        "claude-code".to_string(),
        ToolConfig {
            lean_mode: Some(true),
            setting_sources: Some(vec!["project".to_string()]),
            ..Default::default()
        },
    );
    assert_eq!(
        cfg.tool_setting_sources("claude-code"),
        Some(vec!["project".to_string()]),
        "setting_sources should take priority over lean_mode"
    );
}

#[test]
fn setting_sources_lean_mode_true_maps_to_empty_vec() {
    let mut cfg = empty_config();
    cfg.tools.insert(
        "codex".to_string(),
        ToolConfig {
            lean_mode: Some(true),
            ..Default::default()
        },
    );
    assert_eq!(
        cfg.tool_setting_sources("codex"),
        Some(vec![]),
        "lean_mode=true should map to Some(vec![])"
    );
}

#[test]
fn setting_sources_lean_mode_false_maps_to_none() {
    let mut cfg = empty_config();
    cfg.tools.insert(
        "claude-code".to_string(),
        ToolConfig {
            lean_mode: Some(false),
            ..Default::default()
        },
    );
    assert_eq!(
        cfg.tool_setting_sources("claude-code"),
        None,
        "lean_mode=false should map to None (load everything)"
    );
}

#[test]
fn setting_sources_neither_set_uses_profile_default() {
    let cfg = empty_config();
    // Heavyweight → Some(vec![])
    assert_eq!(cfg.tool_setting_sources("claude-code"), Some(vec![]));
    // Lightweight → None
    assert_eq!(cfg.tool_setting_sources("codex"), None);
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
    assert_eq!(
        opts.setting_sources,
        Some(vec![]),
        "Heavyweight should default to lean (empty setting_sources)"
    );
    assert_eq!(opts.node_heap_limit_mb, Some(2048));
}

#[test]
fn default_sandbox_for_tool_codex() {
    let opts = default_sandbox_for_tool("codex");
    assert_eq!(opts.enforcement, EnforcementMode::Off);
    assert_eq!(opts.memory_max_mb, None);
    assert_eq!(opts.memory_swap_max_mb, None);
    assert_eq!(
        opts.setting_sources, None,
        "Lightweight should default to None (load everything)"
    );
    assert_eq!(opts.node_heap_limit_mb, None);
}
