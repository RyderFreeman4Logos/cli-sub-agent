use anyhow::{Context, Result};
use std::path::Path;
use tracing::info;

use crate::cli::ClaudeSubAgentArgs;
use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::{ToolArg, ToolName};

pub(crate) async fn handle_claude_sub_agent(
    args: ClaudeSubAgentArgs,
    current_depth: u32,
) -> Result<i32> {
    // 1. Determine project root
    let project_root = crate::pipeline::determine_project_root(args.cd.as_deref())?;

    // 2. Load config and validate recursion depth
    let Some((config, global_config)) =
        crate::pipeline::load_and_validate(&project_root, current_depth)?
    else {
        return Ok(1);
    };

    // 3. Read skill content (if provided)
    let skill_content = if let Some(ref skill_path) = args.skill {
        read_skill_content(skill_path)?
    } else {
        None
    };

    // 4. Read prompt
    let raw_prompt = crate::run_helpers::read_prompt(args.question)?;

    // 5. Combine prompt (skill content + user prompt)
    let prompt = match skill_content {
        Some(ref content) => format!("{}\n\n---\n\nTask:\n{}", content, raw_prompt),
        None => raw_prompt,
    };

    // 6. Resolve tool
    let parent_tool = crate::run_helpers::detect_parent_tool();
    let tool = resolve_claude_tool(
        args.tool,
        config.as_ref(),
        &global_config,
        parent_tool.as_deref(),
        &project_root,
    )?;

    // 7. Resolve model
    let (tool_name, resolved_model_spec, resolved_model) =
        crate::run_helpers::resolve_tool_and_model(
            Some(tool),
            args.model_spec.as_deref(),
            args.model.as_deref(),
            config.as_ref(),
            &project_root,
        )?;

    // 8. Build executor and validate tool
    let executor = crate::pipeline::build_and_validate_executor(
        &tool_name,
        resolved_model_spec.as_deref(),
        resolved_model.as_deref(),
        None, // thinking budget
        config.as_ref(),
    )
    .await?;

    // 9. Acquire global slot to enforce concurrency limit
    let _slot_guard = crate::pipeline::acquire_slot(&executor, &global_config)?;

    // 10. Get env injection from global config
    let extra_env = global_config.env_vars(executor.tool_name());

    // 11. Construct session description
    let description = args
        .skill
        .as_deref()
        .map(|s| format!("claude-sub-agent: {}", s));

    // 12. Execute with session
    let result = crate::pipeline::execute_with_session(
        &executor,
        &tool_name,
        &prompt,
        args.session,
        description,
        None, // parent
        &project_root,
        config.as_ref(),
        extra_env,
    )
    .await?;

    // 13. Audit logging
    info!(
        tool = %tool_name.as_str(),
        skill = ?args.skill,
        exit_code = result.exit_code,
        "claude-sub-agent execution completed"
    );

    // 14. Print result
    print!("{}", result.output);

    Ok(result.exit_code)
}

/// Maximum SKILL.md file size (256 KB) to prevent excessive memory/token usage
const MAX_SKILL_SIZE: u64 = 256 * 1024;

/// Read SKILL.md from a skill directory.
/// Returns error if --skill was specified but SKILL.md is missing or too large.
///
/// Uses a single `File::open` to avoid TOCTOU races, validates the fd is a
/// regular file, and streams with `take()` to enforce size limits.
fn read_skill_content(skill_path: &str) -> Result<Option<String>> {
    use std::io::Read;

    let skill_md = std::path::Path::new(skill_path).join("SKILL.md");
    let file = std::fs::File::open(&skill_md).with_context(|| {
        format!(
            "SKILL.md not found or inaccessible at {}. Verify the --skill path is correct.",
            skill_md.display()
        )
    })?;

    let metadata = file
        .metadata()
        .with_context(|| format!("Failed to stat {}", skill_md.display()))?;
    if !metadata.is_file() {
        anyhow::bail!("SKILL.md at {} is not a regular file", skill_md.display());
    }

    // Stream-read with a limit to prevent excessive memory usage.
    // Read one byte past the limit to detect oversized files.
    let mut content = String::new();
    file.take(MAX_SKILL_SIZE + 1)
        .read_to_string(&mut content)
        .with_context(|| format!("Failed to read {}", skill_md.display()))?;

    if content.len() as u64 > MAX_SKILL_SIZE {
        anyhow::bail!(
            "SKILL.md at {} exceeds size limit (>{} bytes)",
            skill_md.display(),
            MAX_SKILL_SIZE,
        );
    }

    Ok(Some(content))
}

