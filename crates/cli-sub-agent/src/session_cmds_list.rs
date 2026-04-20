use anyhow::Result;
use chrono::{DateTime, Duration, Local, Utc};
use std::path::Path;

#[cfg(test)]
use csa_session::decode_session_created_at;
use csa_session::{MetaSessionState, SessionPhase, SessionResult, list_sessions, load_result};

use super::{ensure_terminal_result_for_dead_active_session, retire_if_dead_with_result};

pub(super) fn truncate_with_ellipsis(input: &str, max_chars: usize) -> String {
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

pub(super) fn session_created_at(session: &MetaSessionState) -> DateTime<Utc> {
    session
        .created_at
        .with_timezone(&Utc)
        .min(session.last_accessed)
}

pub(super) fn format_started_at(created_at: DateTime<Utc>) -> String {
    created_at
        .with_timezone(&Local)
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

pub(super) fn format_compact_duration(duration: Duration) -> String {
    let secs = duration.num_seconds().max(0);
    let days = secs / 86_400;
    let hours = (secs % 86_400) / 3_600;
    let minutes = (secs % 3_600) / 60;
    let seconds = secs % 60;

    if days > 0 {
        if hours > 0 {
            format!("{days}d{hours}h")
        } else {
            format!("{days}d")
        }
    } else if hours > 0 {
        if minutes > 0 {
            format!("{hours}h{minutes}m")
        } else {
            format!("{hours}h")
        }
    } else if minutes > 0 {
        format!("{minutes}m")
    } else {
        format!("{seconds}s")
    }
}

pub(super) fn format_elapsed(
    session: &MetaSessionState,
    resolved_status: &str,
    now: DateTime<Utc>,
) -> String {
    let created_at = session_created_at(session);
    let end = if resolved_status == "Active" {
        now
    } else {
        session.last_accessed.max(created_at)
    };
    format_compact_duration(end - created_at)
}

#[cfg(test)]
pub(super) fn decode_ulid_created_at(session_id: &str) -> Result<DateTime<Utc>> {
    decode_session_created_at(session_id)
}

pub(super) fn phase_label(phase: &SessionPhase) -> &'static str {
    match phase {
        SessionPhase::Active => "Active",
        SessionPhase::Available => "Available",
        SessionPhase::Retired => "Retired",
    }
}

pub(super) fn status_from_phase_and_result(
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

pub(super) fn resolve_session_status(session: &MetaSessionState) -> String {
    // Use the session's own project_path so cross-project sessions resolve correctly.
    let project_root = Path::new(&session.project_path);
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
            if matches!(reconciled, Ok(outcome) if outcome.result_became_available())
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

pub(super) fn select_sessions_for_list(
    project_root: &Path,
    branch: Option<&str>,
    tool_filter: Option<&[&str]>,
) -> Result<Vec<MetaSessionState>> {
    let mut sessions = list_sessions(project_root, tool_filter)?;

    if let Some(branch_filter) = branch {
        sessions.retain(|session| session.branch.as_deref() == Some(branch_filter));
    }

    sessions.sort_by_key(|session| std::cmp::Reverse(session.last_accessed));
    Ok(sessions)
}

pub(super) fn select_sessions_for_list_all_projects(
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

    sessions.sort_by_key(|session| std::cmp::Reverse(session.last_accessed));
    Ok(sessions)
}

pub(super) fn session_to_json(session: &MetaSessionState) -> serde_json::Value {
    let status = resolve_session_status(session);
    let created_at = session_created_at(session);
    let now = Utc::now();
    let mut value = serde_json::json!({
        "session_id": session.meta_session_id,
        "started_at": created_at,
        "last_accessed": session.last_accessed,
        "elapsed": format_elapsed(session, &status, now),
        "description": session.description.as_deref().unwrap_or(""),
        "tools": session.tools.keys().collect::<Vec<_>>(),
        "status": status,
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
