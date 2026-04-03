use anyhow::{Result, anyhow};
use std::fs;
use std::path::Path;
use tracing::{info, warn};

use csa_core::types::OutputFormat;
use csa_session::{
    MetaSessionState, SessionPhase, SessionResult, delete_session, get_session_dir, list_sessions,
    list_sessions_tree_filtered, load_result, load_session, save_session_in,
};

// Re-export types and functions from session_cmds_result so that
// callers can continue using `session_cmds::*`.
pub(crate) use crate::session_cmds_result::{
    StructuredOutputOpts, handle_session_artifacts, handle_session_measure, handle_session_result,
    handle_session_tool_output,
};

#[path = "session_cmds_resolve.rs"]
mod resolve;
use resolve::list_checkpoints_from_dirs;
pub(crate) use resolve::{
    SessionPrefixResolution, legacy_sessions_dir_from_primary_root,
    resolve_session_prefix_with_fallback, resolve_session_prefix_with_global_fallback,
};

fn truncate_with_ellipsis(input: &str, max_chars: usize) -> String {
    if input.chars().count() <= max_chars {
        return input.to_string();
    }

    if max_chars <= 3 {
        return ".".repeat(max_chars);
    }

    let visible_chars = max_chars - 3;
    let end = input
        .char_indices()
        .map(|(idx, _)| idx)
        .nth(visible_chars)
        .unwrap_or(input.len());

    format!("{}...", &input[..end])
}

fn phase_label(phase: &SessionPhase) -> &'static str {
    match phase {
        SessionPhase::Active => "Active",
        SessionPhase::Available => "Available",
        SessionPhase::Retired => "Retired",
    }
}

fn status_from_phase_and_result(
    phase: &SessionPhase,
    result: Option<&SessionResult>,
) -> &'static str {
    let Some(result) = result else {
        return if matches!(phase, SessionPhase::Retired) {
            "Retired"
        } else {
            phase_label(phase)
        };
    };

    let normalized_status = result.status.trim().to_ascii_lowercase();
    match normalized_status.as_str() {
        "success" if result.exit_code == 0 => {
            if matches!(phase, SessionPhase::Retired) {
                "Retired"
            } else {
                phase_label(phase)
            }
        }
        "success" => "Failed",
        "failure" | "timeout" | "signal" => "Failed",
        "error" => "Error",
        _ if result.exit_code != 0 => "Failed",
        _ => "Error",
    }
}

pub(crate) fn ensure_terminal_result_for_dead_active_session(
    project_root: &Path,
    session_id: &str,
    trigger: &str,
) -> Result<bool> {
    let mut session = load_session(project_root, session_id)?;
    if !matches!(session.phase, SessionPhase::Active) {
        return Ok(false);
    }
    let session_dir = get_session_dir(project_root, session_id)?;
    if csa_process::ToolLiveness::has_live_process(&session_dir)
        || load_result(project_root, session_id)?.is_some()
    {
        return Ok(false);
    }
    let now = chrono::Utc::now();
    let tool_name = session
        .tools
        .iter()
        .max_by_key(|(_, state)| state.updated_at)
        .map(|(tool, _)| tool.clone())
        .unwrap_or_else(|| "unknown".to_string());
    let artifacts =
        crate::pipeline_post_exec::collect_fallback_result_artifacts(project_root, session_id);
    let summary_prefix =
        format!("synthetic failure by {trigger}: process dead, result.toml missing");
    let fallback = SessionResult {
        status: "failure".to_string(),
        exit_code: 1,
        summary: crate::pipeline_post_exec::build_fallback_result_summary(
            &session_dir,
            &summary_prefix,
        ),
        tool: tool_name,
        started_at: std::cmp::min(session.last_accessed, now),
        completed_at: now,
        events_count: 0,
        artifacts,
    };
    let result_path = session_dir.join(csa_session::result::RESULT_FILE_NAME);
    if result_path.exists() {
        return Ok(false);
    }
    let result_contents = toml::to_string_pretty(&fallback)
        .map_err(|err| anyhow!("Failed to serialize synthetic result for {session_id}: {err}"))?;
    fs::write(&result_path, result_contents)
        .map_err(|err| anyhow!("Failed to write synthetic result for {session_id}: {err}"))?;
    session.termination_reason = Some("orphaned_process".to_string());
    // Transition to Retired so the session no longer blocks new launches (#540).
    let _ = session.apply_phase_event(csa_session::PhaseEvent::Retired);
    let session_root = session_dir
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| anyhow!("Invalid session dir layout: {}", session_dir.display()))?;
    save_session_in(session_root, &session)?;
    warn!(session_id = %session_id, trigger = %trigger, "Recovered orphaned session with synthetic result");
    Ok(true)
}