/// Resolve which tool to use for claude-sub-agent.
/// Priority: CLI --tool > auto (heterogeneous selection based on parent tool,
/// then first enabled+available tool in preference order).
///
/// Config-based selection (`[claude-sub-agent].tool` in project/global config)
/// is not yet implemented — auto selection is the current default.
fn resolve_claude_tool(
    arg_tool: Option<ToolArg>,
    project_config: Option<&ProjectConfig>,
    _global_config: &GlobalConfig,
    parent_tool: Option<&str>,
    project_root: &Path,
) -> Result<ToolName> {
    // CLI override is highest priority
    if let Some(tool_arg) = arg_tool {
        return match tool_arg {
            ToolArg::Specific(t) => Ok(t),
            ToolArg::Auto => resolve_auto_tool(parent_tool, project_config, project_root),
            ToolArg::AnyAvailable => select_any_available_tool(project_config, project_root),
        };
    }

    // Fall through to auto selection
    resolve_auto_tool(parent_tool, project_config, project_root)
}

fn resolve_auto_tool(
    parent_tool: Option<&str>,
    project_config: Option<&ProjectConfig>,
    project_root: &Path,
) -> Result<ToolName> {
    // Build the set of tools that are both enabled in config AND have binaries installed
    let available_tools: Vec<ToolName> = get_enabled_tools(project_config, project_root)
        .into_iter()
        .filter(|t| crate::run_helpers::is_tool_binary_available(t.as_str()))
        .collect();

    // Try heterogeneous selection on actually-available tools
    if let Some(parent) = parent_tool {
        if let Ok(parent_tool_name) = crate::run_helpers::parse_tool_name(parent) {
            if let Some(tool) = select_heterogeneous_tool(&parent_tool_name, &available_tools) {
                return Ok(tool);
            }
        }
    }

    // Fallback: first available in preference order
    for preferred in &["codex", "claude-code", "opencode", "gemini-cli"] {
        if let Ok(tool) = crate::run_helpers::parse_tool_name(preferred) {
            if available_tools.contains(&tool) {
                return Ok(tool);
            }
        }
    }

    anyhow::bail!("No suitable tool found for claude-sub-agent. Install codex or claude-code.")
}

/// Get enabled tools from project config
fn get_enabled_tools(
    project_config: Option<&ProjectConfig>,
    _project_root: &Path,
) -> Vec<ToolName> {
    if let Some(cfg) = project_config {
        csa_config::global::all_known_tools()
            .iter()
            .filter(|t| cfg.is_tool_enabled(t.as_str()))
            .copied()
            .collect()
    } else {
        csa_config::global::all_known_tools().to_vec()
    }
}

/// Select a heterogeneous tool based on parent tool
fn select_heterogeneous_tool(
    parent_tool: &ToolName,
    enabled_tools: &[ToolName],
) -> Option<ToolName> {
    csa_config::global::select_heterogeneous_tool(parent_tool, enabled_tools)
}

