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
