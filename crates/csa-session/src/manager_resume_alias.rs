use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::validate::validate_session_id;

pub const RESUME_TARGET_FILE_NAME: &str = "resume-target.toml";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ResumeTarget {
    kind: String,
    target_session_id: String,
    created_at: DateTime<Utc>,
}

/// Resolved resume-wrapper target identity and on-disk session directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeTargetResolution {
    /// CSA meta session ID of the worker session that the wrapper follows.
    pub session_id: String,
    /// Directory that contains the worker session's `state.toml`.
    pub session_dir: PathBuf,
}

pub fn write_resume_target(
    project_path: &Path,
    wrapper_session_id: &str,
    target_session_id: &str,
) -> Result<()> {
    validate_session_id(wrapper_session_id)?;
    validate_session_id(target_session_id)?;
    if wrapper_session_id == target_session_id {
        return Ok(());
    }

    let base_dir = super::resolve_write_base_dir(project_path, wrapper_session_id)?;
    let wrapper_dir = super::get_session_dir_in(&base_dir, wrapper_session_id);
    resolve_existing_resume_target(project_path, target_session_id)?;

    let target = ResumeTarget {
        kind: "resume-wrapper".to_string(),
        target_session_id: target_session_id.to_string(),
        created_at: Utc::now(),
    };
    let contents = toml::to_string_pretty(&target).context("failed to serialize resume target")?;
    write_file_atomically(&wrapper_dir.join(RESUME_TARGET_FILE_NAME), &contents)
}

pub fn read_resume_target_from_dir(session_dir: &Path) -> Result<Option<String>> {
    let path = session_dir.join(RESUME_TARGET_FILE_NAME);
    if !path.exists() {
        return Ok(None);
    }
    if !path.is_file() {
        bail!("resume target path is not a file: {}", path.display());
    }

    let contents = fs::read_to_string(&path)
        .with_context(|| format!("failed to read resume target: {}", path.display()))?;
    let target: ResumeTarget = toml::from_str(&contents)
        .with_context(|| format!("failed to parse resume target: {}", path.display()))?;
    if target.kind != "resume-wrapper" {
        bail!("unsupported resume target kind '{}'", target.kind);
    }
    validate_session_id(&target.target_session_id)?;
    Ok(Some(target.target_session_id))
}

/// Resolve a wrapper's resume target through all supported project session roots.
///
/// The alias file stores only the target session ID. This function re-resolves
/// that ID instead of assuming the wrapper and worker live in the same root, so
/// legacy/global same-project session roots remain valid.
pub fn resolve_resume_target_from_dir(
    project_path: &Path,
    wrapper_session_dir: &Path,
) -> Result<Option<ResumeTargetResolution>> {
    let Some(target_session_id) = read_resume_target_from_dir(wrapper_session_dir)? else {
        return Ok(None);
    };
    let session_dir = resolve_existing_resume_target(project_path, &target_session_id)
        .with_context(|| {
            format!(
                "failed to resolve resume target session '{target_session_id}' from {}",
                wrapper_session_dir.display()
            )
        })?;
    Ok(Some(ResumeTargetResolution {
        session_id: target_session_id,
        session_dir,
    }))
}

fn resolve_existing_resume_target(project_path: &Path, target_session_id: &str) -> Result<PathBuf> {
    let target_dir = super::get_session_dir(project_path, target_session_id)?;
    if target_dir.join(super::STATE_FILE_NAME).is_file() {
        return Ok(target_dir);
    }
    bail!("resume target session '{target_session_id}' not found in supported session roots");
}

fn write_file_atomically(path: &Path, contents: &str) -> Result<()> {
    let Some(parent_dir) = path.parent() else {
        bail!("path has no parent for atomic write: {}", path.display());
    };
    fs::create_dir_all(parent_dir)
        .with_context(|| format!("failed to create parent dir: {}", parent_dir.display()))?;
    let mut temp_file = tempfile::NamedTempFile::new_in(parent_dir)
        .with_context(|| format!("failed to create temp file in {}", parent_dir.display()))?;
    temp_file
        .write_all(contents.as_bytes())
        .with_context(|| format!("failed to write temp file for {}", path.display()))?;
    temp_file
        .flush()
        .with_context(|| format!("failed to flush temp file for {}", path.display()))?;
    temp_file
        .persist(path)
        .map_err(|err| err.error)
        .with_context(|| format!("failed to atomically replace {}", path.display()))?;
    Ok(())
}