/// Retire an Active session whose tool process is dead and result.toml already
/// exists. Covers the gap where post-exec wrote result.toml but the session was
/// never Retired (e.g. process exited before phase transition). See #540.
pub(crate) fn retire_if_dead_with_result(
    project_root: &Path,
    session_id: &str,
    trigger: &str,
) -> Result<bool> {
    let mut session = load_session(project_root, session_id)?;
    if !matches!(session.phase, SessionPhase::Active) {
        return Ok(false);
    }
    let session_dir = get_session_dir(project_root, session_id)?;
    if csa_process::ToolLiveness::has_live_process(&session_dir)
        || load_result(project_root, session_id)?.is_none()
    {
        return Ok(false);
    }
    if session
        .apply_phase_event(csa_session::PhaseEvent::Retired)
        .is_err()
    {
        return Ok(false);
    }
    session
        .termination_reason
        .get_or_insert_with(|| "completed".to_string());
    let session_root = session_dir
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| anyhow!("Invalid session dir layout: {}", session_dir.display()))?;
    save_session_in(session_root, &session)?;
    info!(session_id = %session_id, trigger = %trigger, "Retired dead Active session with result");
    Ok(true)
}

fn resolve_session_status(project_root: &Path, session: &MetaSessionState) -> String {
    let sid = &session.meta_session_id;
    match load_result(project_root, sid) {
        Ok(Some(result)) => {
            // If session is Active but process is dead, retire it (#540).
            if matches!(session.phase, SessionPhase::Active)
                && retire_if_dead_with_result(project_root, sid, "session list").unwrap_or(false)
            {
                return status_from_phase_and_result(&SessionPhase::Retired, Some(&result))
                    .to_string();
            }
            status_from_phase_and_result(&session.phase, Some(&result)).to_string()
        }
        Ok(None) => {
            let reconciled =
                ensure_terminal_result_for_dead_active_session(project_root, sid, "session list");
            if matches!(reconciled, Ok(true))
                && let Ok(Some(result)) = load_result(project_root, sid)
            {
                return status_from_phase_and_result(&SessionPhase::Retired, Some(&result))
                    .to_string();
            }
            if let Err(err) = reconciled {
                tracing::warn!(session_id = %sid, error = %err, "Failed to reconcile session");
            }
            phase_label(&session.phase).to_string()
        }
        Err(err) => {
            tracing::warn!(session_id = %sid, error = %err, "Failed to load result.toml");
            "Error".to_string()
        }
    }
}

fn select_sessions_for_list(
    project_root: &Path,
    branch: Option<&str>,
    tool_filter: Option<&[&str]>,
) -> Result<Vec<MetaSessionState>> {
    let mut sessions = list_sessions(project_root, tool_filter)?;

    if let Some(branch_filter) = branch {
        sessions.retain(|session| session.branch.as_deref() == Some(branch_filter));
    }

    sessions.sort_by(|a, b| b.last_accessed.cmp(&a.last_accessed));
    Ok(sessions)
}

fn select_sessions_for_list_all_projects(
    branch: Option<&str>,
    tool_filter: Option<&[&str]>,
) -> Result<Vec<MetaSessionState>> {
    let mut sessions = csa_session::list_all_sessions_all_projects()?;

    if let Some(branch_filter) = branch {
        sessions.retain(|session| session.branch.as_deref() == Some(branch_filter));
    }

    if let Some(tools) = tool_filter {
        sessions.retain(|session| tools.iter().any(|tool| session.tools.contains_key(*tool)));
    }

    sessions.sort_by(|a, b| b.last_accessed.cmp(&a.last_accessed));
    Ok(sessions)
}

