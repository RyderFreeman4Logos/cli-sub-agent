//! Session CRUD operations

use crate::result::{RESULT_FILE_NAME, SessionResult};
use crate::state::{MetaSessionState, SessionPhase};
use crate::validate::{new_session_id, resolve_session_prefix, validate_session_id};
use anyhow::{Context, Result, bail};
use chrono::Utc;
use csa_config::paths;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const STATE_FILE_NAME: &str = "state.toml";
const TRANSCRIPT_FILE_NAME: &str = "acp-events.jsonl";

/// Resolved identifiers for resuming a tool session.
#[derive(Debug, Clone)]
pub struct ResumeSessionResolution {
    /// Fully resolved CSA meta session ID (ULID).
    pub meta_session_id: String,
    /// Provider-native session ID for the requested tool, if present in state.
    pub provider_session_id: Option<String>,
}

/// Get the session root directory for a project (`~/.local/state/cli-sub-agent/{project_path}`)
pub fn get_session_root(project_path: &Path) -> Result<PathBuf> {
    let state_dir = paths::state_dir_write().context("Failed to determine project directories")?;
    Ok(state_dir.join(project_storage_key(project_path)))
}

fn legacy_session_root(project_path: &Path) -> Option<PathBuf> {
    paths::legacy_state_dir().map(|state_dir| state_dir.join(project_storage_key(project_path)))
}

fn project_storage_key(project_path: &Path) -> String {
    project_path
        .to_string_lossy()
        .trim_start_matches('/')
        .replace('/', std::path::MAIN_SEPARATOR_STR)
}

fn session_state_exists(base_dir: &Path, session_id: &str) -> bool {
    get_session_dir_in(base_dir, session_id)
        .join(STATE_FILE_NAME)
        .exists()
}

fn resolve_read_base_dir(project_path: &Path, session_id: Option<&str>) -> Result<PathBuf> {
    let primary = get_session_root(project_path)?;
    let Some(legacy) = legacy_session_root(project_path) else {
        return Ok(primary);
    };

    match session_id {
        Some(session_id) => {
            if session_state_exists(&primary, session_id)
                || !session_state_exists(&legacy, session_id)
            {
                Ok(primary)
            } else {
                Ok(legacy)
            }
        }
        None => {
            if primary.join("sessions").exists() || !legacy.join("sessions").exists() {
                Ok(primary)
            } else {
                Ok(legacy)
            }
        }
    }
}

/// Get the directory for a specific session
pub fn get_session_dir(project_path: &Path, session_id: &str) -> Result<PathBuf> {
    let primary_root = get_session_root(project_path)?;
    let primary_dir = primary_root.join("sessions").join(session_id);
    if primary_dir.exists() {
        return Ok(primary_dir);
    }
    if let Some(legacy_root) = legacy_session_root(project_path) {
        let legacy_dir = legacy_root.join("sessions").join(session_id);
        if legacy_dir.exists() {
            return Ok(legacy_dir);
        }
    }
    Ok(primary_dir)
}

/// Internal function for testing: get session directory with explicit base
fn get_session_dir_in(base_dir: &Path, session_id: &str) -> PathBuf {
    base_dir.join("sessions").join(session_id)
}

/// Create a new session
///
/// If `parent_id` is provided, this session will be a child of that parent.
/// Depth is computed from the parent (parent.depth + 1).
/// If `tool` is provided, metadata.toml is created with tool ownership info.
pub fn create_session(
    project_path: &Path,
    description: Option<&str>,
    parent_id: Option<&str>,
    tool: Option<&str>,
) -> Result<MetaSessionState> {
    let base_dir = get_session_root(project_path)?;
    create_session_in(&base_dir, project_path, description, parent_id, tool)
}

