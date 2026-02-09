use anyhow::{Context, Result};
use std::path::Path;
use tracing::error;

use crate::cli::DebateArgs;
use crate::run_helpers::read_prompt;
use csa_config::global::heterogeneous_counterpart;
use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::ToolName;
use csa_process::check_tool_installed;

pub(crate) async fn handle_debate(args: DebateArgs, current_depth: u32) -> Result<i32> {
    // 1. Determine project root
    let project_root = crate::determine_project_root(args.cd.as_deref())?;

    // 2. Load config (optional)
    let config = ProjectConfig::load(&project_root)?;

    // 3. Check recursion depth
    let max_depth = config
        .as_ref()
        .map(|c| c.project.max_recursion_depth)
        .unwrap_or(5u32);
    if current_depth > max_depth {
        error!(
            "Max recursion depth ({}) exceeded. Current: {}. Do it yourself.",
            max_depth, current_depth
        );
        return Ok(1);
    }

    // 4. Load global config for debate tool selection + env injection + slot control.
    let global_config = GlobalConfig::load()?;

    // 5. Read question (from arg or stdin)
    let question = read_prompt(args.question)?;

    // 6. Construct debate prompt
    let prompt = construct_debate_prompt(&question, args.session.is_some());

    // 7. Determine tool (heterogeneous enforcement)
    let parent_tool = std::env::var("CSA_TOOL")
        .ok()
        .or_else(|| std::env::var("CSA_PARENT_TOOL").ok());
    let tool = resolve_debate_tool(
        args.tool,
        config.as_ref(),
        &global_config,
        parent_tool.as_deref(),
        &project_root,
    )?;

    // 8. Build executor
    let executor = crate::run_helpers::build_executor(
        &tool,
        None,
        args.model.as_deref(),
        None,
        config.as_ref(),
    )?;

    // 9. Check tool is installed
    if let Err(e) = check_tool_installed(executor.executable_name()).await {
        error!(
            "Tool '{}' is not installed.\n\n{}\n\nOr disable it in .csa/config.toml:\n  [tools.{}]\n  enabled = false",
            executor.tool_name(),
            executor.install_hint(),
            executor.tool_name()
        );
        anyhow::bail!("{}", e);
    }

    // 10. Check tool is enabled in config
    if let Some(ref cfg) = config {
        if !cfg.is_tool_enabled(executor.tool_name()) {
            error!(
                "Tool '{}' is disabled in project config",
                executor.tool_name()
            );
            return Ok(1);
        }
    }

    // 11. Get env injection from global config
    let extra_env = global_config.env_vars(executor.tool_name());

    // 12. Acquire global slot to enforce concurrency limit
    let max_concurrent = global_config.max_concurrent(executor.tool_name());
    let slots_dir = GlobalConfig::slots_dir()?;
    let _slot_guard = match csa_lock::slot::try_acquire_slot(
        &slots_dir,
        executor.tool_name(),
        max_concurrent,
        None,
    ) {
        Ok(csa_lock::slot::SlotAcquireResult::Acquired(slot)) => slot,
        Ok(csa_lock::slot::SlotAcquireResult::Exhausted(status)) => {
            anyhow::bail!(
                "All {} slots for '{}' occupied ({}/{}). Try again later or use --tool to switch.",
                max_concurrent,
                executor.tool_name(),
                status.occupied,
                status.max_slots,
            );
        }
        Err(e) => {
            anyhow::bail!(
                "Slot acquisition failed for '{}': {}",
                executor.tool_name(),
                e
            );
        }
    };

    // 13. Execute with session
    let result = crate::execute_with_session(
        &executor,
        &tool,
        &prompt,
        args.session,
        Some("Debate session".to_string()),
        None,
        &project_root,
        config.as_ref(),
        extra_env,
    )
    .await?;

    // 14. Print result
    print!("{}", result.output);

    Ok(result.exit_code)
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
        return resolve_debate_tool_from_value(&project_debate.tool, parent_tool, project_root)
            .with_context(|| {
                format!(
                    "Failed to resolve debate tool from project config: {}",
                    ProjectConfig::config_path(project_root).display()
                )
            });
    }

    // Global config [debate] section
    match global_config.resolve_debate_tool(parent_tool) {
        Ok(tool_name) => parse_tool_name(&tool_name).ok_or_else(|| {
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
    project_root: &Path,
) -> Result<ToolName> {
    if tool_value == "auto" {
        let resolved = parent_tool
            .and_then(heterogeneous_counterpart)
            .ok_or_else(|| debate_auto_resolution_error(parent_tool, project_root))?;
        return parse_tool_name(resolved).ok_or_else(|| {
            anyhow::anyhow!(
                "BUG: auto debate tool resolution returned invalid tool '{}'",
                resolved
            )
        });
    }

    parse_tool_name(tool_value).ok_or_else(|| {
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

fn parse_tool_name(name: &str) -> Option<ToolName> {
    match name {
        "gemini-cli" => Some(ToolName::GeminiCli),
        "opencode" => Some(ToolName::Opencode),
        "codex" => Some(ToolName::Codex),
        "claude-code" => Some(ToolName::ClaudeCode),
        _ => None,
    }
}

/// Construct a debate prompt that frames the model as a debate participant.
///
/// When `is_continuation` is true (resuming a session), the prompt is lighter —
/// the model already has the debate context from previous turns.
fn construct_debate_prompt(question: &str, is_continuation: bool) -> String {
    if is_continuation {
        // Continuing an existing debate — the session already has context.
        // The question here is the caller's counterpoint or follow-up.
        format!(
            "The following is a counterpoint or follow-up in our ongoing debate. \
Respond directly to the arguments presented. Be specific, cite evidence, \
and concede valid points while defending your position where warranted.\n\n\
{question}"
        )
    } else {
        // New debate — frame the model's role clearly.
        format!(
            "You are participating in an adversarial debate to stress-test ideas \
through model heterogeneity. A different AI model (the caller) will evaluate \
your response and may counter-argue in subsequent rounds.\n\n\
Analyze the following question or proposal thoroughly:\n\n\
{question}\n\n\
Structure your response as:\n\
1. **Position**: Your concrete stance or proposed solution (2-3 sentences)\n\
2. **Key Arguments**: Numbered, with evidence and reasoning\n\
3. **Implementation**: Concrete actionable steps (if applicable)\n\
4. **Anticipated Counterarguments**: Honestly acknowledge weaknesses and preemptively address them\n\n\
Be intellectually rigorous. Take a clear position — do not hedge or give a \"it depends\" non-answer."
        )
    }
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
        assert!(err
            .to_string()
            .contains("AUTO debate tool selection failed"));
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
        assert!(err
            .to_string()
            .contains("AUTO debate tool selection failed"));
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
    fn construct_debate_prompt_new_debate() {
        let prompt = construct_debate_prompt("Should we use gRPC or REST?", false);
        assert!(prompt.contains("adversarial debate"));
        assert!(prompt.contains("Should we use gRPC or REST?"));
        assert!(prompt.contains("Position"));
        assert!(prompt.contains("Anticipated Counterarguments"));
    }

    #[test]
    fn construct_debate_prompt_continuation() {
        let prompt = construct_debate_prompt("I disagree because X", true);
        assert!(prompt.contains("counterpoint"));
        assert!(prompt.contains("I disagree because X"));
        assert!(!prompt.contains("Structure your response as"));
    }
}