fn session_to_json(project_root: &Path, session: &MetaSessionState) -> serde_json::Value {
    let mut value = serde_json::json!({
        "session_id": session.meta_session_id,
        "last_accessed": session.last_accessed,
        "description": session.description.as_deref().unwrap_or(""),
        "tools": session.tools.keys().collect::<Vec<_>>(),
        "status": resolve_session_status(project_root, session),
        "phase": format!("{:?}", session.phase),
        "branch": session.branch,
        "task_type": session.task_context.task_type,
        "total_token_usage": session.total_token_usage,
        "is_fork": session.genealogy.is_fork(),
    });
    if let Some(ref fork_of) = session.genealogy.fork_of_session_id {
        value["fork_of_session_id"] = serde_json::json!(fork_of);
    }
    if let Some(ref fork_provider) = session.genealogy.fork_provider_session_id {
        value["fork_provider_session_id"] = serde_json::json!(fork_provider);
    }
    if let Some(ref parent) = session.genealogy.parent_session_id {
        value["parent_session_id"] = serde_json::json!(parent);
    }
    value["depth"] = serde_json::json!(session.genealogy.depth);
    if let Some(ref change_id) = session.change_id {
        value["change_id"] = serde_json::json!(change_id);
    }
    // Unified VCS identity (v2)
    let identity = session.resolved_identity();
    value["vcs_kind"] = serde_json::json!(identity.vcs_kind.to_string());
    if let Some(ref vcs_id) = session.vcs_identity {
        value["vcs_identity"] = serde_json::to_value(vcs_id).unwrap_or_default();
    }
    if let Some(ref spec_id) = session.spec_id {
        value["spec_id"] = serde_json::json!(spec_id);
    }
    value
}

pub(crate) fn handle_session_list(
    cd: Option<String>,
    branch: Option<String>,
    tool: Option<String>,
    tree: bool,
    all_projects: bool,
    format: OutputFormat,
) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let tool_filter: Option<Vec<&str>> = tool.as_ref().map(|t| t.split(',').collect());

    if tree {
        let tree_output =
            list_sessions_tree_filtered(&project_root, tool_filter.as_deref(), branch.as_deref())?;
        print!("{tree_output}");
    } else {
        let sessions = if all_projects {
            select_sessions_for_list_all_projects(branch.as_deref(), tool_filter.as_deref())?
        } else {
            select_sessions_for_list(&project_root, branch.as_deref(), tool_filter.as_deref())?
        };
        if sessions.is_empty() {
            match format {
                OutputFormat::Json => {
                    println!("[]");
                }
                OutputFormat::Text => {
                    eprintln!("No sessions found.");
                }
            }
            return Ok(());
        }

        match format {
            OutputFormat::Json => {
                // Serialize sessions as JSON array
                let json_sessions: Vec<_> = sessions
                    .iter()
                    .map(|s| {
                        let mut v = session_to_json(&project_root, s);
                        if all_projects {
                            v["project_path"] = serde_json::json!(&s.project_path);
                        }
                        v
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&json_sessions)?);
            }
            OutputFormat::Text => {
                // Print table header
                if all_projects {
                    println!(
                        "{:<11}  {:<19}  {:<10}  {:<25}  {:<20}  {:<18}  {:<30}  TOKENS",
                        "SESSION",
                        "LAST ACCESSED",
                        "STATUS",
                        "DESCRIPTION",
                        "TOOLS",
                        "BRANCH",
                        "PROJECT"
                    );
                    println!("{}", "-".repeat(160));
                } else {
                    println!(
                        "{:<11}  {:<19}  {:<10}  {:<25}  {:<20}  {:<18}  TOKENS",
                        "SESSION", "LAST ACCESSED", "STATUS", "DESCRIPTION", "TOOLS", "BRANCH"
                    );
                    println!("{}", "-".repeat(130));
                }
                for session in sessions {
                    // Truncate ULID to 11 chars for readability
                    let short_id =
                        &session.meta_session_id[..11.min(session.meta_session_id.len())];
                    let status_str = resolve_session_status(&project_root, &session);
                    let desc = session
                        .description
                        .as_deref()
                        .filter(|d| !d.is_empty())
                        .unwrap_or("-");
                    // Truncate description to 25 visible chars using UTF-8 safe boundaries.
                    let desc_display = truncate_with_ellipsis(desc, 25);
                    let tools: Vec<&String> = session.tools.keys().collect();
                    let tools_str = if tools.is_empty() {
                        "-".to_string()
                    } else {
                        tools
                            .iter()
                            .map(|t| t.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    };
                    let branch_str = session.branch.as_deref().unwrap_or("-");

                    // Format token usage
                    let tokens_str = if let Some(ref usage) = session.total_token_usage {
                        if let Some(total) = usage.total_tokens {
                            if let Some(cost) = usage.estimated_cost_usd {
                                format!("{total}tok ${cost:.4}")
                            } else {
                                format!("{total}tok")
                            }
                        } else if let (Some(input), Some(output)) =
                            (usage.input_tokens, usage.output_tokens)
                        {
                            let total = input + output;
                            if let Some(cost) = usage.estimated_cost_usd {
                                format!("{total}tok ${cost:.4}")
                            } else {
                                format!("{total}tok")
                            }
                        } else {
                            "-".to_string()
                        }
                    } else {
                        "-".to_string()
                    };

                    // Fork indicator appended after tokens
                    let fork_suffix =
                        if let Some(ref fork_of) = session.genealogy.fork_of_session_id {
                            let short_fork = &fork_of[..11.min(fork_of.len())];
                            format!("  \u{21B1} fork of {short_fork}")
                        } else {
                            String::new()
                        };

                    // VCS identity indicator
                    let change_suffix = {
                        let id = session.resolved_identity();
                        format!("  {id}")
                    };

                    if all_projects {
                        let project_display = truncate_with_ellipsis(&session.project_path, 30);
                        println!(
                            "{:<11}  {:<19}  {:<10}  {:<25}  {:<20}  {:<18}  {:<30}  {}{}{}",
                            short_id,
                            session
                                .last_accessed
                                .with_timezone(&chrono::Local)
                                .format("%Y-%m-%d %H:%M"),
                            status_str,
                            desc_display,
                            tools_str,
                            branch_str,
                            project_display,
                            tokens_str,
                            fork_suffix,
                            change_suffix,
                        );
                    } else {
                        println!(
                            "{:<11}  {:<19}  {:<10}  {:<25}  {:<20}  {:<18}  {}{}{}",
                            short_id,
                            session
                                .last_accessed
                                .with_timezone(&chrono::Local)
                                .format("%Y-%m-%d %H:%M"),
                            status_str,
                            desc_display,
                            tools_str,
                            branch_str,
                            tokens_str,
                            fork_suffix,
                            change_suffix,
                        );
                    }
                }
            }
        }
    }

    Ok(())
}

