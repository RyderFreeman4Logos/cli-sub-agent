use anyhow::{Context, Result};
use clap::Parser;
use std::io::Read;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use tracing::{error, info, warn};

mod cli;

use cli::{Cli, Commands, ConfigCommands, ReviewArgs, SessionCommands};
use csa_config::{init_project, validate_config, ProjectConfig};
use csa_core::types::ToolName;
use csa_executor::{Executor, ModelSpec};
use csa_lock::acquire_lock;
use csa_process::check_tool_installed;
use csa_resource::{MemoryMonitor, ResourceGuard, ResourceLimits};
use csa_session::{
    create_session, delete_session, get_session_dir, list_sessions, list_sessions_tree,
    load_session, resolve_session_prefix, save_session, ToolState,
};

#[tokio::main]
async fn main() -> Result<()> {
    // Read current depth from env
    let current_depth: u32 = std::env::var("CSA_DEPTH")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    // Initialize tracing (output to stderr, initialize only once)
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init()
        .ok();

    let cli = Cli::parse();

    match cli.command {
        Commands::Run {
            tool,
            prompt,
            session,
            description,
            parent,
            ephemeral,
            cd,
            model_spec,
            model,
            thinking,
        } => {
            let exit_code = handle_run(
                tool,
                prompt,
                session,
                description,
                parent,
                ephemeral,
                cd,
                model_spec,
                model,
                thinking,
                current_depth,
            )
            .await?;
            std::process::exit(exit_code);
        }
        Commands::Session { cmd } => match cmd {
            SessionCommands::List { cd, tool, tree } => {
                handle_session_list(cd, tool, tree)?;
            }
            SessionCommands::Compress { session } => {
                handle_session_compress(session)?;
            }
            SessionCommands::Delete { session } => {
                handle_session_delete(session)?;
            }
        },
        Commands::Init { non_interactive } => {
            handle_init(non_interactive)?;
        }
        Commands::Gc => {
            handle_gc()?;
        }
        Commands::Config { cmd } => match cmd {
            ConfigCommands::Show => {
                handle_config_show()?;
            }
            ConfigCommands::Edit => {
                handle_config_edit()?;
            }
            ConfigCommands::Validate => {
                handle_config_validate()?;
            }
        },
        Commands::Review(args) => {
            let exit_code = handle_review(args, current_depth).await?;
            std::process::exit(exit_code);
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn handle_run(
    tool: ToolName,
    prompt: Option<String>,
    session_arg: Option<String>,
    description: Option<String>,
    parent: Option<String>,
    ephemeral: bool,
    cd: Option<String>,
    model_spec: Option<String>,
    model: Option<String>,
    thinking: Option<String>,
    current_depth: u32,
) -> Result<i32> {
    // 1. Determine project root
    let project_root = determine_project_root(cd.as_deref())?;

    // 2. Load config (optional)
    let config = ProjectConfig::load(&project_root)?;

    // 3. Check recursion depth (hardcoded for now, config doesn't have this field)
    let max_depth = 5u32;

    if current_depth > max_depth {
        error!(
            "Max recursion depth ({}) exceeded. Current: {}. Do it yourself.",
            max_depth, current_depth
        );
        return Ok(1);
    }

    // 4. Read prompt
    let prompt_text = read_prompt(prompt)?;

    // 5. Build executor
    let executor = build_executor(
        &tool,
        model_spec.as_deref(),
        model.as_deref(),
        thinking.as_deref(),
    )?;

    // 6. Check tool is installed
    check_tool_installed(executor.executable_name())
        .await
        .with_context(|| format!("Tool '{}' is not installed", executor.executable_name()))?;

    // 7. Check tool is enabled in config
    if let Some(ref cfg) = config {
        if !cfg.is_tool_enabled(executor.tool_name()) {
            error!(
                "Tool '{}' is disabled in project config",
                executor.tool_name()
            );
            return Ok(1);
        }
    }

    // 8. Execute
    let result = if ephemeral {
        // Ephemeral: use temp directory
        let temp_dir = TempDir::new()?;
        info!("Ephemeral session in: {:?}", temp_dir.path());
        executor.execute_in(&prompt_text, temp_dir.path()).await?
    } else {
        // Persistent session
        execute_with_session(
            &executor,
            &prompt_text,
            session_arg,
            description,
            parent,
            &project_root,
            config.as_ref(),
        )
        .await?
    };

    // 9. Print result
    print!("{}", result.output);

    Ok(result.exit_code)
}

async fn execute_with_session(
    executor: &Executor,
    prompt: &str,
    session_arg: Option<String>,
    description: Option<String>,
    parent: Option<String>,
    project_root: &Path,
    config: Option<&ProjectConfig>,
) -> Result<csa_process::ExecutionResult> {
    // Resolve or create session
    let mut session = if let Some(ref session_id) = session_arg {
        let sessions_dir = csa_session::get_session_root(project_root)?;
        let resolved_id = resolve_session_prefix(&sessions_dir, session_id)?;
        load_session(project_root, &resolved_id)?
    } else {
        let parent_id = parent.or_else(|| std::env::var("CSA_SESSION_ID").ok());
        create_session(project_root, description.as_deref(), parent_id.as_deref())?
    };

    let session_dir = get_session_dir(project_root, &session.meta_session_id)?;

    // Acquire lock
    let _lock = acquire_lock(&session_dir, executor.tool_name()).with_context(|| {
        format!(
            "Failed to acquire lock for session {}",
            session.meta_session_id
        )
    })?;

    // Resource guard
    let mut resource_guard = if let Some(cfg) = config {
        let limits = ResourceLimits {
            min_free_memory_mb: cfg.resources.min_free_memory_mb,
            min_free_swap_mb: cfg.resources.min_free_swap_mb,
            initial_estimates: cfg.resources.initial_estimates.clone(),
        };
        let stats_path = session_dir.join("resource_stats.json");
        Some(ResourceGuard::new(limits, &stats_path))
    } else {
        None
    };

    // Check resource availability
    if let Some(ref mut guard) = resource_guard {
        guard.check_availability(executor.tool_name())?;
    }

    info!("Executing in session: {}", session.meta_session_id);

    // Start memory monitor
    let monitor = MemoryMonitor::start(std::process::id());

    // Execute
    let tool_state = session.tools.get(executor.tool_name()).cloned();

    let result = executor
        .execute(prompt, tool_state.as_ref(), &session)
        .await?;

    // Stop memory monitor and record usage
    let peak_memory_mb = monitor.stop().await;
    if let Some(ref mut guard) = resource_guard {
        guard.record_usage(executor.tool_name(), peak_memory_mb);
    }

    // Update session state
    session
        .tools
        .entry(executor.tool_name().to_string())
        .and_modify(|t| {
            t.last_action_summary = result.summary.clone();
            t.last_exit_code = result.exit_code;
            t.updated_at = chrono::Utc::now();
        })
        .or_insert_with(|| ToolState {
            provider_session_id: None,
            last_action_summary: result.summary.clone(),
            last_exit_code: result.exit_code,
            updated_at: chrono::Utc::now(),
        });
    session.last_accessed = chrono::Utc::now();

    // Save session
    save_session(&session)?;

    Ok(result)
}

fn handle_session_list(cd: Option<String>, tool: Option<String>, tree: bool) -> Result<()> {
    let project_root = determine_project_root(cd.as_deref())?;
    let tool_filter: Option<Vec<&str>> = tool.as_ref().map(|t| t.split(',').collect());

    if tree {
        let tree_output = list_sessions_tree(&project_root, tool_filter.as_deref())?;
        print!("{}", tree_output);
    } else {
        let sessions = list_sessions(&project_root, tool_filter.as_deref())?;
        for session in sessions {
            println!(
                "{} | {} | {} | {:?}",
                session.meta_session_id,
                session.created_at.format("%Y-%m-%d %H:%M:%S"),
                session.description.as_deref().unwrap_or("(no description)"),
                session.tools.keys().collect::<Vec<_>>()
            );
        }
    }

    Ok(())
}

fn handle_session_compress(_session: String) -> Result<()> {
    eprintln!("Session compression not yet implemented");
    eprintln!("(requires stdin injection into running tool)");
    Ok(())
}

fn handle_session_delete(session: String) -> Result<()> {
    let project_root = determine_project_root(None)?;
    let sessions_dir = csa_session::get_session_root(&project_root)?;
    let resolved_id = resolve_session_prefix(&sessions_dir, &session)?;
    delete_session(&project_root, &resolved_id)?;
    eprintln!("Deleted session: {}", resolved_id);
    Ok(())
}

fn handle_init(non_interactive: bool) -> Result<()> {
    let project_root = determine_project_root(None)?;
    let config = init_project(&project_root, non_interactive)?;
    eprintln!(
        "Initialized project configuration at: {}",
        ProjectConfig::config_path(&project_root).display()
    );
    eprintln!("Project: {}", config.project.name);
    Ok(())
}

fn handle_gc() -> Result<()> {
    eprintln!("Garbage collection not yet implemented");
    Ok(())
}

fn handle_config_show() -> Result<()> {
    let project_root = determine_project_root(None)?;
    let config = ProjectConfig::load(&project_root)?
        .ok_or_else(|| anyhow::anyhow!("No configuration found. Run 'csa init' first."))?;

    let toml_str = toml::to_string_pretty(&config)?;
    print!("{}", toml_str);
    Ok(())
}

fn handle_config_edit() -> Result<()> {
    let project_root = determine_project_root(None)?;
    let config_path = ProjectConfig::config_path(&project_root);

    if !config_path.exists() {
        error!("Configuration file does not exist. Run 'csa init' first.");
        return Ok(());
    }

    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
    let status = std::process::Command::new(editor)
        .arg(&config_path)
        .status()?;

    if !status.success() {
        warn!("Editor exited with non-zero status");
    }

    Ok(())
}

fn handle_config_validate() -> Result<()> {
    let project_root = determine_project_root(None)?;
    validate_config(&project_root)?;
    eprintln!("Configuration is valid");
    Ok(())
}

fn determine_project_root(cd: Option<&str>) -> Result<PathBuf> {
    let path = if let Some(cd_path) = cd {
        PathBuf::from(cd_path)
    } else {
        std::env::current_dir()?
    };

    Ok(path.canonicalize()?)
}

fn read_prompt(prompt: Option<String>) -> Result<String> {
    if let Some(p) = prompt {
        Ok(p)
    } else {
        let mut buffer = String::new();
        std::io::stdin().read_to_string(&mut buffer)?;
        Ok(buffer)
    }
}

async fn handle_review(args: ReviewArgs, current_depth: u32) -> Result<i32> {
    // 1. Determine project root
    let project_root = determine_project_root(args.cd.as_deref())?;

    // 2. Load config (optional)
    let config = ProjectConfig::load(&project_root)?;

    // 3. Check recursion depth
    let max_depth = 5u32;
    if current_depth > max_depth {
        error!(
            "Max recursion depth ({}) exceeded. Current: {}. Do it yourself.",
            max_depth, current_depth
        );
        return Ok(1);
    }

    // 4. Get git diff based on scope
    let diff_output = get_review_diff(&args)?;

    if diff_output.trim().is_empty() {
        eprintln!("No changes to review");
        return Ok(0);
    }

    // 5. Construct review prompt
    let prompt = construct_review_prompt(&args, &diff_output);

    // 6. Determine tool
    let tool = if let Some(t) = args.tool {
        t
    } else if let Some(ref cfg) = config {
        // Use first enabled tool from config
        cfg.tools
            .iter()
            .find(|(_, tool_cfg)| tool_cfg.enabled)
            .and_then(|(name, _)| match name.as_str() {
                "gemini-cli" => Some(ToolName::GeminiCli),
                "opencode" => Some(ToolName::Opencode),
                "codex" => Some(ToolName::Codex),
                "claude-code" => Some(ToolName::ClaudeCode),
                _ => None,
            })
            .ok_or_else(|| anyhow::anyhow!("No enabled tools in project config"))?
    } else {
        // Default to gemini-cli
        ToolName::GeminiCli
    };

    // 7. Build executor
    let executor = build_executor(&tool, None, args.model.as_deref(), None)?;

    // 8. Check tool is installed
    check_tool_installed(executor.executable_name())
        .await
        .with_context(|| format!("Tool '{}' is not installed", executor.executable_name()))?;

    // 9. Check tool is enabled in config
    if let Some(ref cfg) = config {
        if !cfg.is_tool_enabled(executor.tool_name()) {
            error!(
                "Tool '{}' is disabled in project config",
                executor.tool_name()
            );
            return Ok(1);
        }
    }

    // 10. Execute with session
    let result = execute_with_session(
        &executor,
        &prompt,
        args.session,
        Some("Code review session".to_string()),
        None,
        &project_root,
        config.as_ref(),
    )
    .await?;

    // 11. Print result
    print!("{}", result.output);

    Ok(result.exit_code)
}

fn get_review_diff(args: &ReviewArgs) -> Result<String> {
    let output = if let Some(ref commit) = args.commit {
        // Review specific commit
        std::process::Command::new("git")
            .arg("show")
            .arg(commit)
            .output()
            .with_context(|| format!("Failed to run git show for commit: {}", commit))?
    } else if args.diff {
        // Review uncommitted changes
        std::process::Command::new("git")
            .arg("diff")
            .arg("HEAD")
            .output()
            .context("Failed to run git diff")?
    } else {
        // Compare against branch (default: main)
        let branch = &args.branch;
        std::process::Command::new("git")
            .arg("diff")
            .arg(format!("{}...HEAD", branch))
            .output()
            .with_context(|| format!("Failed to run git diff against branch: {}", branch))?
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Git command failed: {}", stderr);
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn construct_review_prompt(args: &ReviewArgs, diff: &str) -> String {
    let default_instruction = "Review the following code changes for bugs, security issues, and code quality. Provide specific, actionable feedback.";

    let instruction = if let Some(ref custom_prompt) = args.prompt {
        format!("{}\n\n{}", default_instruction, custom_prompt)
    } else {
        default_instruction.to_string()
    };

    format!("{}\n\n```diff\n{}\n```", instruction, diff)
}

fn build_executor(
    tool: &ToolName,
    model_spec: Option<&str>,
    model: Option<&str>,
    thinking: Option<&str>,
) -> Result<Executor> {
    if let Some(spec) = model_spec {
        let parsed = ModelSpec::parse(spec)?;
        // ModelSpec.tool is String, need to parse to ToolName
        let tool_name = match parsed.tool.as_str() {
            "gemini-cli" => ToolName::GeminiCli,
            "opencode" => ToolName::Opencode,
            "codex" => ToolName::Codex,
            "claude-code" => ToolName::ClaudeCode,
            _ => anyhow::bail!("Unknown tool in model spec: {}", parsed.tool),
        };
        Ok(Executor::from_tool_name(&tool_name, Some(parsed.model)))
    } else {
        let mut final_model = model.map(|s| s.to_string());

        // Apply thinking budget if specified (tool-specific logic)
        if let Some(thinking_level) = thinking {
            if final_model.is_none() {
                // Generate model string with thinking budget
                final_model = Some(format!("thinking:{}", thinking_level));
            } else {
                warn!("Both --model and --thinking specified; --thinking ignored");
            }
        }

        Ok(Executor::from_tool_name(tool, final_model))
    }
}
