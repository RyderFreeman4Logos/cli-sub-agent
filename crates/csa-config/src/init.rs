use anyhow::{bail, Result};
use chrono::Utc;
use std::collections::HashMap;
use std::path::Path;

use crate::config::{ProjectConfig, ProjectMeta, ResourcesConfig, ToolConfig, ToolRestrictions};

/// Detect which tools are installed on the system
pub fn detect_installed_tools() -> Vec<&'static str> {
    let tools = ["gemini", "opencode", "codex", "claude"];
    let names = ["gemini-cli", "opencode", "codex", "claude-code"];
    tools
        .iter()
        .zip(names.iter())
        .filter(|(exec, _)| which::which(exec).is_ok())
        .map(|(_, name)| *name)
        .collect()
}

/// Initialize project configuration.
/// If non_interactive is true, generate default config with detected tools.
/// Returns the generated config.
pub fn init_project(project_root: &Path, non_interactive: bool) -> Result<ProjectConfig> {
    let config_path = ProjectConfig::config_path(project_root);
    if config_path.exists() {
        bail!("Configuration already exists at {}", config_path.display());
    }

    let project_name = project_root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unnamed".to_string());

    let installed = detect_installed_tools();

    let mut tools = HashMap::new();

    // gemini-cli has special restrictions
    if installed.contains(&"gemini-cli") || !non_interactive {
        let mut gemini_config = ToolConfig {
            enabled: installed.contains(&"gemini-cli"),
            restrictions: Some(ToolRestrictions {
                allow_edit_existing_files: false,
                allowed_operations: vec![
                    "web_search".to_string(),
                    "read".to_string(),
                    "analyze".to_string(),
                    "create_new_file".to_string(),
                ],
            }),
        };
        if !installed.contains(&"gemini-cli") {
            gemini_config.enabled = false;
        }
        tools.insert("gemini-cli".to_string(), gemini_config);
    }

    // Other tools with default config
    for tool_name in &["opencode", "codex", "claude-code"] {
        tools.insert(
            tool_name.to_string(),
            ToolConfig {
                enabled: installed.contains(tool_name),
                restrictions: None,
            },
        );
    }

    let mut initial_estimates = HashMap::new();
    initial_estimates.insert("gemini-cli".to_string(), 150);
    initial_estimates.insert("opencode".to_string(), 500);
    initial_estimates.insert("codex".to_string(), 800);
    initial_estimates.insert("claude-code".to_string(), 1200);

    // Default tier mapping
    let mut tier_mapping = HashMap::new();
    tier_mapping.insert("security_audit".to_string(), "tier1".to_string());
    tier_mapping.insert("architecture_design".to_string(), "tier1".to_string());
    tier_mapping.insert("code_review".to_string(), "tier2".to_string());
    tier_mapping.insert("feature_implementation".to_string(), "tier2".to_string());
    tier_mapping.insert("bug_fix".to_string(), "tier3".to_string());
    tier_mapping.insert("documentation".to_string(), "tier4".to_string());
    tier_mapping.insert("quick_question".to_string(), "tier5".to_string());

    let config = ProjectConfig {
        project: ProjectMeta {
            name: project_name,
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
        resources: ResourcesConfig {
            min_free_memory_mb: 2048,
            min_free_swap_mb: 1024,
            initial_estimates,
        },
        tools,
        tiers: HashMap::new(), // Empty tiers for user to fill in
        tier_mapping,
        aliases: HashMap::new(),
    };

    config.save(project_root)?;

    // Update .gitignore if it exists
    update_gitignore(project_root)?;

    Ok(config)
}

/// Add .csa/ to .gitignore if not already present
fn update_gitignore(project_root: &Path) -> Result<()> {
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
    }
    // If no .gitignore exists, don't create one
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_init_project_creates_config() {
        let dir = tempdir().unwrap();
        let config = init_project(dir.path(), true).unwrap();

        assert!(!config.project.name.is_empty());
        assert_eq!(config.project.max_recursion_depth, 5);
        assert!(config.resources.min_free_memory_mb > 0);

        // Config file should exist
        let config_path = ProjectConfig::config_path(dir.path());
        assert!(config_path.exists());
    }

    #[test]
    fn test_init_project_fails_if_already_exists() {
        let dir = tempdir().unwrap();
        init_project(dir.path(), true).unwrap();

        // Second init should fail
        let result = init_project(dir.path(), true);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("already exists"));
    }

    #[test]
    fn test_update_gitignore_adds_csa() {
        let dir = tempdir().unwrap();
        let gitignore_path = dir.path().join(".gitignore");

        // Create existing .gitignore
        std::fs::write(&gitignore_path, "target/\n*.log\n").unwrap();

        update_gitignore(dir.path()).unwrap();

        let content = std::fs::read_to_string(&gitignore_path).unwrap();
        assert!(content.contains(".csa/"));
        assert!(content.contains("target/"));
        assert!(content.contains("*.log"));
    }

    #[test]
    fn test_update_gitignore_does_not_duplicate() {
        let dir = tempdir().unwrap();
        let gitignore_path = dir.path().join(".gitignore");

        // Create .gitignore with .csa/ already present
        std::fs::write(&gitignore_path, "target/\n.csa/\n*.log\n").unwrap();

        update_gitignore(dir.path()).unwrap();

        let content = std::fs::read_to_string(&gitignore_path).unwrap();
        let csa_count = content
            .lines()
            .filter(|line| line.trim() == ".csa/")
            .count();
        assert_eq!(csa_count, 1, "Should not duplicate .csa/ entry");
    }

    #[test]
    fn test_update_gitignore_no_file_does_nothing() {
        let dir = tempdir().unwrap();
        let gitignore_path = dir.path().join(".gitignore");

        // No .gitignore exists
        assert!(!gitignore_path.exists());

        update_gitignore(dir.path()).unwrap();

        // Still should not exist
        assert!(!gitignore_path.exists());
    }

    #[test]
    fn test_detect_installed_tools() {
        // This test just ensures the function runs without crashing
        let tools = detect_installed_tools();
        // We can't assert specific tools since it depends on the system
        // but we can verify the function returns valid tool names
        for tool in &tools {
            assert!(["gemini-cli", "opencode", "codex", "claude-code"].contains(tool));
        }
    }
}
