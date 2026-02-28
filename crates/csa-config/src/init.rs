use anyhow::{Result, bail};
use chrono::Utc;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::Path;

use crate::config::{
    CURRENT_SCHEMA_VERSION, ProjectConfig, ProjectMeta, ResourcesConfig, TierConfig, ToolConfig,
    ToolRestrictions,
};

/// Load tool names explicitly disabled (`enabled = false`) in the global config.
///
/// Returns an empty list if the global config doesn't exist or can't be parsed.
fn load_globally_disabled_tools() -> Vec<String> {
    let global_path = match ProjectConfig::user_config_path() {
        Some(p) if p.exists() => p,
        _ => return Vec::new(),
    };
    let content = match std::fs::read_to_string(&global_path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let val: toml::Value = match content.parse() {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let tools_table = match val.get("tools").and_then(|t| t.as_table()) {
        Some(t) => t,
        None => return Vec::new(),
    };
    tools_table
        .iter()
        .filter(|(_, v)| v.get("enabled").and_then(|e| e.as_bool()) == Some(false))
        .map(|(name, _)| name.clone())
        .collect()
}

/// Detect which tools are installed on the system
pub fn detect_installed_tools() -> Vec<&'static str> {
    let system_path = std::env::var_os("PATH").unwrap_or_default();
    detect_installed_tools_in_paths(&system_path)
}

/// Detect which tools are installed, searching only the given `paths`.
///
/// This avoids relying on the process-global `PATH` environment variable,
/// making it safe to call from parallel tests.
fn detect_installed_tools_in_paths(paths: &OsStr) -> Vec<&'static str> {
    let tools = ["gemini", "opencode", "codex", "claude"];
    let names = ["gemini-cli", "opencode", "codex", "claude-code"];
    tools
        .iter()
        .zip(names.iter())
        .filter(|(exec, _)| which::which_in(exec, Some(paths), ".").is_ok())
        .map(|(_, name)| *name)
        .collect()
}

/// Build smart tier configuration based on installed tools.
///
/// Assigns tools to tiers based on their characteristics:
/// - tier-1-quick: Fast, cheap (gemini-cli flash > codex sonnet > opencode sonnet > claude-code sonnet)
/// - tier-2-standard: Balanced (codex sonnet > claude-code sonnet > opencode sonnet > gemini-cli pro)
/// - tier-3-complex: Deep reasoning (claude-code opus > codex opus > opencode opus > gemini-cli pro)
///
/// Tools in `globally_disabled` are treated as unavailable even if their binary
/// is installed, respecting the user's global `enabled = false` settings.
///
/// If no tools are available, falls back to gemini-cli with all tiers disabled.
fn build_smart_tiers(
    installed: &[&str],
    globally_disabled: &[String],
) -> HashMap<String, TierConfig> {
    let mut tiers = HashMap::new();

    // A tool is usable only if installed AND not globally disabled.
    let has_tool =
        |name: &str| installed.contains(&name) && !globally_disabled.contains(&name.to_string());

    // tier-1-quick: Fast, cheap
    let tier1_model = if has_tool("gemini-cli") {
        "gemini-cli/google/gemini-3-flash-preview/xhigh"
    } else if has_tool("codex") {
        "codex/anthropic/claude-sonnet-4-5-20250929/default"
    } else if has_tool("opencode") {
        "opencode/anthropic/claude-sonnet-4-5-20250929/default"
    } else if has_tool("claude-code") {
        "claude-code/anthropic/claude-sonnet-4-5-20250929/default"
    } else {
        // Fallback: gemini-cli (will be disabled)
        "gemini-cli/google/gemini-3-flash-preview/xhigh"
    };

    tiers.insert(
        "tier-1-quick".to_string(),
        TierConfig {
            description: "Quick tasks, low cost".to_string(),
            models: vec![tier1_model.to_string()],
            token_budget: None,
            max_turns: None,
        },
    );

    // tier-2-standard: Balanced
    let tier2_model = if has_tool("codex") {
        "codex/anthropic/claude-sonnet-4-5-20250929/default"
    } else if has_tool("claude-code") {
        "claude-code/anthropic/claude-sonnet-4-5-20250929/default"
    } else if has_tool("opencode") {
        "opencode/anthropic/claude-sonnet-4-5-20250929/default"
    } else if has_tool("gemini-cli") {
        "gemini-cli/google/gemini-3-pro-preview/xhigh"
    } else {
        // Fallback: gemini-cli (will be disabled)
        "gemini-cli/google/gemini-3-pro-preview/xhigh"
    };

    tiers.insert(
        "tier-2-standard".to_string(),
        TierConfig {
            description: "Standard development tasks".to_string(),
            models: vec![tier2_model.to_string()],
            token_budget: None,
            max_turns: None,
        },
    );

    // tier-3-complex: Deep reasoning
    let tier3_model = if has_tool("claude-code") {
        "claude-code/anthropic/claude-opus-4-6/default"
    } else if has_tool("codex") {
        "codex/anthropic/claude-opus-4-6/default"
    } else if has_tool("opencode") {
        "opencode/anthropic/claude-opus-4-6/default"
    } else if has_tool("gemini-cli") {
        "gemini-cli/google/gemini-3-pro-preview/xhigh"
    } else {
        // Fallback: gemini-cli (will be disabled)
        "gemini-cli/google/gemini-3-pro-preview/xhigh"
    };

    tiers.insert(
        "tier-3-complex".to_string(),
        TierConfig {
            description: "Complex reasoning, architecture, deep analysis, code review".to_string(),
            models: vec![tier3_model.to_string()],
            token_budget: None,
            max_turns: None,
        },
    );

    tiers
}

