use anyhow::Result;
use std::fs;
use tracing::info;

use csa_core::types::OutputFormat;
use csa_session::{
    delete_session, get_session_dir, list_artifacts, list_sessions, list_sessions_tree,
    load_result, load_session, resolve_session_prefix,
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

pub(crate) fn handle_session_list(
    cd: Option<String>,
    tool: Option<String>,
    tree: bool,
    format: OutputFormat,
) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
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
                            "phase": format!("{:?}", s.phase),
                            "total_token_usage": s.total_token_usage,
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&json_sessions)?);
            }
            OutputFormat::Text => {
                // Print table header
                println!(
                    "{:<11}  {:<19}  {:<10}  {:<25}  {:<20}  TOKENS",
                    "SESSION", "LAST ACCESSED", "PHASE", "DESCRIPTION", "TOOLS"
                );
                println!("{}", "-".repeat(110));
                for session in sessions {
                    // Truncate ULID to 11 chars for readability
                    let short_id =
                        &session.meta_session_id[..11.min(session.meta_session_id.len())];
                    let phase_str = format!("{:?}", session.phase);
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
                        "{:<11}  {:<19}  {:<10}  {:<25}  {:<20}  {}",
                        short_id,
                        session.last_accessed.format("%Y-%m-%d %H:%M"),
                        phase_str,
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
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
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
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
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
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
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

pub(crate) fn handle_session_result(session: String, json: bool, cd: Option<String>) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let sessions_dir = csa_session::get_session_root(&project_root)?.join("sessions");
    let resolved_id = resolve_session_prefix(&sessions_dir, &session)?;
    match load_result(&project_root, &resolved_id)? {
        Some(result) => {
            if json {
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else {
                println!("Session: {}", resolved_id);
                println!("Status:  {}", result.status);
                println!("Exit:    {}", result.exit_code);
                println!("Tool:    {}", result.tool);
                println!("Started: {}", result.started_at);
                println!("Ended:   {}", result.completed_at);
                println!("Summary: {}", result.summary);
                if !result.artifacts.is_empty() {
                    println!("Artifacts:");
                    for a in &result.artifacts {
                        println!("  - {}", a);
                    }
                }
            }
        }
        None => {
            eprintln!("No result found for session '{}'", resolved_id);
        }
    }
    Ok(())
}

pub(crate) fn handle_session_artifacts(session: String, cd: Option<String>) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let sessions_dir = csa_session::get_session_root(&project_root)?.join("sessions");
    let resolved_id = resolve_session_prefix(&sessions_dir, &session)?;
    let artifacts = list_artifacts(&project_root, &resolved_id)?;
    if artifacts.is_empty() {
        eprintln!("No artifacts for session '{}'", resolved_id);
    } else {
        let session_dir = get_session_dir(&project_root, &resolved_id)?;
        for a in &artifacts {
            println!("{}", session_dir.join("output").join(a).display());
        }
    }
    Ok(())
}

pub(crate) fn handle_session_log(session: String, cd: Option<String>) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let sessions_dir = csa_session::get_session_root(&project_root)?.join("sessions");
    let resolved_id = resolve_session_prefix(&sessions_dir, &session)?;
    let log = csa_session::git::history(&sessions_dir, &resolved_id)?;
    if log.is_empty() {
        eprintln!("No git history for session '{}'", resolved_id);
    } else {
        print!("{}", log);
    }
    Ok(())
}

pub(crate) fn handle_session_checkpoint(session: String, cd: Option<String>) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let sessions_dir = csa_session::get_session_root(&project_root)?.join("sessions");
    let resolved_id = resolve_session_prefix(&sessions_dir, &session)?;

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
    let sessions_dir = csa_session::get_session_root(&project_root)?.join("sessions");

    let checkpoints = csa_session::checkpoint::list_checkpoints(&sessions_dir)?;
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

#[cfg(test)]
mod tests {
    use super::truncate_with_ellipsis;

    #[test]
    fn truncate_with_ellipsis_preserves_ascii_short_input() {
        let input = "short description";
        assert_eq!(truncate_with_ellipsis(input, 25), "short description");
    }

    #[test]
    fn truncate_with_ellipsis_handles_multibyte_chinese() {
        let input = "\u{8FD9}\u{662F}\u{4E00}\u{4E2A}\u{7528}\u{4E8E}\u{6D4B}\u{8BD5}\u{622A}\u{65AD}\u{903B}\u{8F91}\u{7684}\u{4E2D}\u{6587}\u{63CF}\u{8FF0}\u{6587}\u{672C}";
        let expected = "\u{8FD9}\u{662F}\u{4E00}\u{4E2A}\u{7528}\u{4E8E}\u{6D4B}...";
        assert_eq!(truncate_with_ellipsis(input, 10), expected);
    }

    #[test]
    fn truncate_with_ellipsis_handles_emoji_without_panic() {
        let input = "session 😀😃😄😁 description";
        assert_eq!(truncate_with_ellipsis(input, 12), "session 😀...");
    }
}
