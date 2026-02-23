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
    MetaSessionState, SessionPhase, SessionResult, delete_session, get_session_dir, list_sessions,
    list_sessions_tree_filtered, load_result, load_session, resolve_session_prefix,
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
    value
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

                    // Fork indicator appended after tokens
                    let fork_suffix =
                        if let Some(ref fork_of) = session.genealogy.fork_of_session_id {
                            let short_fork = &fork_of[..11.min(fork_of.len())];
                            format!("  \u{21B1} fork of {}", short_fork)
                        } else {
                            String::new()
                        };

                    println!(
                        "{:<11}  {:<19}  {:<10}  {:<25}  {:<20}  {:<18}  {}{}",
                        short_id,
                        session.last_accessed.format("%Y-%m-%d %H:%M"),
                        status_str,
                        desc_display,
                        tools_str,
                        branch_str,
                        tokens_str,
                        fork_suffix,
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
    events: bool,
    cd: Option<String>,
) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let resolved = resolve_session_prefix_with_fallback(&project_root, &session)?;
    let resolved_id = resolved.session_id;
    let session_dir = get_session_dir(&project_root, &resolved_id)?;

    if events {
        return display_acp_events(&session_dir, &resolved_id, tail);
    }

    // Try logs/ directory first (Legacy transport)
    if display_log_files(&session_dir, &resolved_id, tail)? {
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

    eprintln!("No logs found for session {}", resolved_id);
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
        eprintln!("=== {} ===", file_name);

        let content = fs::read_to_string(&path)?;
        print_content_with_tail(&content, tail);
        println!();
    }

    Ok(true)
}

