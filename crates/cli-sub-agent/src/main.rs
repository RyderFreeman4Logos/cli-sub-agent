use anyhow::{Context, Result};
use clap::Parser;
use std::io::Read;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use tokio::signal::unix::{signal, SignalKind};
use tracing::{error, info, warn};

mod batch;
mod cli;
mod config_cmds;
mod doctor;
mod gc;
mod mcp_server;
mod review_cmd;
mod session_cmds;
mod setup_cmds;
mod skill_cmds;

use cli::{Cli, Commands, ConfigCommands, SessionCommands, SetupCommands, SkillCommands};
use csa_config::{init_project, ProjectConfig};
use csa_core::types::{OutputFormat, ToolName};
use csa_executor::{create_session_log_writer, Executor, ModelSpec};
use csa_lock::acquire_lock;
use csa_process::check_tool_installed;
use csa_resource::{MemoryMonitor, ResourceGuard, ResourceLimits};
use csa_session::{
    create_session, get_session_dir, load_session, resolve_session_prefix, save_session,
    TokenUsage, ToolState,
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
    let output_format = cli.format.clone();

    match cli.command {
        Commands::Run {
            tool,
            prompt,
            session,
            last,
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
                last,
                description,
                parent,
                ephemeral,
                cd,
                model_spec,
                model,
                thinking,
                current_depth,
                output_format,
            )
            .await?;
            std::process::exit(exit_code);
        }
        Commands::Session { cmd } => match cmd {
            SessionCommands::List { cd, tool, tree } => {
                session_cmds::handle_session_list(cd, tool, tree)?;
            }
            SessionCommands::Compress { session, cd } => {
                session_cmds::handle_session_compress(session, cd)?;
            }
            SessionCommands::Delete { session, cd } => {
                session_cmds::handle_session_delete(session, cd)?;
            }
            SessionCommands::Clean {
                days,
                dry_run,
                tool,
                cd,
            } => {
                session_cmds::handle_session_clean(days, dry_run, tool, cd)?;
            }
            SessionCommands::Logs { session, tail, cd } => {
                session_cmds::handle_session_logs(session, tail, cd)?;
            }
        },
        Commands::Init { non_interactive } => {
            handle_init(non_interactive)?;
        }
        Commands::Gc {
            dry_run,
            max_age_days,
        } => {
            gc::handle_gc(dry_run, max_age_days)?;
        }
        Commands::Config { cmd } => match cmd {
            ConfigCommands::Show { cd } => {
                config_cmds::handle_config_show(cd)?;
            }
            ConfigCommands::Edit { cd } => {
                config_cmds::handle_config_edit(cd)?;
            }
            ConfigCommands::Validate { cd } => {
                config_cmds::handle_config_validate(cd)?;
            }
        },
        Commands::Review(args) => {
            let exit_code = review_cmd::handle_review(args, current_depth).await?;
            std::process::exit(exit_code);
        }
        Commands::Doctor => {
            doctor::run_doctor().await?;
        }
        Commands::Batch { file, cd, dry_run } => {
            batch::handle_batch(file, cd, dry_run, current_depth).await?;
        }
        Commands::McpServer => {
            mcp_server::run_mcp_server().await?;
        }
        Commands::Skill { cmd } => match cmd {
            SkillCommands::Install { source, target } => {
                skill_cmds::handle_skill_install(source, target)?;
            }
            SkillCommands::List => {
                skill_cmds::handle_skill_list()?;
            }
        },
        Commands::Setup { cmd } => match cmd {
            SetupCommands::ClaudeCode => {
                setup_cmds::handle_setup_claude_code()?;
            }
            SetupCommands::Codex => {
                setup_cmds::handle_setup_codex()?;
            }
            SetupCommands::OpenCode => {
                setup_cmds::handle_setup_opencode()?;
            }
        },
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn handle_run(
    tool: Option<ToolName>,
    prompt: Option<String>,
    session_arg: Option<String>,
    last: bool,
    description: Option<String>,
    parent: Option<String>,
    ephemeral: bool,
    cd: Option<String>,
    model_spec: Option<String>,
    model: Option<String>,
    thinking: Option<String>,
    current_depth: u32,
    output_format: OutputFormat,
) -> Result<i32> {
    // 1. Determine project root
    let project_root = determine_project_root(cd.as_deref())?;

    // 2. Resolve --last flag to session ID
    let session_arg = if last {
        let sessions = csa_session::list_sessions(&project_root, None)?;
        if sessions.is_empty() {
            anyhow::bail!("No sessions found. Run a task first to create one.");
        }
        // Sessions should be sorted by last_accessed (most recent first)
        let mut sorted_sessions = sessions;
        sorted_sessions.sort_by(|a, b| b.last_accessed.cmp(&a.last_accessed));
        Some(sorted_sessions[0].meta_session_id.clone())
    } else {
        session_arg
    };

    // 3. Load config (optional)
    let config = ProjectConfig::load(&project_root)?;

    // 4. Check recursion depth (from config or default)
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

    // 5. Read prompt
    let prompt_text = read_prompt(prompt)?;

    // 6. Resolve tool and model_spec
    let (resolved_tool, resolved_model_spec, resolved_model) = resolve_tool_and_model(
        tool,
        model_spec.as_deref(),
        model.as_deref(),
        config.as_ref(),
    )?;

    // 7. Build executor
    let executor = build_executor(
        &resolved_tool,
        resolved_model_spec.as_deref(),
        resolved_model.as_deref(),
        thinking.as_deref(),
    )?;

    // 8. Check tool is installed
    if let Err(e) = check_tool_installed(executor.executable_name()).await {
        error!(
            "Tool '{}' is not installed.\n\n{}\n\nOr disable it in .csa/config.toml:\n  [tools.{}]\n  enabled = false",
            executor.tool_name(),
            executor.install_hint(),
            executor.tool_name()
        );
        anyhow::bail!("{}", e);
    }

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

    // 10. Execute
    let result = if ephemeral {
        // Ephemeral: use temp directory
        let temp_dir = TempDir::new()?;
        info!("Ephemeral session in: {:?}", temp_dir.path());
        executor.execute_in(&prompt_text, temp_dir.path()).await?
    } else {
        // Persistent session
        match execute_with_session(
            &executor,
            &resolved_tool,
            &prompt_text,
            session_arg.clone(),
            description,
            parent,
            &project_root,
            config.as_ref(),
        )
        .await
        {
            Ok(result) => result,
            Err(e) => {
                // BUG-13: Check if this is a lock error and format as JSON if needed
                let error_msg = e.to_string();
                if error_msg.contains("Session locked by PID")
                    && matches!(output_format, OutputFormat::Json)
                {
                    let json_error = serde_json::json!({
                        "error": "session_locked",
                        "session_id": session_arg.unwrap_or_else(|| "(new)".to_string()),
                        "tool": resolved_tool.as_str(),
                        "message": error_msg
                    });
                    println!("{}", serde_json::to_string_pretty(&json_error)?);
                    return Ok(1);
                }
                // Not a lock error or text format - propagate normally
                return Err(e);
            }
        }
    };

    // 11. Print result
    match output_format {
        OutputFormat::Text => {
            print!("{}", result.output);
        }
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(&result)?;
            println!("{}", json);
        }
    }

    Ok(result.exit_code)
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn execute_with_session(
    executor: &Executor,
    tool: &ToolName,
    prompt: &str,
    session_arg: Option<String>,
    description: Option<String>,
    parent: Option<String>,
    project_root: &Path,
    config: Option<&ProjectConfig>,
) -> Result<csa_process::ExecutionResult> {
    // Check for parent session violation: a child process must not operate on its own session
    if let Some(ref session_id) = session_arg {
        if let Ok(env_session) = std::env::var("CSA_SESSION_ID") {
            if env_session == *session_id {
                return Err(csa_core::error::AppError::ParentSessionViolation.into());
            }
        }
    }

    // Resolve or create session
    let mut session = if let Some(ref session_id) = session_arg {
        let sessions_dir = csa_session::get_session_root(project_root)?.join("sessions");
        let resolved_id = resolve_session_prefix(&sessions_dir, session_id)?;
        load_session(project_root, &resolved_id)?
    } else {
        let parent_id = parent.or_else(|| std::env::var("CSA_SESSION_ID").ok());
        create_session(project_root, description.as_deref(), parent_id.as_deref())?
    };

    let session_dir = get_session_dir(project_root, &session.meta_session_id)?;

    // Create session log writer
    let (_log_writer, _log_guard) =
        create_session_log_writer(&session_dir).context("Failed to create session log writer")?;

    // Acquire lock with truncated prompt as reason
    let lock_reason = truncate_prompt(prompt, 80);
    let _lock =
        acquire_lock(&session_dir, executor.tool_name(), &lock_reason).with_context(|| {
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
        // Stats stored at project state level, not per-session
        let project_state_dir = csa_session::get_session_root(project_root)?;
        let stats_path = project_state_dir.join("usage_stats.toml");
        Some(ResourceGuard::new(limits, &stats_path))
    } else {
        None
    };

    // Check resource availability
    if let Some(ref mut guard) = resource_guard {
        guard.check_availability(executor.tool_name())?;
    }

    info!("Executing in session: {}", session.meta_session_id);

    // Apply restrictions if configured
    let can_edit = config.map_or(true, |cfg| cfg.can_tool_edit_existing(executor.tool_name()));
    let effective_prompt = if !can_edit {
        info!(tool = %executor.tool_name(), "Applying edit restriction: tool cannot modify existing files");
        executor.apply_restrictions(prompt, false)
    } else {
        prompt.to_string()
    };

    // Build command
    let tool_state = session.tools.get(executor.tool_name()).cloned();
    let cmd = executor.build_command(&effective_prompt, tool_state.as_ref(), &session);

    // Spawn child process
    let child = csa_process::spawn_tool(cmd)
        .await
        .context("Failed to spawn tool process")?;

    // Get child PID and start memory monitor
    let child_pid = child.id().context("Failed to get child process PID")?;
    let monitor = MemoryMonitor::start(child_pid);

    // Set up signal handlers for SIGTERM and SIGINT
    let mut sigterm =
        signal(SignalKind::terminate()).context("Failed to install SIGTERM handler")?;
    let mut sigint = signal(SignalKind::interrupt()).context("Failed to install SIGINT handler")?;

    // Wait for either child completion or signal
    let wait_future = csa_process::wait_and_capture(child);
    tokio::pin!(wait_future);

    let result = tokio::select! {
        result = &mut wait_future => {
            result.context("Failed to wait for tool process")?
        }
        _ = sigterm.recv() => {
            info!("Received SIGTERM, forwarding to child process group");
            // Forward SIGTERM to the child's process group (negative PID)
            // SAFETY: kill() is async-signal-safe. We use the negative of child_pid
            // to target the entire process group created by setsid().
            #[cfg(unix)]
            unsafe {
                libc::kill(-(child_pid as i32), libc::SIGTERM);
            }
            // Wait for child to exit after signal
            wait_future.await.context("Failed to wait for tool process after SIGTERM")?
        }
        _ = sigint.recv() => {
            info!("Received SIGINT, forwarding to child process group");
            // Forward SIGINT to the child's process group
            // SAFETY: Same as SIGTERM handler above
            #[cfg(unix)]
            unsafe {
                libc::kill(-(child_pid as i32), libc::SIGINT);
            }
            // Wait for child to exit after signal
            wait_future.await.context("Failed to wait for tool process after SIGINT")?
        }
    };

    // Stop memory monitor and record usage
    let peak_memory_mb = monitor.stop().await;
    if let Some(ref mut guard) = resource_guard {
        guard.record_usage(executor.tool_name(), peak_memory_mb);
    }

    // Extract provider session ID from output
    let provider_session_id = csa_executor::extract_session_id(tool, &result.output);

    // Parse token usage from output (best-effort)
    let token_usage = parse_token_usage(&result.output);

    // Update session state
    session
        .tools
        .entry(executor.tool_name().to_string())
        .and_modify(|t| {
            // Only update provider_session_id if extraction succeeded
            if let Some(ref session_id) = provider_session_id {
                t.provider_session_id = Some(session_id.clone());
            }
            t.last_action_summary = result.summary.clone();
            t.last_exit_code = result.exit_code;
            t.updated_at = chrono::Utc::now();

            // Update token usage if parsed successfully
            if let Some(ref usage) = token_usage {
                t.token_usage = Some(usage.clone());
            }
        })
        .or_insert_with(|| ToolState {
            provider_session_id,
            last_action_summary: result.summary.clone(),
            last_exit_code: result.exit_code,
            updated_at: chrono::Utc::now(),
            token_usage: token_usage.clone(),
        });
    session.last_accessed = chrono::Utc::now();

    // Update cumulative token usage if we got new tokens
    if let Some(new_usage) = token_usage {
        let cumulative = session
            .total_token_usage
            .get_or_insert(TokenUsage::default());
        cumulative.input_tokens =
            Some(cumulative.input_tokens.unwrap_or(0) + new_usage.input_tokens.unwrap_or(0));
        cumulative.output_tokens =
            Some(cumulative.output_tokens.unwrap_or(0) + new_usage.output_tokens.unwrap_or(0));
        cumulative.total_tokens =
            Some(cumulative.total_tokens.unwrap_or(0) + new_usage.total_tokens.unwrap_or(0));
        cumulative.estimated_cost_usd = Some(
            cumulative.estimated_cost_usd.unwrap_or(0.0)
                + new_usage.estimated_cost_usd.unwrap_or(0.0),
        );
    }

    // Save session
    save_session(&session)?;

    Ok(result)
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

pub(crate) fn determine_project_root(cd: Option<&str>) -> Result<PathBuf> {
    let path = if let Some(cd_path) = cd {
        PathBuf::from(cd_path)
    } else {
        std::env::current_dir()?
    };

    Ok(path.canonicalize()?)
}

pub(crate) fn read_prompt(prompt: Option<String>) -> Result<String> {
    if let Some(p) = prompt {
        if p.trim().is_empty() {
            anyhow::bail!(
                "Empty prompt provided. Usage:\n  csa run --tool <tool> \"your prompt here\"\n  echo \"prompt\" | csa run --tool <tool>"
            );
        }
        Ok(p)
    } else {
        // No prompt argument: read from stdin
        use std::io::IsTerminal;
        if std::io::stdin().is_terminal() {
            anyhow::bail!(
                "No prompt provided and stdin is a terminal.\n\n\
                 Usage:\n  \
                 csa run --tool <tool> \"your prompt here\"\n  \
                 echo \"prompt\" | csa run --tool <tool>"
            );
        }
        let mut buffer = String::new();
        std::io::stdin().read_to_string(&mut buffer)?;
        if buffer.trim().is_empty() {
            anyhow::bail!("Empty prompt from stdin. Provide a non-empty prompt.");
        }
        Ok(buffer)
    }
}

/// Resolve tool and model from CLI args and config.
///
/// Returns (tool, model_spec, model) where:
/// - tool: the selected tool (from CLI or tier-based selection)
/// - model_spec: optional model spec string (from CLI or tier)
/// - model: optional model string (from CLI, with alias resolution applied)
fn resolve_tool_and_model(
    tool: Option<ToolName>,
    model_spec: Option<&str>,
    model: Option<&str>,
    config: Option<&ProjectConfig>,
) -> Result<(ToolName, Option<String>, Option<String>)> {
    // Case 1: model_spec provided → parse it to get tool
    if let Some(spec) = model_spec {
        let parsed = ModelSpec::parse(spec)?;
        let tool_name = match parsed.tool.as_str() {
            "gemini-cli" => ToolName::GeminiCli,
            "opencode" => ToolName::Opencode,
            "codex" => ToolName::Codex,
            "claude-code" => ToolName::ClaudeCode,
            _ => anyhow::bail!("Unknown tool in model spec: {}", parsed.tool),
        };
        return Ok((tool_name, Some(spec.to_string()), None));
    }

    // Case 2: tool provided → use it with optional model (apply alias resolution)
    if let Some(tool_name) = tool {
        let resolved_model = model.map(|m| {
            config
                .map(|cfg| cfg.resolve_alias(m))
                .unwrap_or_else(|| m.to_string())
        });
        return Ok((tool_name, None, resolved_model));
    }

    // Case 3: neither tool nor model_spec → use tier-based auto-selection
    if let Some(cfg) = config {
        if let Some((tool_name_str, tier_model_spec)) = cfg.resolve_tier_tool("default") {
            let tool_name = match tool_name_str.as_str() {
                "gemini-cli" => ToolName::GeminiCli,
                "opencode" => ToolName::Opencode,
                "codex" => ToolName::Codex,
                "claude-code" => ToolName::ClaudeCode,
                _ => anyhow::bail!("Unknown tool from tier: {}", tool_name_str),
            };
            // Use tier's model_spec
            return Ok((tool_name, Some(tier_model_spec), None));
        }
    }

    // Case 4: no config or no tier mapping → error
    anyhow::bail!(
        "No tool specified and no tier-based selection available. \
         Use --tool or run 'csa init' to configure tiers."
    )
}

pub(crate) fn build_executor(
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

/// Truncate a string to max_len characters, adding "..." if truncated
fn truncate_prompt(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        // Find a good break point (preferably a space)
        let truncate_at = max_len.saturating_sub(3);
        let substring = &s[..truncate_at.min(s.len())];

        // Try to break at last space if possible
        if let Some(last_space) = substring.rfind(' ') {
            if last_space > truncate_at / 2 {
                return format!("{}...", &substring[..last_space]);
            }
        }

        format!("{}...", substring)
    }
}

/// Parse token usage from tool output (best-effort, returns None on failure)
///
/// Looks for common patterns in stdout/stderr:
/// - "tokens: N" or "Tokens: N" or "total_tokens: N"
/// - "input_tokens: N" / "output_tokens: N"
/// - "cost: $N.NN" or "estimated_cost: $N.NN"
fn parse_token_usage(output: &str) -> Option<TokenUsage> {
    let mut usage = TokenUsage::default();
    let mut found_any = false;

    // Simple pattern matching without regex
    for line in output.lines() {
        let line_lower = line.to_lowercase();

        // Parse input_tokens
        if let Some(pos) = line_lower.find("input_tokens") {
            if let Some(val) = extract_number(&line[pos..]) {
                usage.input_tokens = Some(val);
                found_any = true;
            }
        }

        // Parse output_tokens
        if let Some(pos) = line_lower.find("output_tokens") {
            if let Some(val) = extract_number(&line[pos..]) {
                usage.output_tokens = Some(val);
                found_any = true;
            }
        }

        // Parse total_tokens
        if let Some(pos) = line_lower.find("total_tokens") {
            if let Some(val) = extract_number(&line[pos..]) {
                usage.total_tokens = Some(val);
                found_any = true;
            }
        } else if let Some(pos) = line_lower.find("tokens:") {
            if let Some(val) = extract_number(&line[pos..]) {
                usage.total_tokens = Some(val);
                found_any = true;
            }
        }

        // Parse cost (look for "$N.NN" pattern)
        if line_lower.contains("cost") {
            if let Some(val) = extract_cost(line) {
                usage.estimated_cost_usd = Some(val);
                found_any = true;
            }
        }
    }

    // Calculate total_tokens if not found but input/output are available
    if usage.total_tokens.is_none() {
        if let (Some(input), Some(output)) = (usage.input_tokens, usage.output_tokens) {
            usage.total_tokens = Some(input + output);
            found_any = true;
        }
    }

    if found_any {
        Some(usage)
    } else {
        None
    }
}

/// Extract a number after colon or equals sign
fn extract_number(text: &str) -> Option<u64> {
    // Find colon or equals
    let start = text.find(':')?;
    let after_colon = &text[start + 1..];

    // Take first word after colon
    let num_str: String = after_colon
        .chars()
        .skip_while(|c| c.is_whitespace())
        .take_while(|c| c.is_ascii_digit())
        .collect();

    num_str.parse().ok()
}

/// Extract cost value after $ sign
fn extract_cost(text: &str) -> Option<f64> {
    let start = text.find('$')?;
    let after_dollar = &text[start + 1..];

    // Take digits and decimal point
    let num_str: String = after_dollar
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();

    num_str.parse().ok()
}