/// Initialize project configuration.
/// If non_interactive is true, generate default config with detected tools.
/// If minimal is true, generate only [project] + [tools] with no tiers/resources.
/// Returns the generated config.
pub fn init_project(
    project_root: &Path,
    non_interactive: bool,
    minimal: bool,
) -> Result<ProjectConfig> {
    let config_path = ProjectConfig::config_path(project_root);
    if config_path.exists() {
        bail!("Configuration already exists at {}", config_path.display());
    }

    let project_name = project_root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unnamed".to_string());

    let installed = detect_installed_tools();

    // Load global config to discover tools the user has explicitly disabled.
    // Global `enabled = false` is a hard override — init must not re-enable them.
    let globally_disabled: Vec<String> = load_globally_disabled_tools();

    let mut tools = HashMap::new();

    // gemini-cli has special restrictions
    if installed.contains(&"gemini-cli") || !non_interactive {
        let is_usable =
            installed.contains(&"gemini-cli") && !globally_disabled.contains(&"gemini-cli".into());
        tools.insert(
            "gemini-cli".to_string(),
            ToolConfig {
                enabled: is_usable,
                restrictions: Some(ToolRestrictions {
                    allow_edit_existing_files: false,
                }),
                suppress_notify: true,
                ..Default::default()
            },
        );
    }

    // Other tools with default config
    for tool_name in &["opencode", "codex", "claude-code"] {
        let is_usable =
            installed.contains(tool_name) && !globally_disabled.contains(&tool_name.to_string());
        tools.insert(
            tool_name.to_string(),
            ToolConfig {
                enabled: is_usable,
                restrictions: None,
                suppress_notify: true,
                ..Default::default()
            },
        );
    }

    let config = if minimal {
        // Minimal config: only [project] section, rely on global config / defaults for the rest
        ProjectConfig {
            schema_version: CURRENT_SCHEMA_VERSION,
            project: ProjectMeta {
                name: project_name,
                created_at: Utc::now(),
                max_recursion_depth: 5,
            },
            resources: ResourcesConfig::default(),
            acp: Default::default(),
            session: Default::default(),
            memory: Default::default(),
            tools: HashMap::new(),
            review: None,
            debate: None,
            tiers: HashMap::new(),
            tier_mapping: HashMap::new(),
            aliases: HashMap::new(),
            preferences: None,
        }
    } else {
        ProjectConfig {
            schema_version: CURRENT_SCHEMA_VERSION,
            project: ProjectMeta {
                name: project_name,
                created_at: Utc::now(),
                max_recursion_depth: 5,
            },
            resources: ResourcesConfig {
                min_free_memory_mb: 4096,
                idle_timeout_seconds: 300,
                ..Default::default()
            },
            acp: Default::default(),
            session: Default::default(),
            memory: Default::default(),
            tools,
            review: None,
            debate: None,
            tiers: build_smart_tiers(&installed, &globally_disabled),
            tier_mapping: default_tier_mapping(),
            aliases: HashMap::new(),
            preferences: None,
        }
    };

    config.save(project_root)?;

    // Update .gitignore if it exists
    update_gitignore(project_root)?;

    Ok(config)
}

/// Build default tier mapping for common task types.
fn default_tier_mapping() -> HashMap<String, String> {
    let mut tier_mapping = HashMap::new();
    tier_mapping.insert("default".to_string(), "tier-2-standard".to_string());
    tier_mapping.insert("security_audit".to_string(), "tier-3-complex".to_string());
    tier_mapping.insert(
        "architecture_design".to_string(),
        "tier-3-complex".to_string(),
    );
    tier_mapping.insert("code_review".to_string(), "tier-2-standard".to_string());
    tier_mapping.insert(
        "feature_implementation".to_string(),
        "tier-2-standard".to_string(),
    );
    tier_mapping.insert("bug_fix".to_string(), "tier-2-standard".to_string());
    tier_mapping.insert("documentation".to_string(), "tier-1-quick".to_string());
    tier_mapping.insert("quick_question".to_string(), "tier-1-quick".to_string());
    tier_mapping
}

/// Add .csa/ to .gitignore if not already present
pub fn update_gitignore(project_root: &Path) -> Result<()> {
    let gitignore_path = project_root.join(".gitignore");
    if gitignore_path.exists() {
        let content = std::fs::read_to_string(&gitignore_path)?;
        if !content
            .lines()
            .any(|line| line.trim() == ".csa/" || line.trim() == ".csa")
        {
            let mut new_content = content;
            if !new_content.ends_with('\n') {
                new_content.push('\n');
            }
            new_content.push_str(".csa/\n");
            std::fs::write(&gitignore_path, new_content)?;
        }
    } else {
        std::fs::write(&gitignore_path, ".csa/\n")?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "init_tests.rs"]
mod tests;
