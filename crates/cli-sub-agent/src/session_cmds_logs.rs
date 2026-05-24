use anyhow::Result;
use csa_session::SessionResult;
use std::fs;
use std::path::Path;

use super::{
    ensure_terminal_result_for_dead_active_session, resolve_session_prefix_with_global_fallback,
};

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
pub(crate) fn display_log_files(
    session_dir: &Path,
    session_id: &str,
    tail: Option<usize>,
) -> Result<bool> {
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

pub(crate) fn display_daemon_spool_logs(session_dir: &Path, tail: Option<usize>) -> Result<bool> {
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
pub(crate) fn display_acp_events(
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
pub(crate) fn print_content_with_tail(content: &str, tail: Option<usize>) {
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
