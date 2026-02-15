use anyhow::{Context, Result};
use std::path::Path;

use crate::cli::DebateArgs;
use crate::run_helpers::read_prompt;
use csa_config::global::{heterogeneous_counterpart, select_heterogeneous_tool};
use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::ToolName;

pub(crate) async fn handle_debate(args: DebateArgs, current_depth: u32) -> Result<i32> {
    // 1. Determine project root
    let project_root = crate::pipeline::determine_project_root(args.cd.as_deref())?;

    // 2. Load config and validate recursion depth
    let Some((config, global_config)) =
        crate::pipeline::load_and_validate(&project_root, current_depth)?
    else {
        return Ok(1);
    };

    // 3. Read question (from arg or stdin)
    let question = read_prompt(args.question)?;

    // 4. Build debate instruction (parameter passing — tool loads debate skill)
    let prompt = build_debate_instruction(&question, args.session.is_some());

    // 5. Determine tool (heterogeneous enforcement)
    let detected_parent_tool = crate::run_helpers::detect_parent_tool();
    let parent_tool = crate::run_helpers::resolve_tool(detected_parent_tool, &global_config);
    let tool = resolve_debate_tool(
        args.tool,
        config.as_ref(),
        &global_config,
        parent_tool.as_deref(),
        &project_root,
    )?;

    // 6. Build executor and validate tool
    let executor = crate::pipeline::build_and_validate_executor(
        &tool,
        None,
        args.model.as_deref(),
        None,
        config.as_ref(),
    )
    .await?;

    // 7. Get env injection from global config
    let extra_env = global_config.env_vars(executor.tool_name());
    let idle_timeout_seconds = crate::pipeline::resolve_idle_timeout_seconds(config.as_ref(), None);

    // 8. Acquire global slot to enforce concurrency limit
    let _slot_guard = crate::pipeline::acquire_slot(&executor, &global_config)?;

    // 9. Execute with session
    let description = format!(
        "debate: {}",
        crate::run_helpers::truncate_prompt(&question, 80)
    );
    let execution = crate::pipeline::execute_with_session_and_meta(
        &executor,
        &tool,
        &prompt,
        args.session,
        Some(description),
        None,
        &project_root,
        config.as_ref(),
        extra_env,
        Some("debate"),
        None, // debate does not use tier-based selection
        csa_process::StreamMode::BufferOnly,
        idle_timeout_seconds,
        Some(&global_config),
    )
    .await?;

    let output = render_debate_output(
        &execution.execution.output,
        &execution.meta_session_id,
        execution.provider_session_id.as_deref(),
    );

    // 10. Print result
    print!("{output}");

    Ok(execution.execution.exit_code)
}

fn resolve_debate_tool(
    arg_tool: Option<ToolName>,
    project_config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
    parent_tool: Option<&str>,
    project_root: &Path,
) -> Result<ToolName> {
    // CLI --tool override always wins
    if let Some(tool) = arg_tool {
        return Ok(tool);
    }

    // Project-level [debate] config override
    if let Some(project_debate) = project_config.and_then(|cfg| cfg.debate.as_ref()) {
        return resolve_debate_tool_from_value(
            &project_debate.tool,
            parent_tool,
            project_config,
            project_root,
        )
        .with_context(|| {
            format!(
                "Failed to resolve debate tool from project config: {}",
                ProjectConfig::config_path(project_root).display()
            )
        });
    }

    // Global config [debate] section
    match global_config.resolve_debate_tool(parent_tool) {
        Ok(tool_name) => crate::run_helpers::parse_tool_name(&tool_name).map_err(|_| {
            anyhow::anyhow!(
                "Invalid [debate].tool value '{}'. Supported values: gemini-cli, opencode, codex, claude-code.",
                tool_name
            )
        }),
        Err(_) => Err(debate_auto_resolution_error(parent_tool, project_root)),
    }
}