pub(crate) fn handle_session_compress(session: String, cd: Option<String>) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let resolved = resolve_session_prefix_with_fallback(&project_root, &session)?;
    let resolved_id = resolved.session_id;
    let session_state = load_session(&project_root, &resolved_id)?;

    // Find the most recently used tool in this session
    let (tool_name, _tool_state) = session_state
        .tools
        .iter()
        .max_by_key(|(_, state)| &state.updated_at)
        .ok_or_else(|| anyhow::anyhow!("Session '{resolved_id}' has no tool history"))?;

    let compress_cmd = match tool_name.as_str() {
        "gemini-cli" => "/compress",
        _ => "/compact",
    };

    println!("Session {resolved_id} uses tool: {tool_name}");
    println!("Compress command: {compress_cmd}");
    println!();
    println!("To compress, resume the session and send the command:");
    println!(
        "  csa run --sa-mode <true|false> --tool {tool_name} --session {resolved_id} \"{compress_cmd}\""
    );
    println!();
    println!("Note: context status will be updated after the tool confirms compression.");

    // Do NOT mark is_compacted = true here. The actual compression must be
    // performed by the tool. Status should only be updated after `csa run`
    // executes the compress command and succeeds.

    Ok(())
}

pub(crate) fn handle_session_delete(session: String, cd: Option<String>) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let resolved = resolve_session_prefix_with_fallback(&project_root, &session)?;
    let resolved_id = resolved.session_id;
    delete_session(&project_root, &resolved_id)?;
    eprintln!("Deleted session: {resolved_id}");
    Ok(())
}

