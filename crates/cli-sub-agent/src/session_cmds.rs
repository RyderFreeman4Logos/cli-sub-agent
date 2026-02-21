use anyhow::Result;
use csa_session::checkpoint::CheckpointNote;
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use tracing::info;

use csa_config::paths;
use csa_core::types::OutputFormat;
use csa_session::{
    MetaSessionState, SessionPhase, SessionResult, delete_session, get_session_dir, list_artifacts,
    list_sessions, list_sessions_tree_filtered, load_result, load_session, resolve_session_prefix,
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
    // Retired is terminal lifecycle state and takes precedence over execution result.
    if matches!(phase, SessionPhase::Retired) {
        return "Retired";
    }

    let Some(result) = result else {
        return phase_label(phase);
    };

    let normalized_status = result.status.trim().to_ascii_lowercase();
    match normalized_status.as_str() {
        "success" if result.exit_code == 0 => phase_label(phase),
        "success" => "Failed",
        "failure" | "timeout" | "signal" => "Failed",
        "error" => "Error",
        _ if result.exit_code != 0 => "Failed",
        _ => "Error",
    }
}

fn resolve_session_status(project_root: &Path, session: &MetaSessionState) -> String {
    match load_result(project_root, &session.meta_session_id) {
        Ok(result) => status_from_phase_and_result(&session.phase, result.as_ref()).to_string(),
        Err(err) => {
            tracing::warn!(
                session_id = %session.meta_session_id,
                error = %err,
                "Failed to load result.toml while listing sessions"
            );
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

fn session_to_json(project_root: &Path, session: &MetaSessionState) -> serde_json::Value {
    serde_json::json!({
        "session_id": session.meta_session_id,
        "last_accessed": session.last_accessed,
        "description": session.description.as_deref().unwrap_or(""),
        "tools": session.tools.keys().collect::<Vec<_>>(),
        "status": resolve_session_status(project_root, session),
        "phase": format!("{:?}", session.phase),
        "branch": session.branch,
        "task_type": session.task_context.task_type,
        "total_token_usage": session.total_token_usage,
    })
}

#[derive(Debug, Clone)]
struct TranscriptSummary {
    event_count: u64,
    size_bytes: u64,
    first_timestamp: Option<String>,
    last_timestamp: Option<String>,
}

fn load_transcript_summary(session_dir: &Path) -> Result<Option<TranscriptSummary>> {
    let transcript_path = session_dir.join("output").join("acp-events.jsonl");
    if !transcript_path.is_file() {
        return Ok(None);
    }

    let size_bytes = fs::metadata(&transcript_path)?.len();
    let file = File::open(&transcript_path)?;
    let reader = BufReader::new(file);

    let mut event_count = 0u64;
    let mut first_timestamp: Option<String> = None;
    let mut last_timestamp: Option<String> = None;

    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        event_count = event_count.saturating_add(1);
        if let Some(ts) = extract_transcript_timestamp(&line) {
            if first_timestamp.is_none() {
                first_timestamp = Some(ts.clone());
            }
            last_timestamp = Some(ts);
        }
    }

    Ok(Some(TranscriptSummary {
        event_count,
        size_bytes,
        first_timestamp,
        last_timestamp,
    }))
}

fn extract_transcript_timestamp(line: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(line)
        .ok()?
        .get("ts")?
        .as_str()
        .map(ToString::to_string)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionPrefixResolution {
    session_id: String,
    sessions_dir: PathBuf,
}

fn resolve_session_prefix_with_fallback(
    project_root: &Path,
    prefix: &str,
) -> Result<SessionPrefixResolution> {
    let primary_root = csa_session::get_session_root(project_root)?;
    let primary_sessions_dir = primary_root.join("sessions");
    let legacy_sessions_dir = legacy_sessions_dir_from_primary_root(&primary_root);
    resolve_session_prefix_from_dirs(
        prefix,
        &primary_sessions_dir,
        legacy_sessions_dir.as_deref(),
    )
}

fn resolve_session_prefix_from_dirs(
    prefix: &str,
    primary_sessions_dir: &Path,
    legacy_sessions_dir: Option<&Path>,
) -> Result<SessionPrefixResolution> {
    match resolve_session_prefix(primary_sessions_dir, prefix) {
        Ok(session_id) => Ok(SessionPrefixResolution {
            session_id,
            sessions_dir: primary_sessions_dir.to_path_buf(),
        }),
        Err(primary_err) if should_fallback_to_legacy(&primary_err) => {
            let Some(legacy_sessions_dir) = legacy_sessions_dir else {
                return Err(primary_err);
            };

            match resolve_session_prefix(legacy_sessions_dir, prefix) {
                Ok(session_id) => Ok(SessionPrefixResolution {
                    session_id,
                    sessions_dir: legacy_sessions_dir.to_path_buf(),
                }),
                Err(legacy_err) if should_fallback_to_legacy(&legacy_err) => Err(primary_err),
                Err(legacy_err) => Err(legacy_err),
            }
        }
        Err(primary_err) => Err(primary_err),
    }
}

fn should_fallback_to_legacy(err: &anyhow::Error) -> bool {
    err.to_string().contains("No session matching prefix")
}

fn legacy_sessions_dir_from_primary_root(primary_root: &Path) -> Option<PathBuf> {
    let primary_state_dir = paths::state_dir_write()?;
    let legacy_state_dir = paths::legacy_state_dir()?;
    let relative_root = primary_root.strip_prefix(primary_state_dir).ok()?;
    let legacy_root = legacy_state_dir.join(relative_root);
    (legacy_root != primary_root).then(|| legacy_root.join("sessions"))
}

fn list_checkpoints_from_dirs(
    primary_sessions_dir: &Path,
    legacy_sessions_dir: Option<&Path>,
) -> Result<Vec<(String, CheckpointNote)>> {
    let mut checkpoints = csa_session::checkpoint::list_checkpoints(primary_sessions_dir)?;
    let mut seen_ids: HashSet<String> = checkpoints
        .iter()
        .map(|(_, note)| note.session_id.clone())
        .collect();

    if let Some(legacy_dir) = legacy_sessions_dir {
        for (commit, note) in csa_session::checkpoint::list_checkpoints(legacy_dir)? {
            if seen_ids.insert(note.session_id.clone()) {
                checkpoints.push((commit, note));
            }
        }
    }

    checkpoints.sort_by(|a, b| b.1.completed_at.cmp(&a.1.completed_at));
    Ok(checkpoints)
}

pub(crate) fn handle_session_list(
    cd: Option<String>,
    branch: Option<String>,
    tool: Option<String>,
    tree: bool,
    format: OutputFormat,
) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let tool_filter: Option<Vec<&str>> = tool.as_ref().map(|t| t.split(',').collect());

    if tree {
        let tree_output =
            list_sessions_tree_filtered(&project_root, tool_filter.as_deref(), branch.as_deref())?;
        print!("{}", tree_output);
    } else {
        let sessions =
            select_sessions_for_list(&project_root, branch.as_deref(), tool_filter.as_deref())?;
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
                    .map(|s| session_to_json(&project_root, s))
                    .collect();
                println!("{}", serde_json::to_string_pretty(&json_sessions)?);
            }
            OutputFormat::Text => {
                // Print table header
                println!(
                    "{:<11}  {:<19}  {:<10}  {:<25}  {:<20}  {:<18}  TOKENS",
                    "SESSION", "LAST ACCESSED", "STATUS", "DESCRIPTION", "TOOLS", "BRANCH"
                );
                println!("{}", "-".repeat(130));
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
                        "{:<11}  {:<19}  {:<10}  {:<25}  {:<20}  {:<18}  {}",
                        short_id,
                        session.last_accessed.format("%Y-%m-%d %H:%M"),
                        status_str,
                        desc_display,
                        tools_str,
                        branch_str,
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
    let resolved = resolve_session_prefix_with_fallback(&project_root, &session)?;
    let resolved_id = resolved.session_id;
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
    let resolved = resolve_session_prefix_with_fallback(&project_root, &session)?;
    let resolved_id = resolved.session_id;
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
    let resolved = resolve_session_prefix_with_fallback(&project_root, &session)?;
    let resolved_id = resolved.session_id;
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

pub(crate) fn handle_session_is_alive(session: String, cd: Option<String>) -> Result<bool> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let resolved = resolve_session_prefix_with_fallback(&project_root, &session)?;
    let resolved_id = resolved.session_id;
    let session_dir = get_session_dir(&project_root, &resolved_id)?;
    let alive = csa_process::ToolLiveness::is_alive(&session_dir);
    println!("{}", if alive { "alive" } else { "not alive" });
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

pub(crate) fn handle_session_result(session: String, json: bool, cd: Option<String>) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let resolved = resolve_session_prefix_with_fallback(&project_root, &session)?;
    let resolved_id = resolved.session_id;
    let session_dir = get_session_dir(&project_root, &resolved_id)?;
    let transcript_summary = match load_transcript_summary(&session_dir) {
        Ok(summary) => summary,
        Err(err) => {
            tracing::warn!(
                session_id = %resolved_id,
                path = %session_dir.display(),
                error = %err,
                "Failed to load transcript summary; continuing without transcript metadata"
            );
            None
        }
    };
    match load_result(&project_root, &resolved_id)? {
        Some(result) => {
            if json {
                let mut payload = serde_json::to_value(&result)?;
                if let Some(summary) = transcript_summary {
                    payload["transcript_summary"] = serde_json::json!({
                        "event_count": summary.event_count,
                        "size_bytes": summary.size_bytes,
                        "first_timestamp": summary.first_timestamp,
                        "last_timestamp": summary.last_timestamp,
                    });
                }
                println!("{}", serde_json::to_string_pretty(&payload)?);
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
                if let Some(summary) = transcript_summary {
                    println!("Transcript:");
                    println!("  Events: {}", summary.event_count);
                    println!("  Size:   {} bytes", summary.size_bytes);
                    println!(
                        "  First:  {}",
                        summary.first_timestamp.as_deref().unwrap_or("-")
                    );
                    println!(
                        "  Last:   {}",
                        summary.last_timestamp.as_deref().unwrap_or("-")
                    );
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
    let resolved = resolve_session_prefix_with_fallback(&project_root, &session)?;
    let resolved_id = resolved.session_id;
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
    let resolved = resolve_session_prefix_with_fallback(&project_root, &session)?;
    let log = csa_session::git::history(&resolved.sessions_dir, &resolved.session_id)?;
    if log.is_empty() {
        eprintln!("No git history for session '{}'", resolved.session_id);
    } else {
        print!("{}", log);
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

#[cfg(test)]
#[path = "session_cmds_tests.rs"]
mod tests;
