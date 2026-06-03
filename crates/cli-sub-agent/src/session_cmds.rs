use anyhow::Result;
use tracing::info;

use csa_core::types::OutputFormat;
use csa_session::{delete_session, list_sessions, list_sessions_tree_filtered};

use crate::stdout_write::{write_stdout, write_stdout_line};

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

#[path = "session_cmds_compress.rs"]
mod compress;
pub(crate) use compress::handle_session_compress;

#[path = "session_cmds_logs.rs"]
mod logs;
pub(crate) use logs::handle_session_logs;
#[cfg(test)]
pub(crate) use logs::{
    display_acp_events, display_daemon_spool_logs, display_log_files, print_content_with_tail,
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
        write_stdout(&tree_output)?;
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
                    write_stdout_line("[]")?;
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
                write_stdout_line(&serde_json::to_string_pretty(&json_sessions)?)?;
            }
            OutputFormat::Text => {
                // Print table header
                if all_projects && filters.show_version {
                    println!(
                        "{:<11}  {:<19}  {:<8}  {:<19}  {:<10}  {:<25}  {:<20}  {:<18}  {:<18}  {:<30}  {:<12}  TOKENS",
                        "SESSION",
                        "STARTED",
                        "ELAPSED",
                        "LAST ACCESSED",
                        "STATUS",
                        "DESCRIPTION",
                        "TOOLS",
                        "TIER",
                        "BRANCH",
                        "PROJECT",
                        "VERSION"
                    );
                    println!("{}", "-".repeat(226));
                } else if all_projects {
                    println!(
                        "{:<11}  {:<19}  {:<8}  {:<19}  {:<10}  {:<25}  {:<20}  {:<18}  {:<18}  {:<30}  TOKENS",
                        "SESSION",
                        "STARTED",
                        "ELAPSED",
                        "LAST ACCESSED",
                        "STATUS",
                        "DESCRIPTION",
                        "TOOLS",
                        "TIER",
                        "BRANCH",
                        "PROJECT"
                    );
                    println!("{}", "-".repeat(212));
                } else if filters.show_version {
                    println!(
                        "{:<11}  {:<19}  {:<8}  {:<19}  {:<10}  {:<25}  {:<20}  {:<18}  {:<18}  {:<12}  TOKENS",
                        "SESSION",
                        "STARTED",
                        "ELAPSED",
                        "LAST ACCESSED",
                        "STATUS",
                        "DESCRIPTION",
                        "TOOLS",
                        "TIER",
                        "BRANCH",
                        "VERSION"
                    );
                    println!("{}", "-".repeat(196));
                } else {
                    println!(
                        "{:<11}  {:<19}  {:<8}  {:<19}  {:<10}  {:<25}  {:<20}  {:<18}  {:<18}  TOKENS",
                        "SESSION",
                        "STARTED",
                        "ELAPSED",
                        "LAST ACCESSED",
                        "STATUS",
                        "DESCRIPTION",
                        "TOOLS",
                        "TIER",
                        "BRANCH"
                    );
                    println!("{}", "-".repeat(182));
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
                    let tier_str = session.task_context.tier_name.as_deref().unwrap_or("-");
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
                            "{:<11}  {:<19}  {:<8}  {:<19}  {:<10}  {:<25}  {:<20}  {:<18}  {:<18}  {:<30}  {:<12}  {}{}{}",
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
                            tier_str,
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
                            "{:<11}  {:<19}  {:<8}  {:<19}  {:<10}  {:<25}  {:<20}  {:<18}  {:<18}  {:<30}  {}{}{}",
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
                            tier_str,
                            branch_str,
                            project_display,
                            tokens_str,
                            fork_suffix,
                            change_suffix,
                        );
                    } else if filters.show_version {
                        println!(
                            "{:<11}  {:<19}  {:<8}  {:<19}  {:<10}  {:<25}  {:<20}  {:<18}  {:<18}  {:<12}  {}{}{}",
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
                            tier_str,
                            branch_str,
                            csa_version_str,
                            tokens_str,
                            fork_suffix,
                            change_suffix,
                        );
                    } else {
                        println!(
                            "{:<11}  {:<19}  {:<8}  {:<19}  {:<10}  {:<25}  {:<20}  {:<18}  {:<18}  {}{}{}",
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
                            tier_str,
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

pub(crate) fn handle_session_delete(session: String, cd: Option<String>) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let resolved = resolve_session_prefix_with_fallback(&project_root, &session)?;
    let resolved_id = resolved.session_id;
    delete_session(&project_root, &resolved_id)?;
    eprintln!("Deleted session: {resolved_id}");
    Ok(())
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

pub(crate) fn handle_session_checkpoint(
    session: String,
    all: bool,
    cd: Option<String>,
) -> Result<bool> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let SessionPrefixResolution {
        session_id: resolved_id,
        sessions_dir,
        foreign_project_root,
    } = resolve_session_prefix_with_global_fallback(&project_root, &session)?;
    let _effective_project_root = foreign_project_root.unwrap_or(project_root);
    let session_dir = sessions_dir.join(&resolved_id);

    if all {
        let checkpoints = csa_session::checkpoint::read_checkpoints(&session_dir)?;
        if checkpoints.is_empty() {
            return Ok(false);
        }

        #[derive(serde::Serialize)]
        struct CheckpointList<'a> {
            checkpoints: &'a [csa_session::checkpoint::Checkpoint],
        }

        print!(
            "{}",
            toml::to_string_pretty(&CheckpointList {
                checkpoints: &checkpoints,
            })?
        );
        return Ok(true);
    }

    let Some(checkpoint) = csa_session::checkpoint::read_latest_checkpoint(&session_dir)? else {
        return Ok(false);
    };
    print!("{}", toml::to_string_pretty(&checkpoint)?);
    Ok(true)
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
    SessionWaitOutputMode, handle_session_attach, handle_session_attach_with_prompt,
    handle_session_kill, handle_session_wait_with_options,
};

#[cfg(test)]
#[path = "session_cmds_tests.rs"]
mod tests;
