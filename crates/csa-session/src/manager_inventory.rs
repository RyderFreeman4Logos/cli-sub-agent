use super::{STATE_FILE_NAME, list_all_project_session_roots, load_session_in};
use crate::{state::MetaSessionState, validate::validate_session_id};
use anyhow::{Context, Result};
use std::fs;
use std::io;
use std::path::Path;

/// Strict read-only inventory for safety-critical admission.
///
/// Missing state roots and absent state files are ignored; existing roots,
/// entries, and state files must be readable, and state files must be parseable.
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
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let Ok(session_id) = entry.file_name().into_string() else {
            continue;
        };
        if validate_session_id(&session_id).is_err() {
            continue;
        }
        let state_path = entry.path().join(STATE_FILE_NAME);
        match fs::symlink_metadata(&state_path) {
            Ok(_) => sessions.push(load_session_in(base_dir, &session_id)?),
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(err).with_context(|| {
                    format!("Failed to inspect state file: {}", state_path.display())
                });
            }
        }
    }
    Ok(sessions)
}