/// Display ACP JSONL events from output/acp-events.jsonl.
fn display_acp_events(session_dir: &Path, session_id: &str, tail: Option<usize>) -> Result<()> {
    let events_path = session_dir.join("output").join("acp-events.jsonl");
    if !events_path.is_file() {
        eprintln!(
            "No ACP events found for session {} (no output/acp-events.jsonl)",
            session_id
        );
        return Ok(());
    }

    let content = fs::read_to_string(&events_path)?;
    if content.is_empty() {
        eprintln!("ACP events file is empty for session {}", session_id);
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
            println!("{}", line);
        }
    } else {
        print!("{}", content);
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

/// Options for structured output display in `csa session result`.
#[derive(Debug, Default)]
pub(crate) struct StructuredOutputOpts {
    pub summary: bool,
    pub section: Option<String>,
    pub full: bool,
}

impl StructuredOutputOpts {
    fn is_active(&self) -> bool {
        self.summary || self.section.is_some() || self.full
    }
}

pub(crate) fn handle_session_result(
    session: String,
    json: bool,
    cd: Option<String>,
    structured: StructuredOutputOpts,
) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let resolved = resolve_session_prefix_with_fallback(&project_root, &session)?;
    let resolved_id = resolved.session_id;
    let session_dir = get_session_dir(&project_root, &resolved_id)?;

    // If structured output flags are active, handle them and return early
    if structured.is_active() {
        return display_structured_output(&session_dir, &resolved_id, &structured, json);
    }

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

const FALLBACK_LINES: usize = 20;

/// Display structured output sections based on the requested mode.
fn display_structured_output(
    session_dir: &Path,
    session_id: &str,
    opts: &StructuredOutputOpts,
    json: bool,
) -> Result<()> {
    if opts.summary {
        return display_summary_section(session_dir, session_id, json);
    }

    if let Some(ref section_id) = opts.section {
        return display_single_section(session_dir, session_id, section_id, json);
    }

    if opts.full {
        return display_all_sections(session_dir, session_id, json);
    }

    Ok(())
}

/// Show only the summary section, with fallback to first N lines of output.log.
fn display_summary_section(session_dir: &Path, session_id: &str, json: bool) -> Result<()> {
    // Try reading "summary" section first
    let (section_id, content) = match csa_session::read_section(session_dir, "summary")? {
        Some(content) => ("summary", content),
        None => {
            // If there's a "full" section, use that as fallback
            match csa_session::read_section(session_dir, "full")? {
                Some(content) => ("full", content),
                None => {
                    // Final fallback: first N lines of output.log
                    let output_log = session_dir.join("output.log");
                    if output_log.is_file() {
                        let content = fs::read_to_string(&output_log)?;
                        if !content.is_empty() {
                            if json {
                                let payload = serde_json::json!({
                                    "section": "summary",
                                    "source": "output.log",
                                    "content": content.lines().take(FALLBACK_LINES).collect::<Vec<_>>().join("\n"),
                                    "truncated": content.lines().count() > FALLBACK_LINES,
                                });
                                println!("{}", serde_json::to_string_pretty(&payload)?);
                            } else {
                                let lines: Vec<&str> =
                                    content.lines().take(FALLBACK_LINES).collect();
                                println!("{}", lines.join("\n"));
                                if content.lines().count() > FALLBACK_LINES {
                                    eprintln!(
                                        "... ({} more lines, use --full to see all)",
                                        content.lines().count() - FALLBACK_LINES
                                    );
                                }
                            }
                            return Ok(());
                        }
                    }
                    eprintln!("No output found for session '{}'", session_id);
                    return Ok(());
                }
            }
        }
    };

    if json {
        let payload = serde_json::json!({
            "section": section_id,
            "content": content,
            "tokens": csa_session::estimate_tokens(&content),
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        let is_full_fallback = section_id == "full";
        if is_full_fallback {
            let lines: Vec<&str> = content.lines().take(FALLBACK_LINES).collect();
            println!("{}", lines.join("\n"));
            if content.lines().count() > FALLBACK_LINES {
                eprintln!(
                    "... ({} more lines, use --full to see all)",
                    content.lines().count() - FALLBACK_LINES
                );
            }
        } else {
            println!("{}", content);
        }
    }
    Ok(())
}

/// Show a single section by ID.
fn display_single_section(
    session_dir: &Path,
    session_id: &str,
    section_id: &str,
    json: bool,
) -> Result<()> {
    match csa_session::read_section(session_dir, section_id)? {
        Some(content) => {
            if json {
                let payload = serde_json::json!({
                    "section": section_id,
                    "content": content,
                    "tokens": csa_session::estimate_tokens(&content),
                });
                println!("{}", serde_json::to_string_pretty(&payload)?);
            } else {
                println!("{}", content);
            }
        }
        None => {
            // Check if index exists to give a better error
            match csa_session::load_output_index(session_dir)? {
                Some(index) => {
                    let available: Vec<&str> =
                        index.sections.iter().map(|s| s.id.as_str()).collect();
                    anyhow::bail!(
                        "Section '{}' not found in session '{}'. Available sections: {}",
                        section_id,
                        session_id,
                        available.join(", ")
                    );
                }
                None => {
                    anyhow::bail!(
                        "No structured output for session '{}'. Run without --section to see raw result.",
                        session_id
                    );
                }
            }
        }
    }
    Ok(())
}

/// Show all sections in index order.
fn display_all_sections(session_dir: &Path, session_id: &str, json: bool) -> Result<()> {
    let sections = csa_session::read_all_sections(session_dir)?;
    if sections.is_empty() {
        // Fallback: show full output.log
        let output_log = session_dir.join("output.log");
        if output_log.is_file() {
            let content = fs::read_to_string(&output_log)?;
            if !content.is_empty() {
                if json {
                    let payload = serde_json::json!({
                        "sections": [{
                            "section": "full",
                            "content": content,
                            "tokens": csa_session::estimate_tokens(&content),
                        }]
                    });
                    println!("{}", serde_json::to_string_pretty(&payload)?);
                } else {
                    print!("{}", content);
                }
                return Ok(());
            }
        }
        eprintln!("No output found for session '{}'", session_id);
        return Ok(());
    }

    if json {
        let json_sections: Vec<serde_json::Value> = sections
            .iter()
            .map(|(section, content)| {
                serde_json::json!({
                    "section": section.id,
                    "title": section.title,
                    "content": content,
                    "tokens": section.token_estimate,
                })
            })
            .collect();
        let payload = serde_json::json!({ "sections": json_sections });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        for (i, (section, content)) in sections.iter().enumerate() {
            if i > 0 {
                println!();
            }
            println!("=== {} ({}) ===", section.title, section.id);
            println!("{}", content);
        }
    }
    Ok(())
}

pub(crate) fn handle_session_artifacts(session: String, cd: Option<String>) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let resolved = resolve_session_prefix_with_fallback(&project_root, &session)?;
    let resolved_id = resolved.session_id;
    let session_dir = get_session_dir(&project_root, &resolved_id)?;
    let output_dir = session_dir.join("output");

    // Show structured output index if available
    if let Some(index) = csa_session::load_output_index(&session_dir)? {
        println!(
            "Structured output ({} sections, ~{} tokens):",
            index.sections.len(),
            index.total_tokens
        );
        for section in &index.sections {
            let size_str = if let Some(ref fp) = section.file_path {
                let path = output_dir.join(fp);
                match fs::metadata(&path) {
                    Ok(meta) => format_file_size(meta.len()),
                    Err(_) => "missing".to_string(),
                }
            } else {
                "-".to_string()
            };
            println!(
                "  {:<20}  {:<30}  ~{}tok  {}",
                section.id, section.title, section.token_estimate, size_str
            );
        }
        println!();
    }

    // List all files in output/ with sizes
    if output_dir.is_dir() {
        let mut entries: Vec<_> = fs::read_dir(&output_dir)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_file())
            .collect();
        entries.sort_by_key(|e| e.file_name());

        if entries.is_empty() {
            eprintln!("No artifacts for session '{}'", resolved_id);
        } else {
            println!("Files:");
            for entry in &entries {
                let path = entry.path();
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                println!("  {:<40}  {}", name, format_file_size(size));
            }
        }
    } else {
        eprintln!("No artifacts for session '{}'", resolved_id);
    }

    Ok(())
}

fn format_file_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
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

/// Token savings measurement for structured output.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct TokenMeasurement {
    pub session_id: String,
    pub total_tokens: usize,
    pub summary_tokens: usize,
    pub savings_tokens: usize,
    pub savings_percent: f64,
    pub section_count: usize,
    pub section_names: Vec<String>,
    pub is_structured: bool,
}

