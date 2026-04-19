use std::collections::HashMap;
use std::path::PathBuf;

use chrono::Utc;

use crate::config::{
    CURRENT_SCHEMA_VERSION, ProjectConfig, ProjectMeta, ResourcesConfig, ToolConfig,
};
use crate::config_tool::ToolFilesystemSandboxConfig;

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
        execution: Default::default(),
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
    }
}

// ── sandbox_writable_paths resolution ──────────────────────────────────

#[test]
fn writable_paths_returns_none_when_no_per_tool_config() {
    let cfg = empty_config();
    assert_eq!(
        cfg.sandbox_writable_paths("claude-code"),
        None,
        "No per-tool config should return None (caller uses project root)"
    );
}

#[test]
fn writable_paths_tool_level_replaces_project_root() {
    let mut cfg = empty_config();
    cfg.tools.insert(
        "gemini-cli".to_string(),
        ToolConfig {
            filesystem_sandbox: Some(ToolFilesystemSandboxConfig {
                writable_paths: Some(vec![PathBuf::from("/tmp")]),
                readable_paths: None,
                enforcement_mode: None,
            }),
            ..Default::default()
        },
    );
    let paths = cfg.sandbox_writable_paths("gemini-cli");
    assert_eq!(
        paths,
        Some(vec![PathBuf::from("/tmp")]),
        "Tool-level writable_paths should REPLACE project root"
    );
}

#[test]
fn writable_paths_tool_level_appends_global_extra_writable() {
    let mut cfg = empty_config();
    cfg.filesystem_sandbox.extra_writable = vec![PathBuf::from("/opt/data")];
    cfg.tools.insert(
        "gemini-cli".to_string(),
        ToolConfig {
            filesystem_sandbox: Some(ToolFilesystemSandboxConfig {
                writable_paths: Some(vec![PathBuf::from("/tmp")]),
                readable_paths: None,
                enforcement_mode: None,
            }),
            ..Default::default()
        },
    );
    let paths = cfg.sandbox_writable_paths("gemini-cli");
    assert_eq!(
        paths,
        Some(vec![PathBuf::from("/tmp"), PathBuf::from("/opt/data")]),
        "Global extra_writable should be appended to tool-level paths"
    );
}

#[test]
fn writable_paths_legacy_tool_writable_overrides_works() {
    let mut cfg = empty_config();
    cfg.filesystem_sandbox.tool_writable_overrides.insert(
        "claude-code".to_string(),
        vec![PathBuf::from("/home/user/.special")],
    );
    let paths = cfg.sandbox_writable_paths("claude-code");
    assert_eq!(
        paths,
        Some(vec![PathBuf::from("/home/user/.special")]),
        "Legacy tool_writable_overrides should work as fallback"
    );
}

#[test]
fn writable_paths_legacy_overrides_append_global_extra() {
    let mut cfg = empty_config();
    cfg.filesystem_sandbox.extra_writable = vec![PathBuf::from("/opt/cache")];
    cfg.filesystem_sandbox
        .tool_writable_overrides
        .insert("codex".to_string(), vec![PathBuf::from("/tmp/codex")]);
    let paths = cfg.sandbox_writable_paths("codex");
    assert_eq!(
        paths,
        Some(vec![
            PathBuf::from("/tmp/codex"),
            PathBuf::from("/opt/cache")
        ]),
        "Legacy overrides should also append global extra_writable"
    );
}

#[test]
fn writable_paths_new_style_takes_priority_over_legacy() {
    let mut cfg = empty_config();
    // Legacy override
    cfg.filesystem_sandbox
        .tool_writable_overrides
        .insert("gemini-cli".to_string(), vec![PathBuf::from("/old/path")]);
    // New-style override (higher priority)
    cfg.tools.insert(
        "gemini-cli".to_string(),
        ToolConfig {
            filesystem_sandbox: Some(ToolFilesystemSandboxConfig {
                writable_paths: Some(vec![PathBuf::from("/new/path")]),
                readable_paths: None,
                enforcement_mode: None,
            }),
            ..Default::default()
        },
    );
    let paths = cfg.sandbox_writable_paths("gemini-cli");
    assert_eq!(
        paths,
        Some(vec![PathBuf::from("/new/path")]),
        "New-style tool filesystem_sandbox should take priority over legacy"
    );
}

