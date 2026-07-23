//! Detached debate initialization state-directory preflight.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Prepare the state directories a detached debate needs before the parent
/// releases control of the child. This turns host path conflicts into a
/// caller-visible diagnostic instead of a later bare `ENOTDIR` in daemon logs.
pub(super) fn prepare_detached_debate_initialization(project_root: &Path) -> Result<()> {
    let state_dir = csa_config::paths::state_dir_write()
        .context("detached debate initialization could not resolve the CSA state directory")?;
    ensure_detached_debate_directory(&state_dir, "CSA state directory")?;

    let session_root = csa_session::get_session_root(project_root)
        .context("detached debate initialization could not resolve the CSA session root")?;
    ensure_detached_debate_directory(&session_root, "CSA project session directory")?;
    ensure_detached_debate_directory(&session_root.join("sessions"), "CSA sessions directory")
}

fn ensure_detached_debate_directory(path: &Path, label: &str) -> Result<()> {
    let conflict = first_non_directory_component(path)
        .map_err(|error| detached_debate_directory_error(path, label, error))?;
    if let Some(conflict) = conflict {
        return detached_debate_directory_conflict(path, label, &conflict);
    }

    std::fs::create_dir_all(path)
        .map_err(|error| detached_debate_directory_error(path, label, error))
}

fn detached_debate_directory_conflict(path: &Path, label: &str, conflict: &Path) -> Result<()> {
    anyhow::bail!(
        "detached debate initialization requires {label} '{}', but conflicting path '{}' is not a directory. \
         Move or replace that path, then retry the debate.",
        path.display(),
        conflict.display(),
    )
}

fn detached_debate_directory_error(
    path: &Path,
    label: &str,
    error: std::io::Error,
) -> anyhow::Error {
    if error.kind() == std::io::ErrorKind::NotADirectory {
        let conflict = first_non_directory_component(path)
            .ok()
            .flatten()
            .unwrap_or_else(|| path.to_path_buf());
        return anyhow::anyhow!(
            "detached debate initialization could not create {label} '{}'; conflicting path '{}' is not a directory: {error}. \
             Move or replace that path, then retry the debate.",
            path.display(),
            conflict.display(),
        );
    }

    anyhow::Error::new(error).context(format!(
        "detached debate initialization could not create {label} '{}'",
        path.display()
    ))
}

fn first_non_directory_component(path: &Path) -> std::io::Result<Option<PathBuf>> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        match std::fs::metadata(&current) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => return Ok(Some(current)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        }
    }
    Ok(None)
}