/// Internal implementation: create session in explicit base directory
pub(crate) fn create_session_in(
    base_dir: &Path,
    project_path: &Path,
    description: Option<&str>,
    parent_id: Option<&str>,
    tool: Option<&str>,
) -> Result<MetaSessionState> {
    let session_id = new_session_id();
    let session_dir = get_session_dir_in(base_dir, &session_id);

    // Compute depth from parent
    let (parent_session_id, depth) = if let Some(pid) = parent_id {
        validate_session_id(pid)?;
        let parent_state = load_session_in(base_dir, pid)?;
        (Some(pid.to_string()), parent_state.genealogy.depth + 1)
    } else {
        (None, 0)
    };

    // Ensure sessions dir is a git repo (before creating session dir to avoid orphans on failure)
    let sessions_dir = base_dir.join("sessions");
    crate::git::ensure_git_init(&sessions_dir)?;

    // Create session directory
    fs::create_dir_all(&session_dir).with_context(|| {
        format!(
            "Failed to create session directory: {}",
            session_dir.display()
        )
    })?;

    // Create input/ and output/ subdirectories
    fs::create_dir_all(session_dir.join("input"))?;
    fs::create_dir_all(session_dir.join("output"))?;

    // Write metadata.toml if tool is specified
    if let Some(tool_name) = tool {
        let metadata = crate::metadata::SessionMetadata {
            tool: tool_name.to_string(),
            tool_locked: true,
        };
        let metadata_path = session_dir.join(crate::metadata::METADATA_FILE_NAME);
        let contents =
            toml::to_string_pretty(&metadata).context("Failed to serialize session metadata")?;
        fs::write(&metadata_path, contents)
            .with_context(|| format!("Failed to write metadata: {}", metadata_path.display()))?;
    }

    let now = Utc::now();
    let branch = detect_current_branch(project_path);

    let state = MetaSessionState {
        meta_session_id: session_id,
        description: description.map(|s| s.to_string()),
        project_path: project_path.to_string_lossy().to_string(),
        branch,
        created_at: now,
        last_accessed: now,
        genealogy: crate::state::Genealogy {
            parent_session_id,
            depth,
            ..Default::default()
        },
        tools: HashMap::new(),
        context_status: Default::default(),
        total_token_usage: None,
        phase: Default::default(),
        task_context: Default::default(),
        turn_count: 0,
        token_budget: None,
        sandbox_info: None,
        termination_reason: None,
        is_seed_candidate: false,
        git_head_at_creation: detect_git_head(project_path),
    };

    // Write state file
    save_session_in(base_dir, &state)?;

    Ok(state)
}