pub(crate) fn handle_session_measure(
    session: String,
    json: bool,
    cd: Option<String>,
) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let resolved = resolve_session_prefix_with_fallback(&project_root, &session)?;
    let resolved_id = resolved.session_id;
    let session_dir = get_session_dir(&project_root, &resolved_id)?;

    let measurement = compute_token_measurement(&session_dir, &resolved_id)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&measurement)?);
    } else {
        let short_id = &resolved_id[..11.min(resolved_id.len())];
        println!("Session: {}", short_id);
        println!(
            "Total output: {} tokens",
            format_number(measurement.total_tokens)
        );
        println!(
            "Summary only: {} tokens",
            format_number(measurement.summary_tokens)
        );
        if measurement.is_structured && measurement.total_tokens > 0 {
            println!(
                "Savings: {:.1}% ({} tokens saved)",
                measurement.savings_percent,
                format_number(measurement.savings_tokens)
            );
            println!(
                "Sections: {} ({})",
                measurement.section_count,
                measurement.section_names.join(", ")
            );
        } else {
            println!("Savings: N/A (unstructured output)");
        }
    }

    Ok(())
}

fn compute_token_measurement(session_dir: &Path, session_id: &str) -> Result<TokenMeasurement> {
    // Try loading the structured output index
    let index = csa_session::load_output_index(session_dir)?;

    if let Some(index) = index {
        let total_tokens = index.total_tokens;
        let section_names: Vec<String> = index.sections.iter().map(|s| s.id.clone()).collect();
        let section_count = index.sections.len();

        // Find summary section tokens (first section named "summary", or first section)
        let summary_tokens = index
            .sections
            .iter()
            .find(|s| s.id == "summary")
            .map(|s| s.token_estimate)
            .unwrap_or_else(|| {
                index
                    .sections
                    .first()
                    .map(|s| s.token_estimate)
                    .unwrap_or(0)
            });

        // "full" section means unstructured (parser wraps entire output as "full")
        let is_structured = section_count > 1 || (section_count == 1 && section_names[0] != "full");

        let savings_tokens = total_tokens.saturating_sub(summary_tokens);
        let savings_percent = if total_tokens > 0 {
            (1.0 - summary_tokens as f64 / total_tokens as f64) * 100.0
        } else {
            0.0
        };

        Ok(TokenMeasurement {
            session_id: session_id.to_string(),
            total_tokens,
            summary_tokens,
            savings_tokens,
            savings_percent,
            section_count,
            section_names,
            is_structured,
        })
    } else {
        // No index — try computing from output.log directly
        let output_log = session_dir.join("output.log");
        let total_tokens = if output_log.is_file() {
            let content = fs::read_to_string(&output_log)?;
            csa_session::estimate_tokens(&content)
        } else {
            0
        };

        Ok(TokenMeasurement {
            session_id: session_id.to_string(),
            total_tokens,
            summary_tokens: total_tokens,
            savings_tokens: 0,
            savings_percent: 0.0,
            section_count: 0,
            section_names: vec![],
            is_structured: false,
        })
    }
}

/// Format a number with commas for readability.
fn format_number(n: usize) -> String {
    let s = n.to_string();
    let chars: Vec<char> = s.chars().rev().collect();
    let chunks: Vec<String> = chars
        .chunks(3)
        .map(|chunk| chunk.iter().collect::<String>())
        .collect();
    chunks.join(",").chars().rev().collect()
}

#[cfg(test)]
#[path = "session_cmds_tests.rs"]
mod tests;
