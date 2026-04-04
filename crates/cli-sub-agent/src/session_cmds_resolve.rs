use anyhow::Result;
use csa_session::checkpoint::CheckpointNote;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use csa_config::paths;
use csa_session::resolve_session_prefix;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionPrefixResolution {
    pub session_id: String,
    pub sessions_dir: PathBuf,
    /// When the session was resolved via global cross-project fallback, this
    /// contains the foreign project's root path. Callers can use this instead
    /// of the local `project_root` to call project-scoped session APIs.
    pub foreign_project_root: Option<PathBuf>,
}

pub(crate) fn resolve_session_prefix_with_fallback(
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

pub(crate) fn legacy_sessions_dir_from_primary_root(primary_root: &Path) -> Option<PathBuf> {
    let primary_state_dir = paths::state_dir_write()?;
    let legacy_state_dir = paths::legacy_state_dir()?;
    let relative_root = primary_root.strip_prefix(primary_state_dir).ok()?;
    let legacy_root = legacy_state_dir.join(relative_root);
    (legacy_root != primary_root).then(|| legacy_root.join("sessions"))
}

pub(super) fn list_checkpoints_from_dirs(
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

pub(crate) fn resolve_session_prefix_from_dirs(
    prefix: &str,
    primary_sessions_dir: &Path,
    legacy_sessions_dir: Option<&Path>,
) -> Result<SessionPrefixResolution> {
    match resolve_session_prefix(primary_sessions_dir, prefix) {
        Ok(session_id) => Ok(SessionPrefixResolution {
            session_id,
            sessions_dir: primary_sessions_dir.to_path_buf(),
            foreign_project_root: None,
        }),
        Err(primary_err) if should_fallback_to_legacy(&primary_err) => {
            let Some(legacy_sessions_dir) = legacy_sessions_dir else {
                return Err(primary_err);
            };

            match resolve_session_prefix(legacy_sessions_dir, prefix) {
                Ok(session_id) => Ok(SessionPrefixResolution {
                    session_id,
                    sessions_dir: legacy_sessions_dir.to_path_buf(),
                    foreign_project_root: None,
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

/// Resolve a session by first trying project-scoped prefix lookup, then
/// falling back to global exact ULID lookup across all projects (read-only).
///
/// When the global fallback finds a match, a warning is emitted to stderr
/// and the resolution includes the foreign project's session directory.
pub(crate) fn resolve_session_prefix_with_global_fallback(
    project_root: &std::path::Path,
    prefix: &str,
) -> Result<SessionPrefixResolution> {
    // First try the normal project-scoped resolution.
    match resolve_session_prefix_with_fallback(project_root, prefix) {
        Ok(resolution) => Ok(resolution),
        Err(project_err) => {
            // Only attempt global fallback for full 26-char ULIDs
            if prefix.len() != 26 {
                return Err(project_err);
            }
            // Validate it's actually a ULID
            if csa_session::validate_session_id(prefix).is_err() {
                return Err(project_err);
            }
            // Try global exact lookup
            if let Some(session_dir) = csa_session::get_session_dir_global(prefix)? {
                let sessions_dir = session_dir
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| session_dir.clone());
                // Extract the foreign project_path from state.toml for callers
                // that need to call project-scoped session APIs.
                let foreign_project_root = extract_foreign_project_root(&session_dir);
                eprintln!(
                    "Warning: session {} not found in current project, using cross-project fallback",
                    prefix,
                );
                return Ok(SessionPrefixResolution {
                    session_id: prefix.to_string(),
                    sessions_dir,
                    foreign_project_root,
                });
            }
            Err(project_err)
        }
    }
}

/// Read `state.toml` from a session directory and extract the `project_path`
/// field. Returns `None` if the file is missing or the field cannot be parsed.
fn extract_foreign_project_root(session_dir: &Path) -> Option<PathBuf> {
    let state_path = session_dir.join("state.toml");
    let content = std::fs::read_to_string(&state_path).ok()?;
    // Lightweight line-based extraction — avoids full TOML deserialization.
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("project_path") {
            let rest = rest.trim();
            if let Some(rest) = rest.strip_prefix('=') {
                let value = rest.trim().trim_matches('"');
                if !value.is_empty() {
                    return Some(PathBuf::from(value));
                }
            }
        }
    }
    None
}
