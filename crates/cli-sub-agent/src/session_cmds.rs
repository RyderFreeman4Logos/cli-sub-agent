use anyhow::Result;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::Path;
use tracing::info;

use csa_core::types::OutputFormat;
use csa_session::{
    MetaSessionState, SessionPhase, SessionResult, delete_session, find_sessions, get_session_dir,
    list_artifacts, list_sessions, list_sessions_tree_filtered, load_result, load_session,
    resolve_session_prefix,
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
    find_sessions(project_root, branch, None, None, tool_filter)
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

pub(crate) fn handle_session_is_alive(session: String, cd: Option<String>) -> Result<bool> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let sessions_dir = csa_session::get_session_root(&project_root)?.join("sessions");
    let resolved_id = resolve_session_prefix(&sessions_dir, &session)?;
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
    let sessions_dir = csa_session::get_session_root(&project_root)?.join("sessions");
    let resolved_id = resolve_session_prefix(&sessions_dir, &session)?;
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
    use super::{
        select_sessions_for_list, session_to_json, status_from_phase_and_result,
        truncate_with_ellipsis,
    };
    use crate::cli::{Cli, Commands, SessionCommands};
    use chrono::Utc;
    use clap::Parser;
    use csa_session::{
        ContextStatus, Genealogy, MetaSessionState, SessionPhase, SessionResult, TaskContext,
        TokenUsage, create_session, delete_session, load_session, save_session,
    };
    use std::collections::HashMap;
    use tempfile::tempdir;

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

    fn make_result(status: &str, exit_code: i32) -> SessionResult {
        let now = Utc::now();
        SessionResult {
            status: status.to_string(),
            exit_code,
            summary: "summary".to_string(),
            tool: "codex".to_string(),
            started_at: now,
            completed_at: now,
            events_count: 0,
            artifacts: Vec::new(),
        }
    }

    #[test]
    fn session_status_uses_phase_when_no_result() {
        assert_eq!(
            status_from_phase_and_result(&SessionPhase::Active, None),
            "Active"
        );
        assert_eq!(
            status_from_phase_and_result(&SessionPhase::Available, None),
            "Available"
        );
    }

    #[test]
    fn session_status_marks_non_zero_as_failed() {
        let failure = make_result("failure", 1);
        let signal = make_result("signal", 137);
        let inconsistent_success = make_result("success", 2);

        assert_eq!(
            status_from_phase_and_result(&SessionPhase::Active, Some(&failure)),
            "Failed"
        );
        assert_eq!(
            status_from_phase_and_result(&SessionPhase::Active, Some(&signal)),
            "Failed"
        );
        assert_eq!(
            status_from_phase_and_result(&SessionPhase::Active, Some(&inconsistent_success)),
            "Failed"
        );
    }

    #[test]
    fn session_status_marks_unknown_result_as_error() {
        let unknown = make_result("mystery", 0);
        assert_eq!(
            status_from_phase_and_result(&SessionPhase::Active, Some(&unknown)),
            "Error"
        );
    }

    #[test]
    fn retired_phase_takes_precedence_over_failure_result() {
        let failure = make_result("failure", 1);
        assert_eq!(
            status_from_phase_and_result(&SessionPhase::Retired, Some(&failure)),
            "Retired"
        );
    }

    fn sample_session_state() -> MetaSessionState {
        let now = Utc::now();
        MetaSessionState {
            meta_session_id: "01J6F5W0M6Q7BW7Q3T0J4A8V45".to_string(),
            description: Some("Plan".to_string()),
            project_path: "/tmp/project".to_string(),
            branch: Some("feature/x".to_string()),
            created_at: now,
            last_accessed: now,
            genealogy: Genealogy {
                parent_session_id: None,
                depth: 0,
            },
            tools: HashMap::new(),
            context_status: ContextStatus::default(),
            total_token_usage: Some(TokenUsage {
                input_tokens: Some(10),
                output_tokens: Some(20),
                total_tokens: Some(30),
                estimated_cost_usd: None,
            }),
            phase: SessionPhase::Available,
            task_context: TaskContext {
                task_type: Some("plan".to_string()),
                tier_name: None,
            },
            turn_count: 0,
            token_budget: None,
            sandbox_info: None,
            termination_reason: None,
        }
    }

    #[test]
    fn session_to_json_includes_branch_and_task_type() {
        let session = sample_session_state();
        let value = session_to_json(std::path::Path::new("/tmp/project"), &session);
        assert_eq!(
            value.get("branch").and_then(|v| v.as_str()),
            Some("feature/x")
        );
        assert_eq!(
            value.get("task_type").and_then(|v| v.as_str()),
            Some("plan")
        );
    }

    #[test]
    fn session_list_branch_filter_returns_matching_sessions() {
        let td = tempdir().unwrap();
        let project = td.path();

        let s1 = create_session(project, Some("S1"), None, None).unwrap();
        let s2 = create_session(project, Some("S2"), None, None).unwrap();

        let mut session1 = load_session(project, &s1.meta_session_id).unwrap();
        session1.branch = Some("feature/x".to_string());
        save_session(&session1).unwrap();

        let mut session2 = load_session(project, &s2.meta_session_id).unwrap();
        session2.branch = Some("feature/y".to_string());
        save_session(&session2).unwrap();

        let filtered = select_sessions_for_list(project, Some("feature/x"), None).unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].meta_session_id, s1.meta_session_id);

        delete_session(project, &s1.meta_session_id).unwrap();
        delete_session(project, &s2.meta_session_id).unwrap();
    }

    #[test]
    fn session_list_cli_parses_branch_filter() {
        let cli = Cli::try_parse_from(["csa", "session", "list", "--branch", "feature/x"]).unwrap();
        match cli.command {
            Commands::Session {
                cmd: SessionCommands::List { branch, .. },
            } => assert_eq!(branch.as_deref(), Some("feature/x")),
            _ => panic!("expected session list command"),
        }
    }
}
