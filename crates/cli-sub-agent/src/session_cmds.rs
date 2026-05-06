use anyhow::Result;
use std::fs;
use std::path::Path;
use tracing::info;

use csa_core::types::OutputFormat;
use csa_session::{
    SessionResult, delete_session, list_sessions, list_sessions_tree_filtered, load_session,
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

#[path = "session_cmds_list.rs"]
mod list;
use list::{
    filter_sessions_by_csa_version, format_elapsed, format_started_at, resolve_session_status,
    select_sessions_for_list, select_sessions_for_list_all_projects, session_created_at,
    session_to_json, truncate_with_ellipsis,
};
#[cfg(test)]
use list::{is_session_stale_for_test, status_from_phase_and_result};

#[path = "session_cmds_reconcile.rs"]
mod reconcile;
#[cfg(test)]
pub(crate) use reconcile::{
    DeadActiveSessionReconciliation,
    ensure_terminal_result_for_dead_active_session_with_before_write,
};
pub(crate) use reconcile::{
    ensure_terminal_result_for_dead_active_session, retire_if_dead_with_result,
};

/// Parse a human-friendly duration string (e.g., "1h", "30m", "2d") into
/// a `chrono::Duration`. Supports `s` (seconds), `m` (minutes), `h` (hours),
/// and `d` (days).
fn parse_duration_filter(s: &str) -> Result<chrono::Duration> {
    let s = s.trim();
    if s.is_empty() {
        anyhow::bail!("Duration string cannot be empty");
    }

    let (num_str, unit) = s.split_at(s.len() - 1);
    let num: i64 = num_str.parse().map_err(|_| {
        anyhow::anyhow!("Invalid duration: '{s}'. Expected: <number><unit> (e.g., 1h, 30m, 2d)")
    })?;

    match unit {
        "s" => Ok(chrono::Duration::seconds(num)),
        "m" => Ok(chrono::Duration::minutes(num)),
        "h" => Ok(chrono::Duration::hours(num)),
        "d" => Ok(chrono::Duration::days(num)),
        _ => anyhow::bail!("Unknown duration unit '{unit}'. Supported: s, m, h, d"),
    }
}

/// Filter options for `csa session list`.
pub(crate) struct SessionListFilters {
    pub limit: Option<usize>,
    pub since: Option<String>,
    pub status: Option<String>,
    pub csa_version: Option<String>,
    pub show_version: bool,
}

pub(crate) fn handle_session_list(
    cd: Option<String>,
    branch: Option<String>,
    tool: Option<String>,
    tree: bool,
    all_projects: bool,
    filters: SessionListFilters,
    format: OutputFormat,
) -> Result<()> {
    if tree && (filters.limit.is_some() || filters.since.is_some() || filters.status.is_some()) {
        anyhow::bail!("--tree is incompatible with --limit/--since/--status (not yet supported)");
    }

    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let tool_filter: Option<Vec<&str>> = tool.as_ref().map(|t| t.split(',').collect());

    if tree {
        let tree_output =
            list_sessions_tree_filtered(&project_root, tool_filter.as_deref(), branch.as_deref())?;
        print!("{tree_output}");
    } else {
        let mut sessions = if all_projects {
            select_sessions_for_list_all_projects(branch.as_deref(), tool_filter.as_deref())?
        } else {
            select_sessions_for_list(&project_root, branch.as_deref(), tool_filter.as_deref())?
        };

        // --since filter: keep only sessions accessed after the cutoff
        if let Some(ref since_str) = filters.since {
            let duration = parse_duration_filter(since_str)?;
            let cutoff = chrono::Utc::now() - duration;
            sessions.retain(|s| s.last_accessed >= cutoff);
        }

        // --status filter: match resolved status string (case-insensitive)
        if let Some(ref status_filter) = filters.status {
            let filter_lower = status_filter.to_ascii_lowercase();
            sessions.retain(|s| {
                let resolved = resolve_session_status(s).to_ascii_lowercase();
                resolved == filter_lower
            });
        }

        sessions = filter_sessions_by_csa_version(sessions, filters.csa_version.as_deref());

        // --limit: keep only the N most recent (list is already sorted newest-first)
        if let Some(n) = filters.limit {
            sessions.truncate(n);
        }

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
                        let mut v = session_to_json(s);
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
                if all_projects && filters.show_version {
                    println!(
                        "{:<11}  {:<19}  {:<8}  {:<19}  {:<10}  {:<25}  {:<20}  {:<18}  {:<30}  {:<12}  TOKENS",
                        "SESSION",
                        "STARTED",
                        "ELAPSED",
                        "LAST ACCESSED",
                        "STATUS",
                        "DESCRIPTION",
                        "TOOLS",
                        "BRANCH",
                        "PROJECT",
                        "VERSION"
                    );
                    println!("{}", "-".repeat(206));
                } else if all_projects {
                    println!(
                        "{:<11}  {:<19}  {:<8}  {:<19}  {:<10}  {:<25}  {:<20}  {:<18}  {:<30}  TOKENS",
                        "SESSION",
                        "STARTED",
                        "ELAPSED",
                        "LAST ACCESSED",
                        "STATUS",
                        "DESCRIPTION",
                        "TOOLS",
                        "BRANCH",
                        "PROJECT"
                    );
                    println!("{}", "-".repeat(192));
                } else if filters.show_version {
                    println!(
                        "{:<11}  {:<19}  {:<8}  {:<19}  {:<10}  {:<25}  {:<20}  {:<18}  {:<12}  TOKENS",
                        "SESSION",
                        "STARTED",
                        "ELAPSED",
                        "LAST ACCESSED",
                        "STATUS",
                        "DESCRIPTION",
                        "TOOLS",
                        "BRANCH",
                        "VERSION"
                    );
                    println!("{}", "-".repeat(176));
                } else {
                    println!(
                        "{:<11}  {:<19}  {:<8}  {:<19}  {:<10}  {:<25}  {:<20}  {:<18}  TOKENS",
                        "SESSION",
                        "STARTED",
                        "ELAPSED",
                        "LAST ACCESSED",
                        "STATUS",
                        "DESCRIPTION",
                        "TOOLS",
                        "BRANCH"
                    );
                    println!("{}", "-".repeat(162));
                }
                for session in sessions {
                    // Truncate ULID to 11 chars for readability
                    let short_id =
                        &session.meta_session_id[..11.min(session.meta_session_id.len())];
                    let status_str = resolve_session_status(&session);
                    let started_str = format_started_at(session_created_at(&session));
                    let elapsed_str = format_elapsed(&session, &status_str, chrono::Utc::now());
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
                    let csa_version_str = session.csa_version.as_deref().unwrap_or("-");

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

                    if all_projects && filters.show_version {
                        let project_display = truncate_with_ellipsis(&session.project_path, 30);
                        println!(
                            "{:<11}  {:<19}  {:<8}  {:<19}  {:<10}  {:<25}  {:<20}  {:<18}  {:<30}  {:<12}  {}{}{}",
                            short_id,
                            started_str,
                            elapsed_str,
                            session
                                .last_accessed
                                .with_timezone(&chrono::Local)
                                .format("%Y-%m-%d %H:%M"),
                            status_str,
                            desc_display,
                            tools_str,
                            branch_str,
                            project_display,
                            csa_version_str,
                            tokens_str,
                            fork_suffix,
                            change_suffix,
                        );
                    } else if all_projects {
                        let project_display = truncate_with_ellipsis(&session.project_path, 30);
                        println!(
                            "{:<11}  {:<19}  {:<8}  {:<19}  {:<10}  {:<25}  {:<20}  {:<18}  {:<30}  {}{}{}",
                            short_id,
                            started_str,
                            elapsed_str,
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
                    } else if filters.show_version {
                        println!(
                            "{:<11}  {:<19}  {:<8}  {:<19}  {:<10}  {:<25}  {:<20}  {:<18}  {:<12}  {}{}{}",
                            short_id,
                            started_str,
                            elapsed_str,
                            session
                                .last_accessed
                                .with_timezone(&chrono::Local)
                                .format("%Y-%m-%d %H:%M"),
                            status_str,
                            desc_display,
                            tools_str,
                            branch_str,
                            csa_version_str,
                            tokens_str,
                            fork_suffix,
                            change_suffix,
                        );
                    } else {
                        println!(
                            "{:<11}  {:<19}  {:<8}  {:<19}  {:<10}  {:<25}  {:<20}  {:<18}  {}{}{}",
                            short_id,
                            started_str,
                            elapsed_str,
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

    // Use the foreign project root for cross-project sessions, local otherwise.
    let effective_root = resolved
        .foreign_project_root
        .as_deref()
        .unwrap_or(&project_root);
    let is_cross_project = resolved.foreign_project_root.is_some();

    if let Err(err) =
        ensure_terminal_result_for_dead_active_session(effective_root, &resolved_id, "session logs")
    {
        tracing::warn!(
            session_id = %resolved_id,
            error = %err,
            "Failed to reconcile dead Active session in session logs"
        );
    }

    let repaired_result = if is_cross_project {
        match crate::session_observability::refresh_and_repair_result_from_dir(&session_dir) {
            Ok(result) => result,
            Err(err) => {
                tracing::warn!(
                    session_id = %resolved_id,
                    error = %err,
                    "Failed to refresh cross-project session result in session logs"
                );
                None
            }
        }
    } else {
        match crate::session_observability::refresh_and_repair_result(&project_root, &resolved_id) {
            Ok(result) => result,
            Err(err) => {
                tracing::warn!(
                    session_id = %resolved_id,
                    error = %err,
                    "Failed to refresh session result contract in session logs"
                );
                None
            }
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
    let resolved = resolve_session_prefix_with_global_fallback(&project_root, &session)?;
    let resolved_id = resolved.session_id;
    let session_dir = resolved.sessions_dir.join(&resolved_id);
    let alive = csa_process::ToolLiveness::is_alive(&session_dir);
    let working = csa_process::ToolLiveness::is_working(&session_dir);

    let label = if alive && working {
        "alive (working)"
    } else if alive {
        "alive (idle)"
    } else {
        "not alive"
    };
    // Use the foreign project root for cross-project sessions.
    let effective_root = resolved
        .foreign_project_root
        .as_deref()
        .unwrap_or(&project_root);
    if !alive
        && let Err(err) = ensure_terminal_result_for_dead_active_session(
            effective_root,
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
    let resolved = resolve_session_prefix_with_global_fallback(&project_root, &session)?;
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
        ..
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
#[cfg(test)]
pub(crate) use crate::session_cmds_daemon::handle_session_wait;
pub(crate) use crate::session_cmds_daemon::{
    handle_session_attach, handle_session_attach_with_prompt, handle_session_kill,
    handle_session_wait_with_memory_warn,
};

#[cfg(test)]
#[path = "session_cmds_tests.rs"]
mod tests;
