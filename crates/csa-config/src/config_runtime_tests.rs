use std::collections::HashMap;

use chrono::Utc;

use crate::config::{
    CURRENT_SCHEMA_VERSION, EnforcementMode, ProjectConfig, ProjectMeta, ResourcesConfig,
    ToolConfig, ToolResourceProfile, ToolTransport,
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
        tool_aliases: HashMap::new(),
        preferences: None,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        run: Default::default(),
        execution: Default::default(),
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
    }
}

// ── Profile auto-detection ─────────────────────────────────────────────

#[test]
fn profile_codex_is_heavyweight() {
    let cfg = empty_config();
    assert_eq!(
        cfg.tool_resource_profile("codex"),
        ToolResourceProfile::Heavyweight,
        "codex uses codex-acp (Node.js) backend — must be Heavyweight"
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
        cfg.tool_enforcement_mode("opencode"),
        EnforcementMode::Off,
        "Lightweight tools should default to Off"
    );
}

#[test]
fn enforcement_codex_defaults_to_best_effort() {
    let cfg = empty_config();
    assert_eq!(
        cfg.tool_enforcement_mode("codex"),
        EnforcementMode::BestEffort,
        "codex (Heavyweight) should default to BestEffort"
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
        Some(4096),
        "Heavyweight profile should provide 4096 MB default"
    );
}

#[test]
fn memory_max_gemini_cli_defaults_to_none() {
    let cfg = empty_config();
    assert_eq!(
        cfg.sandbox_memory_max_mb("gemini-cli"),
        None,
        "gemini-cli should not force a hard memory_max_mb default"
    );
}

#[test]
fn memory_max_lightweight_gets_none() {
    let cfg = empty_config();
    assert_eq!(
        cfg.sandbox_memory_max_mb("opencode"),
        None,
        "Lightweight profile should not set memory limits"
    );
}

