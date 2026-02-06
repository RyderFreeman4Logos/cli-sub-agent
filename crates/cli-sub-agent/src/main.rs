use anyhow::{Context, Result};
use clap::Parser;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use tokio::signal::unix::{signal, SignalKind};
use tracing::{error, info, warn};

mod cli;
mod doctor;

use cli::{Cli, Commands, ConfigCommands, ReviewArgs, SessionCommands};
use csa_config::{init_project, validate_config, ProjectConfig};
use csa_core::types::{OutputFormat, ToolName};
use csa_executor::{create_session_log_writer, Executor, ModelSpec};
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
    let output_format = cli.format.clone();

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
                output_format,
            )
            .await?;
            std::process::exit(exit_code);
        }
        Commands::Session { cmd } => match cmd {
            SessionCommands::List { cd, tool, tree } => {
                handle_session_list(cd, tool, tree)?;
            }
            SessionCommands::Compress { session, cd } => {
                handle_session_compress(session, cd)?;
            }
            SessionCommands::Delete { session, cd } => {
                handle_session_delete(session, cd)?;
            }
            SessionCommands::Logs { session, tail, cd } => {
                handle_session_logs(session, tail, cd)?;
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
        Commands::Doctor => {
            doctor::run_doctor().await?;
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn handle_run(
    tool: Option<ToolName>,
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
    output_format: OutputFormat,
) -> Result<i32> {
    // 1. Determine project root
    let project_root = determine_project_root(cd.as_deref())?;

    // 2. Load config (optional)
    let config = ProjectConfig::load(&project_root)?;

    // 3. Check recursion depth (from config or default)
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

    // 4. Read prompt
    let prompt_text = read_prompt(prompt)?;

    // 5. Resolve tool and model_spec
    let (resolved_tool, resolved_model_spec, resolved_model) = resolve_tool_and_model(
        tool,
        model_spec.as_deref(),
        model.as_deref(),
        config.as_ref(),
    )?;

    // 6. Build executor
    let executor = build_executor(
        &resolved_tool,
        resolved_model_spec.as_deref(),
        resolved_model.as_deref(),
        thinking.as_deref(),
    )?;

    // 7. Check tool is installed
    if let Err(e) = check_tool_installed(executor.executable_name()).await {
        error!(
            "Tool '{}' is not installed.\n\n{}\n\nOr disable it in .csa/config.toml:\n  [tools.{}]\n  enabled = false",
            executor.tool_name(),
            executor.install_hint(),
            executor.tool_name()
        );
        anyhow::bail!("{}", e);
    }

    // 8. Check tool is enabled in config
    if let Some(ref cfg) = config {
        if !cfg.is_tool_enabled(executor.tool_name()) {
            error!(
                "Tool '{}' is disabled in project config",
                executor.tool_name()
            );
            return Ok(1);
        }
    }

    // 9. Execute
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

    // 10. Print result
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
async fn execute_with_session(
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
        })
        .or_insert_with(|| ToolState {
            provider_session_id,
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

fn handle_session_compress(session: String, cd: Option<String>) -> Result<()> {
    let project_root = determine_project_root(cd.as_deref())?;
    let sessions_dir = csa_session::get_session_root(&project_root)?.join("sessions");
    let resolved_id = resolve_session_prefix(&sessions_dir, &session)?;
    let mut session_state = load_session(&project_root, &resolved_id)?;

    // Find the most recently used tool in this session
    let (tool_name, _tool_state) = session_state
        .tools
        .iter()
        .max_by_key(|(_, state)| &state.updated_at)
        .ok_or_else(|| anyhow::anyhow!("Session '{}' has no tool history", resolved_id))?;

    let compress_cmd = match tool_name.as_str() {
        "gemini-cli" => "/compress",
        _ => "/compact",
    };

    println!("Session {} uses tool: {}", resolved_id, tool_name);
    println!("Compress command: {}", compress_cmd);
    println!();
    println!("To compress, resume the session and send the command:");
    println!(
        "  csa run --tool {} --session {} \"{}\"",
        tool_name, resolved_id, compress_cmd
    );

    // Update context status to mark as compacted
    session_state.context_status.is_compacted = true;
    session_state.context_status.last_compacted_at = Some(chrono::Utc::now());
    save_session(&session_state)?;

    Ok(())
}

fn handle_session_delete(session: String, cd: Option<String>) -> Result<()> {
    let project_root = determine_project_root(cd.as_deref())?;
    let sessions_dir = csa_session::get_session_root(&project_root)?.join("sessions");
    let resolved_id = resolve_session_prefix(&sessions_dir, &session)?;
    delete_session(&project_root, &resolved_id)?;
    eprintln!("Deleted session: {}", resolved_id);
    Ok(())
}

fn handle_session_logs(session: String, tail: Option<usize>, cd: Option<String>) -> Result<()> {
    let project_root = determine_project_root(cd.as_deref())?;
    let sessions_dir = csa_session::get_session_root(&project_root)?.join("sessions");
    let resolved_id = resolve_session_prefix(&sessions_dir, &session)?;
    let session_dir = get_session_dir(&project_root, &resolved_id)?;
    let logs_dir = session_dir.join("logs");

    if !logs_dir.exists() {
        eprintln!("No logs found for session {}", resolved_id);
        return Ok(());
    }

    // Find all log files, sorted by name (timestamp order)
    let mut log_files: Vec<_> = fs::read_dir(&logs_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "log"))
        .collect();
    log_files.sort_by_key(|e| e.file_name());

    if log_files.is_empty() {
        eprintln!("No log files found for session {}", resolved_id);
        return Ok(());
    }

    // Display each log file
    for entry in &log_files {
        let path = entry.path();
        let file_name = path.file_name().unwrap_or_default().to_string_lossy();
        eprintln!("=== {} ===", file_name);

        let content = fs::read_to_string(&path)?;

        if let Some(n) = tail {
            // Show last N lines
            let lines: Vec<&str> = content.lines().collect();
            let start = lines.len().saturating_sub(n);
            for line in &lines[start..] {
                println!("{}", line);
            }
        } else {
            print!("{}", content);
        }
        println!();
    }

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
    let project_root = determine_project_root(None)?;
    let sessions = list_sessions(&project_root, None)?;

    let mut stale_locks_removed = 0;
    let mut empty_sessions_removed = 0;
    let mut orphan_dirs_removed = 0;

    for session in &sessions {
        let session_dir = get_session_dir(&project_root, &session.meta_session_id)?;
        let locks_dir = session_dir.join("locks");

        // 1. Check for stale locks
        if locks_dir.exists() {
            if let Ok(entries) = fs::read_dir(&locks_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().is_some_and(|ext| ext == "lock") {
                        // Try to read the lock file JSON
                        if let Ok(content) = fs::read_to_string(&path) {
                            // Simple JSON parsing to extract PID
                            if let Some(pid) = extract_pid_from_lock(&content) {
                                // Check if process is alive (Linux-specific)
                                if !is_process_alive(pid) {
                                    // Process is dead, remove stale lock
                                    if fs::remove_file(&path).is_ok() {
                                        stale_locks_removed += 1;
                                        info!(
                                            "Removed stale lock for dead PID {}: {:?}",
                                            pid,
                                            path.file_name()
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // 2. Check for empty sessions (no tools used)
        if session.tools.is_empty()
            && delete_session(&project_root, &session.meta_session_id).is_ok()
        {
            empty_sessions_removed += 1;
            info!("Removed empty session: {}", session.meta_session_id);
        }
    }

    // BUG-12: Clean orphan directories (no state.toml)
    // list_sessions() only returns loadable sessions, so orphans aren't included
    let session_root = csa_session::get_session_root(&project_root)?;
    let sessions_dir = session_root.join("sessions");

    if sessions_dir.exists() {
        if let Ok(entries) = fs::read_dir(&sessions_dir) {
            for entry in entries.flatten() {
                if entry.file_type().is_ok_and(|ft| ft.is_dir()) {
                    let session_dir = entry.path();
                    let state_path = session_dir.join("state.toml");

                    // If no state.toml exists, it's an orphan
                    if !state_path.exists() && fs::remove_dir_all(&session_dir).is_ok() {
                        orphan_dirs_removed += 1;
                        info!(
                            "Removed orphan directory without state.toml: {}",
                            session_dir.display()
                        );
                    }
                }
            }
        }
    }

    eprintln!("Garbage collection complete:");
    eprintln!("  Stale locks removed: {}", stale_locks_removed);
    eprintln!("  Empty sessions removed: {}", empty_sessions_removed);
    eprintln!("  Orphan directories removed: {}", orphan_dirs_removed);

    Ok(())
}

/// Extract PID from lock file JSON content
fn extract_pid_from_lock(json_content: &str) -> Option<u32> {
    // Simple parsing: look for "pid": followed by a number
    json_content
        .split("\"pid\":")
        .nth(1)?
        .trim()
        .split(',')
        .next()?
        .trim()
        .parse::<u32>()
        .ok()
}

/// Check if a process is alive (Linux-specific)
fn is_process_alive(pid: u32) -> bool {
    // On Linux, check if /proc/{pid} exists
    std::path::Path::new(&format!("/proc/{}", pid)).exists()
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

    // 3. Check recursion depth (from config or default)
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

    // 10. Apply restrictions if configured
    let can_edit = config
        .as_ref()
        .map_or(true, |cfg| cfg.can_tool_edit_existing(executor.tool_name()));
    let effective_prompt = if !can_edit {
        info!(tool = %executor.tool_name(), "Applying edit restriction: tool cannot modify existing files");
        executor.apply_restrictions(&prompt, false)
    } else {
        prompt.clone()
    };

    // 11. Execute with session
    let result = execute_with_session(
        &executor,
        &tool,
        &effective_prompt,
        args.session,
        Some("Code review session".to_string()),
        None,
        &project_root,
        config.as_ref(),
    )
    .await?;

    // 12. Print result
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

        // Check for specific error patterns and provide friendly messages
        if stderr.contains("unknown revision") || stderr.contains("ambiguous argument") {
            if let Some(ref commit) = args.commit {
                anyhow::bail!(
                    "Commit '{}' not found. Ensure the commit SHA exists.",
                    commit
                );
            } else if !args.diff {
                anyhow::bail!(
                    "Branch '{}' not found. Ensure the branch exists locally.",
                    args.branch
                );
            }
        }

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
