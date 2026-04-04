//! Filesystem-dependent rotation tests (split for monolith limit).

use super::*;
use csa_config::{
    ProjectConfig, ProjectMeta, TierConfig, TierStrategy, ToolConfig, ToolRestrictions,
};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use tempfile::tempdir;

static ROTATION_TAIL_ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

struct ScopedXdgOverride {
    original: Option<String>,
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl ScopedXdgOverride {
    fn new(tmp: &tempfile::TempDir) -> Self {
        let lock = ROTATION_TAIL_ENV_LOCK.lock().expect("env lock poisoned");
        let original = std::env::var("XDG_STATE_HOME").ok();
        // SAFETY: test-scoped env mutation protected by ROTATION_TAIL_ENV_LOCK.
        unsafe { std::env::set_var("XDG_STATE_HOME", tmp.path().join("state").to_str().unwrap()) };
        Self {
            original,
            _lock: lock,
        }
    }
}

impl Drop for ScopedXdgOverride {
    fn drop(&mut self) {
        // SAFETY: restoration of test-scoped env mutation (lock still held).
        unsafe {
            match &self.original {
                Some(v) => std::env::set_var("XDG_STATE_HOME", v),
                None => std::env::remove_var("XDG_STATE_HOME"),
            }
        }
    }
}

fn make_config(models: Vec<&str>, disabled_tools: Vec<&str>) -> ProjectConfig {
    make_config_with_strategy(models, disabled_tools, TierStrategy::default())
}

fn make_config_with_strategy(
    models: Vec<&str>,
    disabled_tools: Vec<&str>,
    strategy: TierStrategy,
) -> ProjectConfig {
    let mut tools = HashMap::new();
    for tool in disabled_tools {
        tools.insert(
            tool.to_string(),
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
            description: "test tier".to_string(),
            models: models.iter().map(|s| s.to_string()).collect(),
            strategy,
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

fn make_config_with_restrictions(models: Vec<&str>, restricted_tools: Vec<&str>) -> ProjectConfig {
    let mut tools = HashMap::new();
    for tool in restricted_tools {
        tools.insert(
            tool.to_string(),
            ToolConfig {
                restrictions: Some(ToolRestrictions {
                    allow_edit_existing_files: false,
                    allow_write_new_files: true,
                }),
                ..Default::default()
            },
        );
    }

    let mut tiers = HashMap::new();
    tiers.insert(
        "tier3".to_string(),
        TierConfig {
            description: "test tier".to_string(),
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

#[test]
fn test_resolve_tier_tool_rotated_round_robin() {
    let temp = tempdir().unwrap();
    let _xdg = ScopedXdgOverride::new(&temp);
    let config = make_config_with_strategy(
        vec![
            "gemini-cli/google/gemini-2.5-pro/0",
            "codex/openai/o4-mini/0",
            "claude-code/anthropic/sonnet/0",
        ],
        vec![],
        TierStrategy::RoundRobin,
    );

    // First call → index 1 (codex)
    let result = resolve_tier_tool_rotated(&config, "default", temp.path(), false)
        .unwrap()
        .unwrap();
    assert_eq!(result.0, "codex", "First rotation should pick codex");

    // Second call → index 2 (claude-code)
    let result = resolve_tier_tool_rotated(&config, "default", temp.path(), false)
        .unwrap()
        .unwrap();
    assert_eq!(
        result.0, "claude-code",
        "Second rotation should pick claude-code"
    );

    // Third call → wraps to index 0 (gemini-cli)
    let result = resolve_tier_tool_rotated(&config, "default", temp.path(), false)
        .unwrap()
        .unwrap();
    assert_eq!(
        result.0, "gemini-cli",
        "Third rotation should wrap to gemini-cli"
    );

    // Fourth call → back to index 1 (codex)
    let result = resolve_tier_tool_rotated(&config, "default", temp.path(), false)
        .unwrap()
        .unwrap();
    assert_eq!(
        result.0, "codex",
        "Fourth rotation should cycle back to codex"
    );
}

#[test]
fn test_resolve_tier_tool_priority_always_first() {
    let temp = tempdir().unwrap();
    let _xdg = ScopedXdgOverride::new(&temp);
    let config = make_config_with_strategy(
        vec![
            "gemini-cli/google/gemini-2.5-pro/0",
            "codex/openai/o4-mini/0",
            "claude-code/anthropic/sonnet/0",
        ],
        vec![],
        TierStrategy::Priority,
    );

    // Every call should pick the first model (gemini-cli)
    for i in 0..4 {
        let result = resolve_tier_tool_rotated(&config, "default", temp.path(), false)
            .unwrap()
            .unwrap();
        assert_eq!(
            result.0, "gemini-cli",
            "Priority call {i} should always pick first eligible (gemini-cli)"
        );
    }
}

#[test]
fn test_resolve_tier_tool_priority_skips_disabled_first() {
    let temp = tempdir().unwrap();
    let _xdg = ScopedXdgOverride::new(&temp);
    let config = make_config_with_strategy(
        vec![
            "gemini-cli/google/gemini-2.5-pro/0",
            "codex/openai/o4-mini/0",
            "claude-code/anthropic/sonnet/0",
        ],
        vec!["gemini-cli"],
        TierStrategy::Priority,
    );

    // First eligible is codex (gemini-cli disabled)
    for _ in 0..3 {
        let result = resolve_tier_tool_rotated(&config, "default", temp.path(), false)
            .unwrap()
            .unwrap();
        assert_eq!(
            result.0, "codex",
            "Priority should pick codex when gemini-cli disabled"
        );
    }
}

#[test]
fn test_resolve_tier_tool_rotated_skips_disabled_round_robin() {
    let temp = tempdir().unwrap();
    let _xdg = ScopedXdgOverride::new(&temp);
    let config = make_config_with_strategy(
        vec![
            "gemini-cli/google/gemini-2.5-pro/0",
            "codex/openai/o4-mini/0",
            "claude-code/anthropic/sonnet/0",
        ],
        vec!["codex"], // codex disabled
        TierStrategy::RoundRobin,
    );

    // First call → skips codex (disabled), picks claude-code (index 2)
    let result = resolve_tier_tool_rotated(&config, "default", temp.path(), false)
        .unwrap()
        .unwrap();
    assert_eq!(
        result.0, "claude-code",
        "Should skip disabled codex and pick claude-code"
    );

    // Second call → wraps, skips codex again, picks gemini-cli (index 0)
    let result = resolve_tier_tool_rotated(&config, "default", temp.path(), false)
        .unwrap()
        .unwrap();
    assert_eq!(result.0, "gemini-cli", "Should wrap and pick gemini-cli");
}

#[test]
fn test_resolve_tier_tool_rotated_all_disabled() {
    let temp = tempdir().unwrap();
    let config = make_config(
        vec![
            "gemini-cli/google/gemini-2.5-pro/0",
            "codex/openai/o4-mini/0",
        ],
        vec!["gemini-cli", "codex"],
    );

    let result = resolve_tier_tool_rotated(&config, "default", temp.path(), false).unwrap();
    assert!(
        result.is_none(),
        "Should return None when all tools disabled"
    );
}

#[test]
fn test_resolve_tier_tool_rotated_empty_models() {
    let temp = tempdir().unwrap();
    let config = make_config(vec![], vec![]);

    let result = resolve_tier_tool_rotated(&config, "default", temp.path(), false).unwrap();
    assert!(result.is_none(), "Should return None for empty models list");
}

#[test]
fn test_resolve_tier_tool_rotated_single_tool() {
    let temp = tempdir().unwrap();
    let _xdg = ScopedXdgOverride::new(&temp);
    let config = make_config(vec!["gemini-cli/google/gemini-2.5-pro/0"], vec![]);

    // With only one tool, it should always return that tool
    let result = resolve_tier_tool_rotated(&config, "default", temp.path(), false)
        .unwrap()
        .unwrap();
    assert_eq!(result.0, "gemini-cli");

    let result = resolve_tier_tool_rotated(&config, "default", temp.path(), false)
        .unwrap()
        .unwrap();
    assert_eq!(
        result.0, "gemini-cli",
        "Single tool should always be selected"
    );
}

#[test]
fn test_resolve_tier_tool_rotated_returns_full_spec() {
    let temp = tempdir().unwrap();
    let _xdg = ScopedXdgOverride::new(&temp);
    let config = make_config(vec!["codex/openai/o4-mini/0"], vec![]);

    let result = resolve_tier_tool_rotated(&config, "default", temp.path(), false)
        .unwrap()
        .unwrap();
    assert_eq!(result.0, "codex");
    assert_eq!(result.1, "codex/openai/o4-mini/0");
}

#[test]
fn test_resolve_tier_name_missing_tier_returns_none() {
    // Config with no tiers at all
    let config = ProjectConfig {
        schema_version: 1,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
        resources: Default::default(),
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
    };
    assert_eq!(resolve_tier_name(&config, "anything"), None);
}

#[test]
fn test_rotation_state_default_is_empty() {
    let state = RotationState::default();
    assert!(state.tiers.is_empty());
}

#[test]
fn test_rotated_skips_restricted_tool_when_needs_edit() {
    let temp = tempdir().unwrap();
    let _xdg = ScopedXdgOverride::new(&temp);
    let config = make_config_with_restrictions(
        vec![
            "gemini-cli/google/gemini-2.5-pro/0",
            "codex/openai/o4-mini/0",
        ],
        vec!["gemini-cli"],
    );

    // needs_edit=true → skip gemini-cli (restricted), pick codex
    let result = resolve_tier_tool_rotated(&config, "default", temp.path(), true)
        .unwrap()
        .unwrap();
    assert_eq!(result.0, "codex", "Should skip restricted gemini-cli");

    // needs_edit=false → priority picks first eligible (gemini-cli, since restriction only blocks editing)
    let temp2 = tempdir().unwrap();
    let result = resolve_tier_tool_rotated(&config, "default", temp2.path(), false)
        .unwrap()
        .unwrap();
    assert_eq!(
        result.0, "gemini-cli",
        "Without needs_edit, priority should pick first eligible (gemini-cli)"
    );
}

#[test]
fn test_rotated_returns_none_when_all_restricted_and_needs_edit() {
    let temp = tempdir().unwrap();
    let config = make_config_with_restrictions(
        vec!["gemini-cli/google/gemini-2.5-pro/0"],
        vec!["gemini-cli"],
    );

    let result = resolve_tier_tool_rotated(&config, "default", temp.path(), true).unwrap();
    assert!(
        result.is_none(),
        "Should return None when only tool is restricted and needs_edit"
    );
}
