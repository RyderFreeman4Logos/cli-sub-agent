use anyhow::{Context, Result};
use csa_config::paths;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::warn;

/// Get the session root directory for a project (`~/.local/state/cli-sub-agent/{project_path}`)
pub fn get_session_root(project_path: &Path) -> Result<PathBuf> {
    let state_dir = paths::state_dir_write().context("Failed to determine project directories")?;
    let normalized = normalize_project_path(project_path);
    Ok(state_dir.join(project_storage_key(&normalized)))
}

pub(super) fn legacy_session_root(project_path: &Path) -> Option<PathBuf> {
    let normalized = normalize_project_path(project_path);
    paths::legacy_state_dir().map(|state_dir| state_dir.join(project_storage_key(&normalized)))
}

fn session_roots_for_reads(project_path: &Path) -> Result<Vec<PathBuf>> {
    let normalized = normalize_project_path(project_path);
    let state_dir = paths::state_dir_write().context("Failed to determine project directories")?;
    let mut roots = Vec::new();

    push_unique_root(
        &mut roots,
        state_dir.join(project_storage_key_from_path(&normalized)),
    );
    if normalized.as_path() != project_path {
        push_unique_root(
            &mut roots,
            state_dir.join(project_storage_key_from_path(project_path)),
        );
    }

    if let Some(legacy_state_dir) = paths::legacy_state_dir() {
        push_unique_root(
            &mut roots,
            legacy_state_dir.join(project_storage_key_from_path(&normalized)),
        );
        if normalized.as_path() != project_path {
            push_unique_root(
                &mut roots,
                legacy_state_dir.join(project_storage_key_from_path(project_path)),
            );
        }
    }

    Ok(roots)
}

fn push_unique_root(roots: &mut Vec<PathBuf>, candidate: PathBuf) {
    if !roots.contains(&candidate) {
        roots.push(candidate);
    }
}

pub(super) fn normalize_project_path(project_path: &Path) -> PathBuf {
    fs::canonicalize(project_path).unwrap_or_else(|_| project_path.to_path_buf())
}

fn project_storage_key(project_path: &Path) -> String {
    project_storage_key_from_path(project_path)
}

pub(super) fn project_storage_key_from_path(project_path: &Path) -> String {
    project_path
        .to_string_lossy()
        .trim_start_matches('/')
        .replace('/', std::path::MAIN_SEPARATOR_STR)
}

fn session_state_exists(base_dir: &Path, session_id: &str) -> bool {
    get_session_dir_in(base_dir, session_id)
        .join(super::STATE_FILE_NAME)
        .exists()
}

fn find_session_base_dir_under(state_dir: &Path, session_id: &str) -> Result<Option<PathBuf>> {
    let mut stack = vec![state_dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if session_state_exists(&dir, session_id) {
            return Ok(Some(dir));
        }

        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("Failed to read state dir: {}", dir.display()));
            }
        };

        for entry in entries {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            if entry.file_name() == "sessions" {
                continue;
            }
            stack.push(entry.path());
        }
    }

    Ok(None)
}

fn find_session_base_dir_anywhere(session_id: &str) -> Result<Option<PathBuf>> {
    let primary_state_dir =
        paths::state_dir_write().context("Failed to determine project directories")?;
    if let Some(base_dir) = find_session_base_dir_under(&primary_state_dir, session_id)? {
        return Ok(Some(base_dir));
    }

    if let Some(legacy_state_dir) = paths::legacy_state_dir()
        && let Some(base_dir) = find_session_base_dir_under(&legacy_state_dir, session_id)?
    {
        return Ok(Some(base_dir));
    }

    Ok(None)
}

/// Verify that a globally-found session base dir belongs to the same physical
/// project (via canonical path comparison). Returns `None` if the session's
/// stored project_path resolves to a different canonical location.
fn validate_global_fallback_for_project(
    base_dir: &Path,
    session_id: &str,
    project_path: &Path,
) -> bool {
    let state_path = get_session_dir_in(base_dir, session_id).join(super::STATE_FILE_NAME);
    let Ok(content) = fs::read_to_string(&state_path) else {
        return true; // can't verify, allow fallback
    };
    // Extract project_path from state TOML without full deserialization
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("project_path") {
            let rest = rest.trim();
            if let Some(rest) = rest.strip_prefix('=') {
                let value = rest.trim().trim_matches('"');
                let session_canonical =
                    fs::canonicalize(value).unwrap_or_else(|_| PathBuf::from(value));
                let request_canonical = normalize_project_path(project_path);
                return session_canonical == request_canonical;
            }
        }
    }
    true // no project_path field found, allow fallback
}

pub(super) fn resolve_read_base_dir(
    project_path: &Path,
    session_id: Option<&str>,
) -> Result<PathBuf> {
    let roots = session_roots_for_reads(project_path)?;
    let primary = roots
        .first()
        .cloned()
        .context("Failed to determine project directories")?;

    match session_id {
        Some(session_id) => {
            if let Some(base_dir) = roots
                .into_iter()
                .find(|root| session_state_exists(root, session_id))
            {
                return Ok(base_dir);
            }

            if let Some(base_dir) = find_session_base_dir_anywhere(session_id)?
                && validate_global_fallback_for_project(&base_dir, session_id, project_path)
            {
                warn!(
                    session_id,
                    found_root = %base_dir.display(),
                    "session resolved via global fallback (symlink-equivalent path)"
                );
                return Ok(base_dir);
            }

            Ok(primary)
        }
        None => Ok(roots
            .into_iter()
            .find(|root| root.join("sessions").exists())
            .unwrap_or(primary)),
    }
}

pub(super) fn resolve_write_base_dir(project_path: &Path, session_id: &str) -> Result<PathBuf> {
    let primary = get_session_root(project_path)?;
    let roots = session_roots_for_reads(project_path)?;
    if let Some(base_dir) = roots
        .into_iter()
        .find(|root| session_state_exists(root, session_id))
    {
        return Ok(base_dir);
    }

    if let Some(base_dir) = find_session_base_dir_anywhere(session_id)?
        && validate_global_fallback_for_project(&base_dir, session_id, project_path)
    {
        warn!(
            session_id,
            found_root = %base_dir.display(),
            "session write resolved via global fallback (symlink-equivalent path)"
        );
        return Ok(base_dir);
    }

    Ok(primary)
}

/// Get the directory for a specific session
pub fn get_session_dir(project_path: &Path, session_id: &str) -> Result<PathBuf> {
    let primary_dir = get_session_root(project_path)?
        .join("sessions")
        .join(session_id);
    for root in session_roots_for_reads(project_path)? {
        let candidate = root.join("sessions").join(session_id);
        if candidate.exists() {
            return Ok(candidate);
        }
    }

    if let Some(base_dir) = find_session_base_dir_anywhere(session_id)?
        && validate_global_fallback_for_project(&base_dir, session_id, project_path)
    {
        let candidate = base_dir.join("sessions").join(session_id);
        if candidate.exists() {
            return Ok(candidate);
        }
    }

    Ok(primary_dir)
}

/// Internal function for testing: get session directory with explicit base
pub(super) fn get_session_dir_in(base_dir: &Path, session_id: &str) -> PathBuf {
    base_dir.join("sessions").join(session_id)
}