#[test]
fn memory_max_codex_gets_elevated_default() {
    let cfg = empty_config();
    assert_eq!(
        cfg.sandbox_memory_max_mb("codex"),
        Some(12288),
        "codex should get 12288 MB default (Node.js + Rust compilation headroom)"
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
fn memory_swap_gemini_cli_defaults_to_none() {
    let cfg = empty_config();
    assert_eq!(
        cfg.sandbox_memory_swap_max_mb("gemini-cli"),
        None,
        "gemini-cli should not force a memory_swap_max_mb default"
    );
}

#[test]
fn memory_swap_lightweight_gets_none() {
    let cfg = empty_config();
    assert_eq!(
        cfg.sandbox_memory_swap_max_mb("opencode"),
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
fn enforcement_memory_override_on_lightweight_auto_promotes() {
    let mut cfg = empty_config();
    cfg.tools.insert(
        "opencode".to_string(),
        ToolConfig {
            memory_max_mb: Some(512),
            ..Default::default()
        },
    );
    assert_eq!(
        cfg.tool_resource_profile("opencode"),
        ToolResourceProfile::Custom,
    );
    assert_eq!(
        cfg.tool_enforcement_mode("opencode"),
        EnforcementMode::BestEffort,
        "Lightweight tool with memory_max_mb should auto-promote to BestEffort"
    );
}

#[test]
fn enforcement_memory_override_on_codex_inherits_heavyweight_best_effort() {
    let mut cfg = empty_config();
    cfg.tools.insert(
        "codex".to_string(),
        ToolConfig {
            memory_max_mb: Some(8192),
            ..Default::default()
        },
    );
    assert_eq!(
        cfg.tool_resource_profile("codex"),
        ToolResourceProfile::Custom,
    );
    assert_eq!(
        cfg.tool_enforcement_mode("codex"),
        EnforcementMode::BestEffort,
        "Custom profile on codex (inherently Heavyweight) should inherit BestEffort"
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
    // should fall back to inherent Heavyweight defaults (4096), not None.
    assert_eq!(
        cfg.tool_resource_profile("claude-code"),
        ToolResourceProfile::Custom,
    );
    assert_eq!(
        cfg.sandbox_memory_max_mb("claude-code"),
        Some(4096),
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
        "opencode".to_string(),
        ToolConfig {
            enforcement_mode: Some(EnforcementMode::BestEffort),
            ..Default::default()
        },
    );
    assert_eq!(
        cfg.tool_resource_profile("opencode"),
        ToolResourceProfile::Custom,
    );
    assert_eq!(
        cfg.sandbox_memory_max_mb("opencode"),
        None,
        "Lightweight inherent profile should still return None for memory"
    );
}

// ── Safety net: auto-promote Off when memory limits set ────────────────

#[test]
fn enforcement_auto_promotes_off_when_memory_set_on_lightweight() {
    let mut cfg = empty_config();
    cfg.tools.insert(
        "opencode".to_string(),
        ToolConfig {
            memory_max_mb: Some(4096),
            ..Default::default()
        },
    );
    assert_eq!(
        cfg.tool_enforcement_mode("opencode"),
        EnforcementMode::BestEffort,
        "Off should auto-promote to BestEffort when memory_max_mb is set"
    );
}

#[test]
fn enforcement_no_auto_promote_when_explicitly_off() {
    let mut cfg = empty_config();
    cfg.tools.insert(
        "opencode".to_string(),
        ToolConfig {
            enforcement_mode: Some(EnforcementMode::Off),
            memory_max_mb: Some(4096),
            ..Default::default()
        },
    );
    assert_eq!(
        cfg.tool_enforcement_mode("opencode"),
        EnforcementMode::Off,
        "Explicit Off should NOT be auto-promoted"
    );
}

#[test]
fn enforcement_no_auto_promote_without_memory_limits() {
    let cfg = empty_config();
    assert_eq!(
        cfg.tool_enforcement_mode("opencode"),
        EnforcementMode::Off,
        "No memory limits → no auto-promote"
    );
}

#[test]
fn enforcement_auto_promotes_with_swap_limit_only() {
    let mut cfg = empty_config();
    cfg.tools.insert(
        "opencode".to_string(),
        ToolConfig {
            memory_swap_max_mb: Some(2048),
            ..Default::default()
        },
    );
    assert_eq!(
        cfg.tool_enforcement_mode("opencode"),
        EnforcementMode::BestEffort,
        "Off should auto-promote when memory_swap_max_mb is set"
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
        !cfg.tool_lean_mode("opencode"),
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
        cfg.tool_setting_sources("opencode"),
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
    assert_eq!(cfg.tool_setting_sources("opencode"), None);
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

#[test]
fn node_heap_limit_gemini_cli_defaults_to_none() {
    let cfg = empty_config();
    assert_eq!(
        cfg.sandbox_node_heap_limit_mb("gemini-cli"),
        None,
        "gemini-cli should not inject NODE_OPTIONS heap limit by default"
    );
}

// ── default_sandbox_for_tool pub API ───────────────────────────────────

#[test]
fn default_sandbox_for_tool_claude_code() {
    let opts = default_sandbox_for_tool("claude-code");
    assert_eq!(opts.enforcement, EnforcementMode::BestEffort);
    assert_eq!(opts.memory_max_mb, Some(4096));
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
    assert_eq!(
        opts.enforcement,
        EnforcementMode::BestEffort,
        "codex (Heavyweight) must default to BestEffort enforcement"
    );
    assert_eq!(
        opts.memory_max_mb,
        Some(12288),
        "codex must get elevated memory limit (Node.js + Rust compilation)"
    );
    assert_eq!(
        opts.memory_swap_max_mb,
        Some(0),
        "codex (Heavyweight) must disable swap by default"
    );
    assert_eq!(
        opts.setting_sources,
        Some(vec![]),
        "Heavyweight should default to lean (empty setting_sources)"
    );
    assert_eq!(opts.node_heap_limit_mb, Some(2048));
}

#[test]
fn default_sandbox_for_tool_opencode() {
    let opts = default_sandbox_for_tool("opencode");
    assert_eq!(opts.enforcement, EnforcementMode::Off);
    assert_eq!(opts.memory_max_mb, None);
    assert_eq!(opts.memory_swap_max_mb, None);
    assert_eq!(
        opts.setting_sources, None,
        "Lightweight should default to None (load everything)"
    );
    assert_eq!(opts.node_heap_limit_mb, None);
}

#[test]
fn default_sandbox_for_tool_gemini_cli_uses_unbounded_defaults() {
    let opts = default_sandbox_for_tool("gemini-cli");
    assert_eq!(opts.enforcement, EnforcementMode::BestEffort);
    assert_eq!(opts.memory_max_mb, None);
    assert_eq!(opts.memory_swap_max_mb, None);
    assert_eq!(
        opts.setting_sources,
        Some(vec![]),
        "Gemini remains heavyweight for setting source defaults"
    );
    assert_eq!(opts.node_heap_limit_mb, None);
}

#[test]
fn codex_auto_trust_defaults_to_false() {
    let cfg = empty_config();
    assert!(!cfg.codex_auto_trust());
}

#[test]
fn codex_auto_trust_reads_tools_codex_setting() {
    let mut cfg = empty_config();
    cfg.tools.insert(
        "codex".to_string(),
        ToolConfig {
            codex_auto_trust: true,
            ..Default::default()
        },
    );
    assert!(cfg.codex_auto_trust());
}

#[test]
fn tool_default_model_reads_tool_override() {
    let mut cfg = empty_config();
    cfg.tools.insert(
        "codex".to_string(),
        ToolConfig {
            default_model: Some("gpt-5.4".to_string()),
            ..Default::default()
        },
    );
    assert_eq!(cfg.tool_default_model("codex"), Some("gpt-5.4"));
    assert_eq!(cfg.tool_default_model("claude-code"), None);
}

#[test]
fn tool_default_thinking_reads_tool_override() {
    let mut cfg = empty_config();
    cfg.tools.insert(
        "codex".to_string(),
        ToolConfig {
            default_thinking: Some("xhigh".to_string()),
            ..Default::default()
        },
    );
    assert_eq!(cfg.tool_default_thinking("codex"), Some("xhigh"));
    assert_eq!(cfg.tool_default_thinking("claude-code"), None);
}

#[test]
fn tool_transport_reads_tool_override() {
    let mut cfg = empty_config();
    cfg.tools.insert(
        "codex".to_string(),
        ToolConfig {
            transport: Some(ToolTransport::Cli),
            ..Default::default()
        },
    );

    assert_eq!(cfg.tool_transport("codex"), Some(ToolTransport::Cli));
    assert_eq!(cfg.tool_transport("claude-code"), None);
}

// FS sandbox tests moved to config_runtime_fs_sandbox_tests.rs