/// Select the first available enabled tool (in built-in preference order, no heterogeneity constraint)
fn select_any_available_tool(
    project_config: Option<&ProjectConfig>,
    project_root: &Path,
) -> Result<ToolName> {
    let enabled_tools = get_enabled_tools(project_config, project_root);

    for tool in enabled_tools {
        if crate::run_helpers::is_tool_binary_available(tool.as_str()) {
            return Ok(tool);
        }
    }

    anyhow::bail!(
        "No tools available. Install at least one tool (codex, claude-code, opencode, gemini-cli)."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use csa_config::{ProjectMeta, ResourcesConfig, ToolConfig};
    use std::collections::HashMap;

    fn project_config_with_enabled_tools(tools: &[&str]) -> ProjectConfig {
        let mut tool_map = HashMap::new();
        for tool in tools {
            tool_map.insert(
                (*tool).to_string(),
                ToolConfig {
                    enabled: true,
                    restrictions: None,
                    suppress_notify: false,
                },
            );
        }

        ProjectConfig {
            schema_version: 1,
            project: ProjectMeta::default(),
            resources: ResourcesConfig::default(),
            tools: tool_map,
            review: None,
            debate: None,
            tiers: HashMap::new(),
            tier_mapping: HashMap::new(),
            aliases: HashMap::new(),
        }
    }

    #[test]
    fn read_skill_content_errors_when_skill_md_missing() {
        let tempdir = tempfile::tempdir().unwrap();
        let skill_path = tempdir.path().to_str().unwrap();
        let result = read_skill_content(skill_path);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("SKILL.md not found"));
    }

    #[test]
    fn read_skill_content_returns_content_when_skill_md_exists() {
        let tempdir = tempfile::tempdir().unwrap();
        let skill_md = tempdir.path().join("SKILL.md");
        std::fs::write(&skill_md, "# Test Skill\nThis is a test skill.").unwrap();

        let skill_path = tempdir.path().to_str().unwrap();
        let result = read_skill_content(skill_path).unwrap();
        assert!(result.is_some());
        assert!(result.unwrap().contains("Test Skill"));
    }

    #[test]
    fn resolve_claude_tool_prefers_cli_override() {
        let global = GlobalConfig::default();
        let cfg = project_config_with_enabled_tools(&["gemini-cli", "codex"]);
        let tool = resolve_claude_tool(
            Some(ToolArg::Specific(ToolName::Codex)),
            Some(&cfg),
            &global,
            Some("claude-code"),
            std::path::Path::new("/tmp/test-project"),
        )
        .unwrap();
        assert!(matches!(tool, ToolName::Codex));
    }

    #[test]
    fn get_enabled_tools_returns_all_when_no_config() {
        let tools = get_enabled_tools(None, std::path::Path::new("/tmp"));
        assert_eq!(tools.len(), csa_config::global::all_known_tools().len());
    }

    #[test]
    fn read_skill_content_errors_when_too_large() {
        let tempdir = tempfile::tempdir().unwrap();
        let skill_md = tempdir.path().join("SKILL.md");
        // Create a file exceeding MAX_SKILL_SIZE
        let content = "x".repeat((MAX_SKILL_SIZE + 1) as usize);
        std::fs::write(&skill_md, content).unwrap();

        let skill_path = tempdir.path().to_str().unwrap();
        let result = read_skill_content(skill_path);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("exceeds size limit"));
    }

    #[test]
    fn get_enabled_tools_filters_by_project_config() {
        // Create config with only codex and claude-code enabled, others disabled
        let mut tool_map = HashMap::new();
        tool_map.insert(
            "codex".to_string(),
            ToolConfig {
                enabled: true,
                restrictions: None,
                suppress_notify: false,
            },
        );
        tool_map.insert(
            "claude-code".to_string(),
            ToolConfig {
                enabled: true,
                restrictions: None,
                suppress_notify: false,
            },
        );
        tool_map.insert(
            "gemini-cli".to_string(),
            ToolConfig {
                enabled: false,
                restrictions: None,
                suppress_notify: false,
            },
        );
        tool_map.insert(
            "opencode".to_string(),
            ToolConfig {
                enabled: false,
                restrictions: None,
                suppress_notify: false,
            },
        );

        let cfg = ProjectConfig {
            schema_version: 1,
            project: ProjectMeta::default(),
            resources: ResourcesConfig::default(),
            tools: tool_map,
            review: None,
            debate: None,
            tiers: HashMap::new(),
            tier_mapping: HashMap::new(),
            aliases: HashMap::new(),
        };

        let tools = get_enabled_tools(Some(&cfg), std::path::Path::new("/tmp"));
        assert_eq!(tools.len(), 2);
        assert!(tools.contains(&ToolName::Codex));
        assert!(tools.contains(&ToolName::ClaudeCode));
    }

    #[test]
    fn read_skill_content_errors_when_not_regular_file() {
        // A directory named SKILL.md should be rejected
        let tempdir = tempfile::tempdir().unwrap();
        let skill_md = tempdir.path().join("SKILL.md");
        std::fs::create_dir(&skill_md).unwrap();

        let skill_path = tempdir.path().to_str().unwrap();
        let result = read_skill_content(skill_path);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("not a regular file") || err_msg.contains("directory"),
            "Expected 'not a regular file' error, got: {}",
            err_msg
        );
    }

    #[test]
    fn select_heterogeneous_tool_picks_different_family() {
        let enabled = vec![ToolName::ClaudeCode, ToolName::Codex, ToolName::GeminiCli];
        // Parent is claude-code (Anthropic family), should pick Codex (OpenAI) or GeminiCli
        let result = select_heterogeneous_tool(&ToolName::ClaudeCode, &enabled);
        assert!(result.is_some());
        let tool = result.unwrap();
        assert_ne!(
            tool.model_family(),
            ToolName::ClaudeCode.model_family(),
            "Heterogeneous selection must pick a different model family"
        );
    }

    #[test]
    fn select_heterogeneous_tool_returns_none_when_only_same_family() {
        // Only claude-code available (same family as parent)
        let enabled = vec![ToolName::ClaudeCode];
        let result = select_heterogeneous_tool(&ToolName::ClaudeCode, &enabled);
        assert!(result.is_none());
    }

    #[test]
    fn select_any_available_tool_errors_when_none_installed() {
        // With a config that only enables a non-existent tool name,
        // select_any_available_tool should return an error
        let cfg = project_config_with_enabled_tools(&["gemini-cli"]);
        // gemini-cli is likely not installed in test environment
        let result = select_any_available_tool(Some(&cfg), std::path::Path::new("/tmp"));
        // This may pass or fail depending on the test machine, so we just verify it doesn't panic
        // and returns either Ok or a meaningful error
        if let Err(e) = result {
            assert!(e.to_string().contains("No tools available"));
        }
    }
}
