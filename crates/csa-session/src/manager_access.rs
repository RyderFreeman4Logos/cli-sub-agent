use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

use super::{
    ResumeSessionResolution, get_session_dir_in, get_session_root, legacy_session_root,
    load_session_in, resolve_read_base_dir,
};
use crate::validate::{resolve_session_prefix, validate_session_id};

/// Load session metadata (tool ownership info)
pub fn load_metadata(
    project_path: &Path,
    session_id: &str,
) -> Result<Option<crate::metadata::SessionMetadata>> {
    let base_dir = resolve_read_base_dir(project_path, Some(session_id))?;
    load_metadata_in(&base_dir, session_id)
}

/// Internal implementation: load metadata from explicit base directory
pub(crate) fn load_metadata_in(
    base_dir: &Path,
    session_id: &str,
) -> Result<Option<crate::metadata::SessionMetadata>> {
    validate_session_id(session_id)?;
    let session_dir = get_session_dir_in(base_dir, session_id);
    let metadata_path = session_dir.join(crate::metadata::METADATA_FILE_NAME);

    if !metadata_path.exists() {
        return Ok(None);
    }

    let contents = fs::read_to_string(&metadata_path)
        .with_context(|| format!("Failed to read metadata: {}", metadata_path.display()))?;
    let metadata: crate::metadata::SessionMetadata = toml::from_str(&contents)
        .with_context(|| format!("Failed to parse metadata: {}", metadata_path.display()))?;

    Ok(Some(metadata))
}

/// Resolve a session reference as a fork source without tool-lock enforcement.
///
/// Unlike [`super::resolve_resume_session`], this function does NOT check `tool_locked`
/// because soft forks only read context from the parent session and do not require
/// tool ownership. The returned `provider_session_id` is from the *source* tool
/// (not the fork target), which native forks may need.
pub fn resolve_fork_source(
    project_path: &Path,
    session_ref: &str,
) -> Result<ResumeSessionResolution> {
    let primary = get_session_root(project_path)?;
    match resolve_fork_source_in(&primary, session_ref) {
        Ok(resolution) => Ok(resolution),
        Err(primary_error) => {
            let Some(legacy) = legacy_session_root(project_path) else {
                return Err(primary_error);
            };
            if !legacy.join("sessions").exists() {
                return Err(primary_error);
            }
            resolve_fork_source_in(&legacy, session_ref).map_err(|_| primary_error)
        }
    }
}

/// Internal implementation: resolve fork source IDs without tool-lock check.
fn resolve_fork_source_in(base_dir: &Path, session_ref: &str) -> Result<ResumeSessionResolution> {
    let sessions_dir = base_dir.join("sessions");
    let meta_session_id = resolve_session_prefix(&sessions_dir, session_ref)?;

    // Load session to find the source tool's provider session ID (for native fork).
    // We take the first tool entry that has a provider_session_id.
    let session = load_session_in(base_dir, &meta_session_id)?;
    let provider_session_id = session
        .tools
        .values()
        .find_map(|state| state.provider_session_id.clone());

    Ok(ResumeSessionResolution {
        meta_session_id,
        provider_session_id,
    })
}

/// Validate that the given tool can access this session.
/// Returns Ok(()) if access is allowed, Err if tool_locked and tool doesn't match.
pub fn validate_tool_access(project_path: &Path, session_id: &str, tool: &str) -> Result<()> {
    let base_dir = resolve_read_base_dir(project_path, Some(session_id))?;
    validate_tool_access_in(&base_dir, session_id, tool)
}

/// Internal implementation: validate tool access in explicit base directory
pub(crate) fn validate_tool_access_in(base_dir: &Path, session_id: &str, tool: &str) -> Result<()> {
    if let Some(metadata) = load_metadata_in(base_dir, session_id)?
        && metadata.tool_locked
        && metadata.tool != tool
    {
        anyhow::bail!(
            "Session '{}' is locked to tool '{}', cannot access with '{}'",
            session_id,
            metadata.tool,
            tool
        );
    }
    Ok(())
}