fn resolve_debate_tool_from_value(
    tool_value: &str,
    parent_tool: Option<&str>,
    project_config: Option<&ProjectConfig>,
    project_root: &Path,
) -> Result<ToolName> {
    if tool_value == "auto" {
        // Try old heterogeneous_counterpart first for backward compatibility
        if let Some(resolved) = parent_tool.and_then(heterogeneous_counterpart) {
            return crate::run_helpers::parse_tool_name(resolved).map_err(|_| {
                anyhow::anyhow!(
                    "BUG: auto debate tool resolution returned invalid tool '{}'",
                    resolved
                )
            });
        }

        // Fallback to new ModelFamily-based selection (filtered by enabled tools)
        if let Some(parent_str) = parent_tool {
            if let Ok(parent_tool_name) = crate::run_helpers::parse_tool_name(parent_str) {
                let enabled_tools: Vec<_> = if let Some(cfg) = project_config {
                    csa_config::global::all_known_tools()
                        .iter()
                        .filter(|t| cfg.is_tool_enabled(t.as_str()))
                        .copied()
                        .collect()
                } else {
                    csa_config::global::all_known_tools().to_vec()
                };
                if let Some(tool) = select_heterogeneous_tool(&parent_tool_name, &enabled_tools) {
                    return Ok(tool);
                }
            }
        }

        // Both methods failed
        return Err(debate_auto_resolution_error(parent_tool, project_root));
    }

    crate::run_helpers::parse_tool_name(tool_value).map_err(|_| {
        anyhow::anyhow!(
            "Invalid project [debate].tool value '{}'. Supported values: auto, gemini-cli, opencode, codex, claude-code.",
            tool_value
        )
    })
}

fn debate_auto_resolution_error(parent_tool: Option<&str>, project_root: &Path) -> anyhow::Error {
    let parent = parent_tool.unwrap_or("<none>").escape_default().to_string();
    let global_path = GlobalConfig::config_path()
        .ok()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "~/.config/cli-sub-agent/config.toml".to_string());
    let project_path = ProjectConfig::config_path(project_root)
        .display()
        .to_string();

    anyhow::anyhow!(
        "AUTO debate tool selection failed (tool = \"auto\").\n\n\
STOP: Do not proceed. Ask the user to configure the debate tool explicitly.\n\n\
Parent tool context: {parent}\n\
Supported auto mapping: claude-code <-> codex\n\n\
Choose one:\n\
1) Global config (user-level): {global_path}\n\
   [debate]\n\
   tool = \"codex\"  # or \"claude-code\", \"opencode\", \"gemini-cli\"\n\
2) Project config override: {project_path}\n\
   [debate]\n\
   tool = \"codex\"  # or \"claude-code\", \"opencode\", \"gemini-cli\"\n\
3) CLI override: csa debate --tool codex\n\n\
Reason: CSA enforces heterogeneity in auto mode and will not fall back."
    )
}

/// Build a debate instruction that passes parameters to the debate skill.
///
/// The debate tool loads the debate skill from the project's `.claude/skills/`
/// directory and follows its instructions autonomously. We only pass parameters.
fn build_debate_instruction(question: &str, is_continuation: bool) -> String {
    if is_continuation {
        format!("Use the debate skill. continuation=true. question={question}")
    } else {
        format!("Use the debate skill. question={question}")
    }
}

