use anyhow::Result;
use std::fs;
use tracing::info;

use csa_core::types::OutputFormat;
use csa_session::{
    delete_session, get_session_dir, list_sessions, list_sessions_tree, load_session,
    resolve_session_prefix,
};

pub(crate) fn handle_session_list(
    cd: Option<String>,
    tool: Option<String>,
    tree: bool,
    format: OutputFormat,
) -> Result<()> {
    let project_root = crate::determine_project_root(cd.as_deref())?;
    let tool_filter: Option<Vec<&str>> = tool.as_ref().map(|t| t.split(',').collect());

    if tree {
        let tree_output = list_sessions_tree(&project_root, tool_filter.as_deref())?;
        print!("{}", tree_output);
    } else {
        let sessions = list_sessions(&project_root, tool_filter.as_deref())?;
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
                        serde_json::json!({
                            "session_id": s.meta_session_id,
                            "last_accessed": s.last_accessed,
                            "description": s.description.as_deref().unwrap_or(""),
                            "tools": s.tools.keys().collect::<Vec<_>>(),
                            "total_token_usage": s.total_token_usage,
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&json_sessions)?);
            }
            OutputFormat::Text => {
                // Print table header
                println!(
                    "{:<11}  {:<19}  {:<30}  {:<20}  TOKENS",
                    "SESSION", "LAST ACCESSED", "DESCRIPTION", "TOOLS"
                );
                println!("{}", "-".repeat(100));
                for session in sessions {
                    // Truncate ULID to 11 chars for readability
                    let short_id =
                        &session.meta_session_id[..11.min(session.meta_session_id.len())];
                    let desc = session
                        .description
                        .as_deref()
                        .filter(|d| !d.is_empty())
                        .unwrap_or("-");
                    // Truncate description to 30 chars
                    let desc_display = if desc.len() > 30 {
                        format!("{}...", &desc[..27])
                    } else {
                        desc.to_string()
                    };
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

                    // Format token usage
                    let tokens_str = if let Some(ref usage) = session.total_token_usage {
                        if let Some(total) = usage.total_tokens {
                            if let Some(cost) = usage.estimated_cost_usd {
                                format!("{}tok ${:.4}", total, cost)
                            } else {
                                format!("{}tok", total)
                            }
                        } else if let (Some(input), Some(output)) =
                            (usage.input_tokens, usage.output_tokens)
                        {
                            let total = input + output;
                            if let Some(cost) = usage.estimated_cost_usd {
                                format!("{}tok ${:.4}", total, cost)
                            } else {
                                format!("{}tok", total)
                            }
                        } else {
                            "-".to_string()
                        }
                    } else {
                        "-".to_string()
                    };

                    println!(
                        "{:<11}  {:<19}  {:<30}  {:<20}  {}",
                        short_id,
                        session.last_accessed.format("%Y-%m-%d %H:%M"),
                        desc_display,
                        tools_str,
                        tokens_str,
                    );
                }
            }
        }
    }

    Ok(())
}

pub(crate) fn handle_session_compress(session: String, cd: Option<String>) -> Result<()> {
    let project_root = crate::determine_project_root(cd.as_deref())?;
    let sessions_dir = csa_session::get_session_root(&project_root)?.join("sessions");
    let resolved_id = resolve_session_prefix(&sessions_dir, &session)?;
    let session_state = load_session(&project_root, &resolved_id)?;

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
    println!();
    println!("Note: context status will be updated after the tool confirms compression.");

    // Do NOT mark is_compacted = true here. The actual compression must be
    // performed by the tool. Status should only be updated after `csa run`
    // executes the compress command and succeeds.

    Ok(())
}

pub(crate) fn handle_session_delete(session: String, cd: Option<String>) -> Result<()> {
    let project_root = crate::determine_project_root(cd.as_deref())?;
    let sessions_dir = csa_session::get_session_root(&project_root)?.join("sessions");
    let resolved_id = resolve_session_prefix(&sessions_dir, &session)?;
    delete_session(&project_root, &resolved_id)?;
    eprintln!("Deleted session: {}", resolved_id);
    Ok(())
}

pub(crate) fn handle_session_logs(
    session: String,
    tail: Option<usize>,
    cd: Option<String>,
) -> Result<()> {
    let project_root = crate::determine_project_root(cd.as_deref())?;
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

pub(crate) fn handle_session_clean(
    days: u64,
    dry_run: bool,
    tool: Option<String>,
    cd: Option<String>,
) -> Result<()> {
    let project_root = crate::determine_project_root(cd.as_deref())?;
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