/// Detect the current git HEAD commit hash (full SHA) for seed invalidation.
pub fn detect_git_head(project_path: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(project_path)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let head = String::from_utf8(output.stdout).ok()?;
    let trimmed = head.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn detect_current_branch(project_path: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(project_path)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let branch = String::from_utf8(output.stdout).ok()?;
    let trimmed = branch.trim();
    if trimmed.is_empty() || trimmed == "HEAD" {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Load an existing session
pub fn load_session(project_path: &Path, session_id: &str) -> Result<MetaSessionState> {
    let base_dir = resolve_read_base_dir(project_path, Some(session_id))?;
    load_session_in(&base_dir, session_id)
}

/// Internal implementation: load session from explicit base directory
pub(crate) fn load_session_in(base_dir: &Path, session_id: &str) -> Result<MetaSessionState> {
    validate_session_id(session_id)?;

    let session_dir = get_session_dir_in(base_dir, session_id);
    let state_path = session_dir.join(STATE_FILE_NAME);

    if !state_path.exists() {
        bail!("Session '{}' not found", session_id);
    }

    let contents = fs::read_to_string(&state_path)
        .with_context(|| format!("Failed to read state file: {}", state_path.display()))?;

    let state: MetaSessionState = toml::from_str(&contents)
        .with_context(|| format!("Failed to parse state file: {}", state_path.display()))?;

    Ok(state)
}

/// Save session state to disk
pub fn save_session(state: &MetaSessionState) -> Result<()> {
    let project_path = Path::new(&state.project_path);
    let base_dir = get_session_root(project_path)?;
    save_session_in(&base_dir, state)
}

///// Save session state to an explicit base directory.
pub fn save_session_in(base_dir: &Path, state: &MetaSessionState) -> Result<()> {
    let session_dir = get_session_dir_in(base_dir, &state.meta_session_id);
    let state_path = session_dir.join(STATE_FILE_NAME);

    let contents = toml::to_string_pretty(state).context("Failed to serialize session state")?;

    fs::write(&state_path, contents)
        .with_context(|| format!("Failed to write state file: {}", state_path.display()))?;

    Ok(())
}

/// Delete a session and its directory
pub fn delete_session(project_path: &Path, session_id: &str) -> Result<()> {
    let base_dir = resolve_read_base_dir(project_path, Some(session_id))?;
    delete_session_in(&base_dir, session_id)
}

/// Internal implementation: delete session from explicit base directory
pub(crate) fn delete_session_in(base_dir: &Path, session_id: &str) -> Result<()> {
    validate_session_id(session_id)?;

    let session_dir = get_session_dir_in(base_dir, session_id);

    if !session_dir.exists() {
        bail!("Session '{}' not found", session_id);
    }

    fs::remove_dir_all(&session_dir).with_context(|| {
        format!(
            "Failed to remove session directory: {}",
            session_dir.display()
        )
    })?;

    Ok(())
}

/// List all sessions for a project
pub fn list_all_sessions(project_path: &Path) -> Result<Vec<MetaSessionState>> {
    let base_dir = resolve_read_base_dir(project_path, None)?;
    list_all_sessions_in(&base_dir)
}

/// List all sessions from an explicit session root directory (for global GC).
pub fn list_sessions_from_root(session_root: &Path) -> Result<Vec<MetaSessionState>> {
    list_all_sessions_in(session_root)
}

/// Read-only variant of `list_sessions_from_root` (skips corrupt-state recovery).
pub fn list_sessions_from_root_readonly(session_root: &Path) -> Result<Vec<MetaSessionState>> {
    list_all_sessions_in_readonly(session_root)
}

/// Delete a session from an explicit session root directory (for global GC).
pub fn delete_session_from_root(session_root: &Path, session_id: &str) -> Result<()> {
    delete_session_in(session_root, session_id)
}

/// List sessions with corrupt-state recovery (BUG-11).
pub(crate) fn list_all_sessions_in(base_dir: &Path) -> Result<Vec<MetaSessionState>> {
    list_all_sessions_impl(base_dir, true)
}

/// List sessions without writes (for dry-run GC). Corrupt sessions are skipped.
pub(crate) fn list_all_sessions_in_readonly(base_dir: &Path) -> Result<Vec<MetaSessionState>> {
    list_all_sessions_impl(base_dir, false)
}

fn list_all_sessions_impl(base_dir: &Path, recover: bool) -> Result<Vec<MetaSessionState>> {
    let sessions_dir = base_dir.join("sessions");

    if !sessions_dir.exists() {
        return Ok(Vec::new());
    }

    let mut sessions = Vec::new();

    let entries = fs::read_dir(&sessions_dir).with_context(|| {
        format!(
            "Failed to read sessions directory: {}",
            sessions_dir.display()
        )
    })?;

    for entry in entries {
        let entry = entry.context("Failed to read directory entry")?;
        let session_id = entry.file_name().to_string_lossy().to_string();

        if !entry.file_type()?.is_dir() || session_id.starts_with('.') {
            continue;
        }

        match load_session_in(base_dir, &session_id) {
            Ok(state) => sessions.push(state),
            Err(e) if !recover => {
                tracing::debug!(
                    session_id = %session_id,
                    error = %e,
                    "Skipping session with unreadable state (readonly mode)"
                );
            }
            Err(e) => {
                // BUG-11: Corrupt state.toml recovery
                let session_dir = get_session_dir_in(base_dir, &session_id);
                let state_path = session_dir.join(STATE_FILE_NAME);
                if !state_path.exists() {
                    tracing::warn!(session_id = %session_id, "No state.toml");
                    continue;
                }
                let backup_path = session_dir.join("state.toml.corrupt");
                if let Err(backup_err) = fs::rename(&state_path, &backup_path) {
                    tracing::warn!(
                        session_id = %session_id,
                        error = %backup_err,
                        "Failed to backup corrupt state.toml"
                    );
                    continue;
                }
                tracing::warn!(
                    session_id = %session_id,
                    error = %e,
                    "Recovered corrupt state.toml → state.toml.corrupt"
                );
                let now = chrono::Utc::now();
                let minimal_state = MetaSessionState {
                    meta_session_id: session_id.clone(),
                    description: Some("(recovered from corrupt state)".to_string()),
                    project_path: "(unknown)".to_string(),
                    branch: None,
                    created_at: now,
                    last_accessed: now,
                    genealogy: crate::state::Genealogy::default(),
                    tools: HashMap::new(),
                    context_status: Default::default(),
                    total_token_usage: None,
                    phase: Default::default(),
                    task_context: Default::default(),
                    turn_count: 0,
                    token_budget: None,
                    sandbox_info: None,
                    termination_reason: None,
                    is_seed_candidate: false,
                    git_head_at_creation: None,
                };
                if let Err(save_err) = save_session_in(base_dir, &minimal_state) {
                    tracing::warn!(
                        session_id = %session_id,
                        error = %save_err,
                        "Failed to save minimal state after recovery"
                    );
                    continue;
                }
                sessions.push(minimal_state);
            }
        }
    }

    Ok(sessions)
}

/// List sessions, optionally filtered by tool presence
///
/// If `tool_filter` is Some, only return sessions that have state for at least one of the specified tools.
pub fn list_sessions(
    project_path: &Path,
    tool_filter: Option<&[&str]>,
) -> Result<Vec<MetaSessionState>> {
    let base_dir = resolve_read_base_dir(project_path, None)?;
    list_sessions_in(&base_dir, tool_filter)
}

/// Internal implementation: list sessions with optional filter
pub(crate) fn list_sessions_in(
    base_dir: &Path,
    tool_filter: Option<&[&str]>,
) -> Result<Vec<MetaSessionState>> {
    let all_sessions = list_all_sessions_in(base_dir)?;

    if let Some(tools) = tool_filter {
        Ok(all_sessions
            .into_iter()
            .filter(|session| tools.iter().any(|tool| session.tools.contains_key(*tool)))
            .collect())
    } else {
        Ok(all_sessions)
    }
}

/// Find sessions by multiple optional filters.
pub fn find_sessions(
    project_path: &Path,
    branch: Option<&str>,
    task_type: Option<&str>,
    phase: Option<SessionPhase>,
    tool_filter: Option<&[&str]>,
) -> Result<Vec<MetaSessionState>> {
    let base_dir = resolve_read_base_dir(project_path, None)?;
    find_sessions_in(
        &base_dir,
        Some(project_path),
        branch,
        task_type,
        phase,
        tool_filter,
    )
}

/// Internal implementation of [`find_sessions`] for tests.
pub(crate) fn find_sessions_in(
    base_dir: &Path,
    project_path: Option<&Path>,
    branch: Option<&str>,
    task_type: Option<&str>,
    phase: Option<SessionPhase>,
    tool_filter: Option<&[&str]>,
) -> Result<Vec<MetaSessionState>> {
    let mut sessions = list_all_sessions_in(base_dir)?;

    if let Some(path) = project_path {
        let project_key = path.to_string_lossy();
        sessions.retain(|session| session.project_path == project_key);
    }

    if let Some(branch_filter) = branch {
        sessions.retain(|session| session.branch.as_deref() == Some(branch_filter));
    }

    if let Some(task_type_filter) = task_type {
        sessions
            .retain(|session| session.task_context.task_type.as_deref() == Some(task_type_filter));
    }

    if let Some(phase_filter) = phase {
        sessions.retain(|session| session.phase == phase_filter);
    }

    if let Some(tools) = tool_filter {
        sessions.retain(|session| tools.iter().any(|tool| session.tools.contains_key(*tool)));
    }

    sessions.sort_by(|a, b| b.last_accessed.cmp(&a.last_accessed));
    sessions.truncate(10);
    Ok(sessions)
}

/// Resolve a user-provided session reference for resume.
///
/// This function accepts a full ULID or unique prefix, validates tool ownership,
/// and returns both CSA meta session ID and provider session ID (if present)
/// from `state.toml`.
pub fn resolve_resume_session(
    project_path: &Path,
    session_ref: &str,
    tool: &str,
) -> Result<ResumeSessionResolution> {
    let primary = get_session_root(project_path)?;
    match resolve_resume_session_in(&primary, session_ref, tool) {
        Ok(resolution) => Ok(resolution),
        Err(primary_error) => {
            let Some(legacy) = legacy_session_root(project_path) else {
                return Err(primary_error);
            };
            if !legacy.join("sessions").exists() {
                return Err(primary_error);
            }
            resolve_resume_session_in(&legacy, session_ref, tool).map_err(|_| primary_error)
        }
    }
}

/// Internal implementation: resolve resume IDs from explicit base directory.
pub(crate) fn resolve_resume_session_in(
    base_dir: &Path,
    session_ref: &str,
    tool: &str,
) -> Result<ResumeSessionResolution> {
    let sessions_dir = base_dir.join("sessions");
    let meta_session_id = resolve_session_prefix(&sessions_dir, session_ref)?;

    validate_tool_access_in(base_dir, &meta_session_id, tool)?;

    let session = load_session_in(base_dir, &meta_session_id)?;
    let provider_session_id = session
        .tools
        .get(tool)
        .and_then(|state| state.provider_session_id.clone());

    Ok(ResumeSessionResolution {
        meta_session_id,
        provider_session_id,
    })
}

/// Update the last_accessed timestamp and save
pub fn update_last_accessed(state: &mut MetaSessionState) -> Result<()> {
    state.last_accessed = Utc::now();
    save_session(state)
}

/// Mark a session as complete and commit its state to git.
/// Returns the short commit hash.
pub fn complete_session(project_path: &Path, session_id: &str, message: &str) -> Result<String> {
    let base_dir = resolve_read_base_dir(project_path, Some(session_id))?;
    complete_session_in(&base_dir, session_id, message)
}

/// Internal implementation: complete session in explicit base directory
pub(crate) fn complete_session_in(
    base_dir: &Path,
    session_id: &str,
    message: &str,
) -> Result<String> {
    validate_session_id(session_id)?;
    let sessions_dir = base_dir.join("sessions");
    crate::git::commit_session(&sessions_dir, session_id, message)
}

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
/// Unlike [`resolve_resume_session`], this function does NOT check `tool_locked`
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
    if let Some(metadata) = load_metadata_in(base_dir, session_id)? {
        if metadata.tool_locked && metadata.tool != tool {
            anyhow::bail!(
                "Session '{}' is locked to tool '{}', cannot access with '{}'",
                session_id,
                metadata.tool,
                tool
            );
        }
    }
    Ok(())
}

/// Write a session result to disk
pub fn save_result(project_path: &Path, session_id: &str, result: &SessionResult) -> Result<()> {
    let base_dir = get_session_root(project_path)?;
    save_result_in(&base_dir, session_id, result)
}

pub(crate) fn save_result_in(
    base_dir: &Path,
    session_id: &str,
    result: &SessionResult,
) -> Result<()> {
    validate_session_id(session_id)?;
    let session_dir = get_session_dir_in(base_dir, session_id);
    let result_path = session_dir.join(RESULT_FILE_NAME);
    let contents = toml::to_string_pretty(result).context("Failed to serialize session result")?;
    fs::write(&result_path, contents)
        .with_context(|| format!("Failed to write result: {}", result_path.display()))?;
    Ok(())
}

/// Load a session result
pub fn load_result(project_path: &Path, session_id: &str) -> Result<Option<SessionResult>> {
    let base_dir = resolve_read_base_dir(project_path, Some(session_id))?;
    load_result_in(&base_dir, session_id)
}

pub(crate) fn load_result_in(base_dir: &Path, session_id: &str) -> Result<Option<SessionResult>> {
    validate_session_id(session_id)?;
    let session_dir = get_session_dir_in(base_dir, session_id);
    let result_path = session_dir.join(RESULT_FILE_NAME);
    if !result_path.exists() {
        return Ok(None);
    }
    let contents = fs::read_to_string(&result_path)
        .with_context(|| format!("Failed to read result: {}", result_path.display()))?;
    let result: SessionResult = toml::from_str(&contents)
        .with_context(|| format!("Failed to parse result: {}", result_path.display()))?;
    Ok(Some(result))
}

/// List artifacts in a session's output/ directory
pub fn list_artifacts(project_path: &Path, session_id: &str) -> Result<Vec<String>> {
    let base_dir = resolve_read_base_dir(project_path, Some(session_id))?;
    list_artifacts_in(&base_dir, session_id)
}

pub(crate) fn list_artifacts_in(base_dir: &Path, session_id: &str) -> Result<Vec<String>> {
    validate_session_id(session_id)?;
    let session_dir = get_session_dir_in(base_dir, session_id);
    let output_dir = session_dir.join("output");
    if !output_dir.exists() {
        return Ok(Vec::new());
    }
    let mut artifacts = Vec::new();
    for entry in fs::read_dir(&output_dir)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            artifacts.push(entry.file_name().to_string_lossy().to_string());
        }
    }
    let transcript_path = output_dir.join(TRANSCRIPT_FILE_NAME);
    if transcript_path.is_file() && !artifacts.iter().any(|name| name == TRANSCRIPT_FILE_NAME) {
        artifacts.push(TRANSCRIPT_FILE_NAME.to_string());
    }
    artifacts.sort();
    Ok(artifacts)
}

#[cfg(test)]
#[path = "manager_tests.rs"]
mod tests;