fn render_debate_output(
    tool_output: &str,
    meta_session_id: &str,
    provider_session_id: Option<&str>,
) -> String {
    let mut output = match provider_session_id {
        Some(provider_id) => tool_output.replace(provider_id, meta_session_id),
        None => tool_output.to_string(),
    };

    if !output.is_empty() && !output.ends_with('\n') {
        output.push('\n');
    }

    output.push_str(&format!("CSA Meta Session ID: {meta_session_id}\n"));
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use csa_config::global::ReviewConfig;
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
                    suppress_notify: true,
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
    fn resolve_debate_tool_prefers_cli_override() {
        let global = GlobalConfig::default();
        let cfg = project_config_with_enabled_tools(&["gemini-cli"]);
        let tool = resolve_debate_tool(
            Some(ToolName::Codex),
            Some(&cfg),
            &global,
            Some("claude-code"),
            std::path::Path::new("/tmp/test-project"),
        )
        .unwrap();
        assert!(matches!(tool, ToolName::Codex));
    }

    #[test]
    fn resolve_debate_tool_auto_maps_heterogeneous() {
        let global = GlobalConfig::default();
        let cfg = project_config_with_enabled_tools(&["codex"]);
        let tool = resolve_debate_tool(
            None,
            Some(&cfg),
            &global,
            Some("claude-code"),
            std::path::Path::new("/tmp/test-project"),
        )
        .unwrap();
        assert!(matches!(tool, ToolName::Codex));
    }

    #[test]
    fn resolve_debate_tool_auto_maps_reverse() {
        let global = GlobalConfig::default();
        let cfg = project_config_with_enabled_tools(&["claude-code"]);
        let tool = resolve_debate_tool(
            None,
            Some(&cfg),
            &global,
            Some("codex"),
            std::path::Path::new("/tmp/test-project"),
        )
        .unwrap();
        assert!(matches!(tool, ToolName::ClaudeCode));
    }

    #[test]
    fn resolve_debate_tool_errors_without_parent_context() {
        let global = GlobalConfig::default();
        let cfg = project_config_with_enabled_tools(&["opencode"]);
        let err = resolve_debate_tool(
            None,
            Some(&cfg),
            &global,
            None,
            std::path::Path::new("/tmp/test-project"),
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("AUTO debate tool selection failed")
        );
    }

    #[test]
    fn resolve_debate_tool_errors_on_unknown_parent() {
        let global = GlobalConfig::default();
        let cfg = project_config_with_enabled_tools(&["opencode"]);
        let err = resolve_debate_tool(
            None,
            Some(&cfg),
            &global,
            Some("opencode"),
            std::path::Path::new("/tmp/test-project"),
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("AUTO debate tool selection failed")
        );
    }

    #[test]
    fn resolve_debate_tool_prefers_project_override() {
        let global = GlobalConfig::default();
        let mut cfg = project_config_with_enabled_tools(&["codex", "opencode"]);
        cfg.debate = Some(ReviewConfig {
            tool: "opencode".to_string(),
        });

        let tool = resolve_debate_tool(
            None,
            Some(&cfg),
            &global,
            Some("claude-code"),
            std::path::Path::new("/tmp/test-project"),
        )
        .unwrap();
        assert!(matches!(tool, ToolName::Opencode));
    }

    #[test]
    fn resolve_debate_tool_project_auto_maps_heterogeneous() {
        let global = GlobalConfig::default();
        let mut cfg = project_config_with_enabled_tools(&["codex", "claude-code"]);
        cfg.debate = Some(ReviewConfig {
            tool: "auto".to_string(),
        });

        let tool = resolve_debate_tool(
            None,
            Some(&cfg),
            &global,
            Some("claude-code"),
            std::path::Path::new("/tmp/test-project"),
        )
        .unwrap();
        assert!(matches!(tool, ToolName::Codex));
    }

    #[test]
    fn build_debate_instruction_new_debate() {
        let prompt = build_debate_instruction("Should we use gRPC or REST?", false);
        assert!(prompt.contains("debate skill"));
        assert!(prompt.contains("Should we use gRPC or REST?"));
        assert!(!prompt.contains("continuation=true"));
    }

    #[test]
    fn build_debate_instruction_continuation() {
        let prompt = build_debate_instruction("I disagree because X", true);
        assert!(prompt.contains("debate skill"));
        assert!(prompt.contains("continuation=true"));
        assert!(prompt.contains("I disagree because X"));
    }

    #[test]
    fn render_debate_output_appends_meta_session_id() {
        let output = render_debate_output("debate answer", "01ARZ3NDEKTSV4RRFFQ69G5FAV", None);
        assert!(output.contains("debate answer"));
        assert!(output.contains("CSA Meta Session ID: 01ARZ3NDEKTSV4RRFFQ69G5FAV"));
    }

    #[test]
    fn render_debate_output_replaces_provider_id_with_meta_id() {
        let provider = "019c5589-3c84-7f03-b9c4-9f0a164c4eb2";
        let meta = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
        let tool_output = format!("session_id={provider}\nresult=ok");

        let output = render_debate_output(&tool_output, meta, Some(provider));
        assert!(!output.contains(provider));
        assert!(output.contains(meta));
    }
}