pub(crate) fn handle_session_logs(
    session: String,
    tail: Option<usize>,
    events: bool,
    cd: Option<String>,
) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let resolved = resolve_session_prefix_with_global_fallback(&project_root, &session)?;
    let resolved_id = resolved.session_id;
    let session_dir = resolved.sessions_dir.join(&resolved_id);
    if let Err(err) =
        ensure_terminal_result_for_dead_active_session(&project_root, &resolved_id, "session logs")
    {
        tracing::warn!(
            session_id = %resolved_id,
            error = %err,
            "Failed to reconcile dead Active session in session logs"
        );
    }
    let repaired_result = match crate::session_observability::refresh_and_repair_result(
        &project_root,
        &resolved_id,
    ) {
        Ok(result) => result,
        Err(err) => {
            tracing::warn!(
                session_id = %resolved_id,
                error = %err,
                "Failed to refresh session result contract in session logs"
            );
            None
        }
    };

    if events {
        return display_acp_events(&session_dir, &resolved_id, tail, repaired_result.as_ref());
    }

    // Try logs/ directory first (Legacy transport)
    if display_log_files(&session_dir, &resolved_id, tail)? {
        return Ok(());
    }

    // Daemon mode persists stdout/stderr spools even when logs/ is empty.
    if display_daemon_spool_logs(&session_dir, tail)? {
        return Ok(());
    }

    // Fallback: display output.log for ACP sessions where logs/ is empty
    let output_log = session_dir.join("output.log");
    if output_log.is_file() {
        let content = fs::read_to_string(&output_log)?;
        if !content.is_empty() {
            eprintln!("=== output.log (ACP session) ===");
            print_content_with_tail(&content, tail);
            return Ok(());
        }
    }

    eprintln!(
        "{}",
        crate::session_observability::build_missing_logs_diagnostic(
            &resolved_id,
            &session_dir,
            repaired_result.as_ref(),
        )
    );
    eprintln!("Hint: use --events to view ACP transcript events (if available)");
    Ok(())
}

/// Display log files from the logs/ directory. Returns true if any non-empty
/// content was displayed.
fn display_log_files(session_dir: &Path, session_id: &str, tail: Option<usize>) -> Result<bool> {
    let logs_dir = session_dir.join("logs");
    if !logs_dir.exists() {
        return Ok(false);
    }

    let mut log_files: Vec<_> = fs::read_dir(&logs_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "log"))
        .collect();
    log_files.sort_by_key(|e| e.file_name());

    if log_files.is_empty() {
        return Ok(false);
    }

    // Check if all log files are empty (broken _log_writer scenario)
    let all_empty = log_files
        .iter()
        .all(|e| fs::metadata(e.path()).map(|m| m.len() == 0).unwrap_or(true));

    if all_empty {
        tracing::debug!(
            session_id,
            "All log files in logs/ are empty, falling back to output.log"
        );
        return Ok(false);
    }

    for entry in &log_files {
        let path = entry.path();
        let file_name = path.file_name().unwrap_or_default().to_string_lossy();
        eprintln!("=== {file_name} ===");

        let content = fs::read_to_string(&path)?;
        print_content_with_tail(&content, tail);
        println!();
    }

    Ok(true)
}

fn display_daemon_spool_logs(session_dir: &Path, tail: Option<usize>) -> Result<bool> {
    let mut displayed_any = false;
    for file_name in ["stdout.log", "stderr.log"] {
        let path = session_dir.join(file_name);
        if !path.is_file() {
            continue;
        }

        let content = fs::read_to_string(&path)?;
        if content.is_empty() {
            continue;
        }

        eprintln!("=== {file_name} ===");
        print_content_with_tail(&content, tail);
        println!();
        displayed_any = true;
    }

    Ok(displayed_any)
}

/// Display ACP JSONL events from output/acp-events.jsonl.
fn display_acp_events(
    session_dir: &Path,
    session_id: &str,
    tail: Option<usize>,
    result: Option<&SessionResult>,
) -> Result<()> {
    let events_path = session_dir.join("output").join("acp-events.jsonl");
    if !events_path.is_file() {
        eprintln!(
            "{}",
            crate::session_observability::build_missing_events_diagnostic(
                session_id,
                session_dir,
                result,
            )
        );
        return Ok(());
    }

    let content = fs::read_to_string(&events_path)?;
    if content.is_empty() {
        eprintln!(
            "{}",
            crate::session_observability::build_missing_events_diagnostic(
                session_id,
                session_dir,
                result,
            )
        );
        return Ok(());
    }

    eprintln!("=== acp-events.jsonl ===");
    print_content_with_tail(&content, tail);
    Ok(())
}

/// Print content, optionally showing only the last N lines.
fn print_content_with_tail(content: &str, tail: Option<usize>) {
    if let Some(n) = tail {
        let lines: Vec<&str> = content.lines().collect();
        let start = lines.len().saturating_sub(n);
        for line in &lines[start..] {
            println!("{line}");
        }
    } else {
        print!("{content}");
    }
}

