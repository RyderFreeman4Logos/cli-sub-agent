use super::{list_all_project_session_roots, load_session_in};
use crate::state::MetaSessionState;
use anyhow::{Context, Result};
use std::fs;
use std::io;
use std::path::Path;

/// Strict read-only inventory for safety-critical admission.
///
/// Missing state roots are empty; existing roots, entries, and session states
/// must all be readable so callers never admit from a partial inventory.
pub fn list_all_sessions_all_projects_strict() -> Result<Vec<MetaSessionState>> {
    let roots = list_all_project_session_roots()?;
    let mut all_sessions = Vec::new();
    for (root, _) in roots {
        all_sessions.extend(list_sessions_in_strict(&root)?);
    }
    all_sessions.sort_by_key(|session| std::cmp::Reverse(session.last_accessed));
    Ok(all_sessions)
}

fn list_sessions_in_strict(base_dir: &Path) -> Result<Vec<MetaSessionState>> {
    let sessions_dir = base_dir.join("sessions");
    let entries = match fs::read_dir(&sessions_dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => {
            return Err(err).with_context(|| {
                format!(
                    "Failed to read sessions directory: {}",
                    sessions_dir.display()
                )
            });
        }
    };

    let mut sessions = Vec::new();
    for entry in entries {
        let entry = entry.context("Failed to read directory entry")?;
        let session_id = entry.file_name().to_string_lossy().to_string();
        if !entry.file_type()?.is_dir()
            || session_id.starts_with('.')
            || matches!(
                session_id.as_str(),
                "run-pre-session-preflight" | "review-pre-session-preflight"
            )
        {
            continue;
        }
        sessions.push(load_session_in(base_dir, &session_id)?);
    }
    Ok(sessions)
}