#[test]
fn writable_paths_other_tool_unaffected() {
    let mut cfg = empty_config();
    cfg.tools.insert(
        "gemini-cli".to_string(),
        ToolConfig {
            filesystem_sandbox: Some(ToolFilesystemSandboxConfig {
                writable_paths: Some(vec![PathBuf::from("/tmp")]),
                readable_paths: None,
                enforcement_mode: None,
            }),
            ..Default::default()
        },
    );
    assert_eq!(
        cfg.sandbox_writable_paths("claude-code"),
        None,
        "Other tools should not be affected by gemini-cli config"
    );
}

// ── tool_fs_enforcement_mode resolution ────────────────────────────────

#[test]
fn fs_enforcement_returns_none_when_no_config() {
    let cfg = empty_config();
    assert_eq!(
        cfg.tool_fs_enforcement_mode("claude-code"),
        None,
        "No FS sandbox config should return None"
    );
}

#[test]
fn fs_enforcement_tool_level_override_wins() {
    let mut cfg = empty_config();
    cfg.filesystem_sandbox.enforcement_mode = Some("off".to_string());
    cfg.tools.insert(
        "gemini-cli".to_string(),
        ToolConfig {
            filesystem_sandbox: Some(ToolFilesystemSandboxConfig {
                writable_paths: None,
                readable_paths: Some(vec![PathBuf::from("/tmp/readable.json")]),
                enforcement_mode: Some("required".to_string()),
            }),
            ..Default::default()
        },
    );
    assert_eq!(
        cfg.tool_fs_enforcement_mode("gemini-cli"),
        Some("required".to_string()),
        "Tool-level enforcement should win over global"
    );
}

#[test]
fn fs_enforcement_global_fallback() {
    let mut cfg = empty_config();
    cfg.filesystem_sandbox.enforcement_mode = Some("best-effort".to_string());
    assert_eq!(
        cfg.tool_fs_enforcement_mode("claude-code"),
        Some("best-effort".to_string()),
        "Global enforcement should be returned when no tool override"
    );
}

#[test]
fn fs_enforcement_safety_net_promotes_off_with_writable_paths() {
    let mut cfg = empty_config();
    cfg.tools.insert(
        "gemini-cli".to_string(),
        ToolConfig {
            filesystem_sandbox: Some(ToolFilesystemSandboxConfig {
                writable_paths: Some(vec![PathBuf::from("/tmp")]),
                readable_paths: None,
                enforcement_mode: Some("off".to_string()),
            }),
            ..Default::default()
        },
    );
    assert_eq!(
        cfg.tool_fs_enforcement_mode("gemini-cli"),
        Some("best-effort".to_string()),
        "Safety net: off + writable_paths should auto-promote to best-effort"
    );
}

#[test]
fn fs_enforcement_safety_net_promotes_absent_with_writable_paths() {
    let mut cfg = empty_config();
    cfg.tools.insert(
        "gemini-cli".to_string(),
        ToolConfig {
            filesystem_sandbox: Some(ToolFilesystemSandboxConfig {
                writable_paths: Some(vec![PathBuf::from("/tmp")]),
                readable_paths: None,
                enforcement_mode: None,
            }),
            ..Default::default()
        },
    );
    assert_eq!(
        cfg.tool_fs_enforcement_mode("gemini-cli"),
        Some("best-effort".to_string()),
        "Safety net: absent enforcement + writable_paths should auto-promote"
    );
}

#[test]
fn fs_enforcement_safety_net_uses_global_when_writable_paths_set() {
    let mut cfg = empty_config();
    cfg.filesystem_sandbox.enforcement_mode = Some("required".to_string());
    cfg.tools.insert(
        "gemini-cli".to_string(),
        ToolConfig {
            filesystem_sandbox: Some(ToolFilesystemSandboxConfig {
                writable_paths: Some(vec![PathBuf::from("/tmp")]),
                readable_paths: None,
                enforcement_mode: None,
            }),
            ..Default::default()
        },
    );
    assert_eq!(
        cfg.tool_fs_enforcement_mode("gemini-cli"),
        Some("required".to_string()),
        "When writable_paths set but no tool enforcement, should use global"
    );
}

#[test]
fn fs_enforcement_off_without_writable_paths_stays_off() {
    let mut cfg = empty_config();
    cfg.tools.insert(
        "gemini-cli".to_string(),
        ToolConfig {
            filesystem_sandbox: Some(ToolFilesystemSandboxConfig {
                writable_paths: None,
                readable_paths: None,
                enforcement_mode: Some("off".to_string()),
            }),
            ..Default::default()
        },
    );
    assert_eq!(
        cfg.tool_fs_enforcement_mode("gemini-cli"),
        Some("off".to_string()),
        "off without writable_paths is valid — no safety net needed"
    );
}