pub(crate) fn handle_session_is_alive(session: String, cd: Option<String>) -> Result<bool> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let resolved = resolve_session_prefix_with_fallback(&project_root, &session)?;
    let resolved_id = resolved.session_id;
    let session_dir = get_session_dir(&project_root, &resolved_id)?;
    let alive = csa_process::ToolLiveness::is_alive(&session_dir);
    let working = csa_process::ToolLiveness::is_working(&session_dir);

    let label = if alive && working {
        "alive (working)"
    } else if alive {
        "alive (idle)"
    } else {
        "not alive"
    };
    if !alive
        && let Err(err) = ensure_terminal_result_for_dead_active_session(
            &project_root,
            &resolved_id,
            "session is-alive",
        )
    {
        tracing::warn!(
            session_id = %resolved_id,
            error = %err,
            "Failed to reconcile dead Active session in session is-alive"
        );
    }
    println!("{label}");
    Ok(alive)
}

pub(crate) fn handle_session_clean(
    days: u64,
    dry_run: bool,
    tool: Option<String>,
    cd: Option<String>,
) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let tool_filter: Option<Vec<&str>> = tool.as_ref().map(|t| t.split(',').collect());
    let sessions = list_sessions(&project_root, tool_filter.as_deref())?;
    let now = chrono::Utc::now();
    let mut removed = 0;

    if dry_run {
        eprintln!("[dry-run] No changes will be made.");
    }

    for session in &sessions {
        let age = now.signed_duration_since(session.last_accessed);
        if age.num_days() > days as i64 {
            if dry_run {
                eprintln!(
                    "[dry-run] Would remove: {} (last accessed {} days ago)",
                    &session.meta_session_id[..11.min(session.meta_session_id.len())],
                    age.num_days()
                );
            } else if delete_session(&project_root, &session.meta_session_id).is_ok() {
                info!("Removed expired session: {}", session.meta_session_id);
            }
            removed += 1;
        }
    }

    let prefix = if dry_run { "[dry-run] " } else { "" };
    eprintln!(
        "{}Sessions {} (>{} days): {}",
        prefix,
        if dry_run { "to remove" } else { "removed" },
        days,
        removed
    );

    Ok(())
}

pub(crate) fn format_file_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

pub(crate) fn handle_session_log(session: String, cd: Option<String>) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let resolved = resolve_session_prefix_with_fallback(&project_root, &session)?;
    let log = csa_session::git::history(&resolved.sessions_dir, &resolved.session_id)?;
    if log.is_empty() {
        eprintln!("No git history for session '{}'", resolved.session_id);
    } else {
        print!("{log}");
    }
    Ok(())
}

pub(crate) fn handle_session_checkpoint(session: String, cd: Option<String>) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let SessionPrefixResolution {
        session_id: resolved_id,
        sessions_dir,
    } = resolve_session_prefix_with_fallback(&project_root, &session)?;

    // Load the session state to build the checkpoint note
    let state = csa_session::load_session(&project_root, &resolved_id)?;
    let mut note = csa_session::checkpoint::note_from_session(&state);
    // Use the CLI-resolved ID as the authoritative session identity,
    // not state.meta_session_id which could be stale or tampered.
    note.session_id.clone_from(&resolved_id);

    csa_session::checkpoint::write_checkpoint(&sessions_dir, &note)?;
    eprintln!(
        "Checkpoint written for session '{}' (tool={}, turns={}, status={})",
        resolved_id,
        note.tool.as_deref().unwrap_or("none"),
        note.turn_count,
        note.status,
    );

    Ok(())
}

pub(crate) fn handle_session_checkpoints(cd: Option<String>) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let primary_root = csa_session::get_session_root(&project_root)?;
    let primary_sessions_dir = primary_root.join("sessions");
    let legacy_sessions_dir = legacy_sessions_dir_from_primary_root(&primary_root);
    let checkpoints =
        list_checkpoints_from_dirs(&primary_sessions_dir, legacy_sessions_dir.as_deref())?;
    if checkpoints.is_empty() {
        eprintln!("No checkpoint notes found.");
        return Ok(());
    }

    for (commit, note) in &checkpoints {
        println!(
            "{:.7}  {}  tool={}  turns={}  status={}",
            commit,
            note.session_id,
            note.tool.as_deref().unwrap_or("none"),
            note.turn_count,
            note.status,
        );
    }

    Ok(())
}

// Daemon-specific commands (wait, attach, kill) are in session_cmds_daemon.rs.
pub(crate) use crate::session_cmds_daemon::{
    handle_session_attach, handle_session_kill, handle_session_wait,
};

#[cfg(test)]
#[path = "session_cmds_tests.rs"]
mod tests;
