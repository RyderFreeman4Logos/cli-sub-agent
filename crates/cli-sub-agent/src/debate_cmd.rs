use std::io::IsTerminal;

use anyhow::{Context, Result};
use std::path::Path;
use tracing::{debug, error};

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

    // 2b. Verify debate skill is available (fail fast before any execution)
    verify_debate_skill_available(&project_root)?;

    // 3. Read question (from arg or stdin)
    let question = read_prompt(args.question)?;

    // 4. Build debate instruction (parameter passing — tool loads debate skill)
    let prompt = build_debate_instruction(&question, args.session.is_some(), args.rounds);

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
        false, // skip tier whitelist for debate tool selection
    )
    .await?;

    // 7. Get env injection from global config
    let extra_env = global_config.env_vars(executor.tool_name());
    let idle_timeout_seconds =
        crate::pipeline::resolve_idle_timeout_seconds(config.as_ref(), args.idle_timeout);

    // Resolve stream mode from CLI flags (default: BufferOnly for debate)
    let stream_mode = resolve_debate_stream_mode(args.stream_stdout, args.no_stream_stdout);

    // 8. Acquire global slot to enforce concurrency limit
    let _slot_guard = crate::pipeline::acquire_slot(&executor, &global_config)?;

    // 9. Execute with session (with optional absolute timeout)
    let description = format!(
        "debate: {}",
        crate::run_helpers::truncate_prompt(&question, 80)
    );
    let execute_future = crate::pipeline::execute_with_session_and_meta(
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
        None, // debate does not override context loading options
        stream_mode,
        idle_timeout_seconds,
        Some(&global_config),
    );

    let timeout_secs = resolve_debate_timeout_seconds(args.timeout, &global_config);
    let execution =
        match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), execute_future)
            .await
        {
            Ok(inner) => inner?,
            Err(_) => {
                error!(
                    timeout_secs = timeout_secs,
                    "Debate aborted: wall-clock timeout exceeded"
                );
                anyhow::bail!(
                    "Debate aborted: timeout {timeout_secs}s exceeded. \
                     Use --timeout to override, or set [debate].timeout_seconds in global config."
                );
            }
        };

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
            global_config,
            project_root,
        )
        .with_context(|| {
            format!(
                "Failed to resolve debate tool from project config: {}",
                ProjectConfig::config_path(project_root).display()
            )
        });
    }

    // When global [debate].tool is "auto", try priority-aware selection first
    if global_config.debate.tool == "auto" {
        let has_known_priority =
            csa_config::global::effective_tool_priority(project_config, global_config)
                .iter()
                .any(|entry| {
                    csa_config::global::all_known_tools()
                        .iter()
                        .any(|tool| tool.as_str() == entry)
                });
        if has_known_priority {
            if let Some(tool) = select_auto_debate_tool(parent_tool, project_config, global_config)
            {
                return Ok(tool);
            }
        }
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
    global_config: &GlobalConfig,
    project_root: &Path,
) -> Result<ToolName> {
    if tool_value == "auto" {
        let has_known_priority =
            csa_config::global::effective_tool_priority(project_config, global_config)
                .iter()
                .any(|entry| {
                    csa_config::global::all_known_tools()
                        .iter()
                        .any(|tool| tool.as_str() == entry)
                });
        if has_known_priority {
            if let Some(tool) = select_auto_debate_tool(parent_tool, project_config, global_config)
            {
                return Ok(tool);
            }
        }

        // Try old heterogeneous_counterpart first for backward compatibility
        if let Some(resolved) = parent_tool.and_then(heterogeneous_counterpart) {
            return crate::run_helpers::parse_tool_name(resolved).map_err(|_| {
                anyhow::anyhow!(
                    "BUG: auto debate tool resolution returned invalid tool '{}'",
                    resolved
                )
            });
        }

        // Fallback to ModelFamily-based selection (filtered by enabled tools)
        if let Some(tool) = select_auto_debate_tool(parent_tool, project_config, global_config) {
            return Ok(tool);
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

fn select_auto_debate_tool(
    parent_tool: Option<&str>,
    project_config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
) -> Option<ToolName> {
    let parent_str = parent_tool?;
    let parent_tool_name = crate::run_helpers::parse_tool_name(parent_str).ok()?;
    let enabled_tools: Vec<_> = if let Some(cfg) = project_config {
        let tools: Vec<_> = csa_config::global::all_known_tools()
            .iter()
            .filter(|t| cfg.is_tool_auto_selectable(t.as_str()))
            .copied()
            .collect();
        csa_config::global::sort_tools_by_effective_priority(&tools, project_config, global_config)
    } else {
        csa_config::global::sort_tools_by_effective_priority(
            csa_config::global::all_known_tools(),
            project_config,
            global_config,
        )
    };

    select_heterogeneous_tool(&parent_tool_name, &enabled_tools)
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

/// Verify the debate pattern is installed before attempting execution.
///
/// Fails fast with actionable install guidance if the pattern is missing,
/// preventing silent degradation where the tool runs without skill context.
fn verify_debate_skill_available(project_root: &Path) -> Result<()> {
    match crate::pattern_resolver::resolve_pattern("debate", project_root) {
        Ok(resolved) => {
            debug!(
                pattern_dir = %resolved.dir.display(),
                has_config = resolved.config.is_some(),
                skill_md_len = resolved.skill_md.len(),
                "Debate pattern resolved"
            );
            Ok(())
        }
        Err(resolve_err) => {
            anyhow::bail!(
                "Debate pattern not found — `csa debate` requires the 'debate' pattern.\n\n\
                 {resolve_err}\n\n\
                 Install the debate pattern with one of:\n\
                 1) csa skill install RyderFreeman4Logos/cli-sub-agent\n\
                 2) Manually place skills/debate/SKILL.md inside .csa/patterns/debate/ or patterns/debate/\n\n\
                 Without the pattern, the debate tool cannot follow the structured debate protocol."
            )
        }
    }
}

/// Resolve stream mode for debate command.
///
/// - `--stream-stdout` forces TeeToStderr (progressive output)
/// - `--no-stream-stdout` forces BufferOnly (silent until complete)
/// - Default: auto-detect TTY on stderr -> TeeToStderr if interactive,
///   BufferOnly otherwise. Symmetric with review's behavior (#139).
fn resolve_debate_stream_mode(
    stream_stdout: bool,
    no_stream_stdout: bool,
) -> csa_process::StreamMode {
    if no_stream_stdout {
        csa_process::StreamMode::BufferOnly
    } else if stream_stdout || std::io::stderr().is_terminal() {
        csa_process::StreamMode::TeeToStderr
    } else {
        csa_process::StreamMode::BufferOnly
    }
}

fn resolve_debate_timeout_seconds(
    timeout_override: Option<u64>,
    global_config: &GlobalConfig,
) -> u64 {
    timeout_override.unwrap_or(global_config.debate.timeout_seconds)
}

/// Build a debate instruction that passes parameters to the debate skill.
///
/// The debate tool loads the debate skill from the project's `.claude/skills/`
/// directory and follows its instructions autonomously. We only pass parameters.
fn build_debate_instruction(question: &str, is_continuation: bool, rounds: u32) -> String {
    if is_continuation {
        format!("Use the debate skill. continuation=true. rounds={rounds}. question={question}")
    } else {
        format!("Use the debate skill. rounds={rounds}. question={question}")
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
                    ..Default::default()
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
            preferences: None,
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
    fn resolve_debate_tool_project_auto_prefers_priority_over_counterpart() {
        let mut global = GlobalConfig::default();
        global.preferences.tool_priority = vec!["opencode".to_string(), "claude-code".to_string()];

        let mut cfg = project_config_with_enabled_tools(&["codex", "claude-code", "opencode"]);
        cfg.debate = Some(ReviewConfig {
            tool: "auto".to_string(),
        });

        let tool = resolve_debate_tool(
            None,
            Some(&cfg),
            &global,
            Some("codex"),
            std::path::Path::new("/tmp/test-project"),
        )
        .unwrap();
        assert!(matches!(tool, ToolName::Opencode));
    }

    #[test]
    fn resolve_debate_tool_ignores_unknown_priority_entries() {
        let mut global = GlobalConfig::default();
        global.preferences.tool_priority = vec!["codexx".to_string()];

        let mut cfg = project_config_with_enabled_tools(&["codex", "claude-code", "opencode"]);
        cfg.debate = Some(ReviewConfig {
            tool: "auto".to_string(),
        });

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
    fn build_debate_instruction_new_debate() {
        let prompt = build_debate_instruction("Should we use gRPC or REST?", false, 3);
        assert!(prompt.contains("debate skill"));
        assert!(prompt.contains("Should we use gRPC or REST?"));
        assert!(!prompt.contains("continuation=true"));
        assert!(prompt.contains("rounds=3"));
    }

    #[test]
    fn build_debate_instruction_continuation() {
        let prompt = build_debate_instruction("I disagree because X", true, 3);
        assert!(prompt.contains("debate skill"));
        assert!(prompt.contains("continuation=true"));
        assert!(prompt.contains("I disagree because X"));
        assert!(prompt.contains("rounds=3"));
    }

    #[test]
    fn build_debate_instruction_custom_rounds() {
        let prompt = build_debate_instruction("topic", false, 5);
        assert!(prompt.contains("rounds=5"));
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

    // --- CLI parse tests for timeout/stream flags (#146) ---

    fn parse_debate_args(argv: &[&str]) -> crate::cli::DebateArgs {
        use crate::cli::{Cli, Commands};
        use clap::Parser;
        let cli = Cli::try_parse_from(argv).expect("debate CLI args should parse");
        match cli.command {
            Commands::Debate(args) => args,
            _ => panic!("expected debate subcommand"),
        }
    }

    #[test]
    fn debate_cli_parses_timeout_flag() {
        let args = parse_debate_args(&["csa", "debate", "--timeout", "120", "question"]);
        assert_eq!(args.timeout, Some(120));
    }

    #[test]
    fn debate_cli_parses_idle_timeout_flag() {
        let args = parse_debate_args(&["csa", "debate", "--idle-timeout", "60", "question"]);
        assert_eq!(args.idle_timeout, Some(60));
    }

    #[test]
    fn debate_cli_parses_both_timeouts() {
        let args = parse_debate_args(&[
            "csa",
            "debate",
            "--timeout",
            "300",
            "--idle-timeout",
            "30",
            "question",
        ]);
        assert_eq!(args.timeout, Some(300));
        assert_eq!(args.idle_timeout, Some(30));
    }

    #[test]
    fn debate_cli_parses_stream_stdout_flag() {
        let args = parse_debate_args(&["csa", "debate", "--stream-stdout", "question"]);
        assert!(args.stream_stdout);
        assert!(!args.no_stream_stdout);
    }

    #[test]
    fn debate_cli_parses_no_stream_stdout_flag() {
        let args = parse_debate_args(&["csa", "debate", "--no-stream-stdout", "question"]);
        assert!(!args.stream_stdout);
        assert!(args.no_stream_stdout);
    }

    #[test]
    fn debate_cli_defaults_no_timeout() {
        let args = parse_debate_args(&["csa", "debate", "question"]);
        assert_eq!(args.timeout, None);
        assert_eq!(args.idle_timeout, None);
        assert!(!args.stream_stdout);
        assert!(!args.no_stream_stdout);
    }

    #[test]
    fn debate_cli_rejects_zero_timeout() {
        use clap::Parser;
        let result =
            crate::cli::Cli::try_parse_from(["csa", "debate", "--timeout", "0", "question"]);
        assert!(result.is_err(), "timeout=0 should be rejected");
    }

    #[test]
    fn debate_cli_rejects_zero_idle_timeout() {
        use clap::Parser;
        let result =
            crate::cli::Cli::try_parse_from(["csa", "debate", "--idle-timeout", "0", "question"]);
        assert!(result.is_err(), "idle_timeout=0 should be rejected");
    }

    #[test]
    fn debate_timeout_uses_global_default_when_cli_missing() {
        let config = GlobalConfig::default();
        assert_eq!(resolve_debate_timeout_seconds(None, &config), 1800);
    }

    #[test]
    fn debate_timeout_prefers_cli_override() {
        let config = GlobalConfig::default();
        assert_eq!(resolve_debate_timeout_seconds(Some(900), &config), 900);
    }

    // --- CLI parse tests for --rounds flag (#138) ---

    #[test]
    fn debate_cli_parses_rounds_flag() {
        let args = parse_debate_args(&["csa", "debate", "--rounds", "5", "question"]);
        assert_eq!(args.rounds, 5);
    }

    #[test]
    fn debate_cli_rounds_defaults_to_3() {
        let args = parse_debate_args(&["csa", "debate", "question"]);
        assert_eq!(args.rounds, 3);
    }

    #[test]
    fn debate_cli_rejects_zero_rounds() {
        use clap::Parser;
        let result =
            crate::cli::Cli::try_parse_from(["csa", "debate", "--rounds", "0", "question"]);
        assert!(result.is_err(), "rounds=0 should be rejected");
    }

    // --- resolve_debate_stream_mode tests ---

    #[test]
    fn debate_stream_mode_default_non_tty_is_buffer_only() {
        // In test environment (non-TTY stderr), default should be BufferOnly.
        // Note: in interactive TTY, default would be TeeToStderr (symmetric with review, #139)
        let mode = resolve_debate_stream_mode(false, false);
        assert!(matches!(mode, csa_process::StreamMode::BufferOnly));
    }

    #[test]
    fn debate_stream_mode_explicit_stream() {
        let mode = resolve_debate_stream_mode(true, false);
        assert!(matches!(mode, csa_process::StreamMode::TeeToStderr));
    }

    #[test]
    fn debate_stream_mode_explicit_no_stream() {
        let mode = resolve_debate_stream_mode(false, true);
        assert!(matches!(mode, csa_process::StreamMode::BufferOnly));
    }

    // --- verify_debate_skill_available tests (#140) ---

    #[test]
    fn verify_debate_skill_missing_returns_actionable_error() {
        let tmp = tempfile::TempDir::new().unwrap();
        let err = verify_debate_skill_available(tmp.path()).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("Debate pattern not found"),
            "should mention missing pattern: {msg}"
        );
        assert!(
            msg.contains("csa skill install"),
            "should include install guidance: {msg}"
        );
        assert!(
            msg.contains("patterns/debate"),
            "should list searched paths: {msg}"
        );
    }

    #[test]
    fn verify_debate_skill_present_succeeds() {
        let tmp = tempfile::TempDir::new().unwrap();
        // Pattern layout: .csa/patterns/debate/skills/debate/SKILL.md
        let skill_dir = tmp
            .path()
            .join(".csa")
            .join("patterns")
            .join("debate")
            .join("skills")
            .join("debate");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "# Debate Skill\nStructured debate.",
        )
        .unwrap();

        assert!(verify_debate_skill_available(tmp.path()).is_ok());
    }

    #[test]
    fn verify_debate_skill_no_fallback_without_skill() {
        // Ensure no execution path silently downgrades when skill is missing.
        // The verify function must return Err — it must NOT return Ok with a warning.
        let tmp = tempfile::TempDir::new().unwrap();
        let result = verify_debate_skill_available(tmp.path());
        assert!(
            result.is_err(),
            "missing skill must be a hard error, not a warning"
        );
    }
}
