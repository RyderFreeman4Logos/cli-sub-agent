//! Session CRUD operations

use crate::result::{RESULT_FILE_NAME, SessionResult};
use crate::state::MetaSessionState;
use crate::validate::{new_session_id, resolve_session_prefix, validate_session_id};
use anyhow::{Context, Result, bail};
use chrono::Utc;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

const STATE_FILE_NAME: &str = "state.toml";

/// Resolved identifiers for resuming a tool session.
#[derive(Debug, Clone)]
pub struct ResumeSessionResolution {
    /// Fully resolved CSA meta session ID (ULID).
    pub meta_session_id: String,
    /// Provider-native session ID for the requested tool, if present in state.
    pub provider_session_id: Option<String>,
}

/// Get the session root directory for a project (`~/.local/state/csa/{project_path}`)
pub fn get_session_root(project_path: &Path) -> Result<PathBuf> {
    let proj_dirs = directories::ProjectDirs::from("", "", "csa")
        .context("Failed to determine project directories")?;

    // state_dir() is Linux-only; fall back to data_local_dir() on macOS/Windows.
    let state_dir = proj_dirs
        .state_dir()
        .unwrap_or_else(|| proj_dirs.data_local_dir());

    // Convert absolute project path to a relative-like key for storage
    // Strip leading "/" and replace remaining "/" with platform separator
    let project_key = project_path
        .to_string_lossy()
        .trim_start_matches('/')
        .replace('/', std::path::MAIN_SEPARATOR_STR);

    Ok(state_dir.join(project_key))
}

/// Get the directory for a specific session
pub fn get_session_dir(project_path: &Path, session_id: &str) -> Result<PathBuf> {
    let root = get_session_root(project_path)?;
    Ok(root.join("sessions").join(session_id))
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

    let state = MetaSessionState {
        meta_session_id: session_id,
        description: description.map(|s| s.to_string()),
        project_path: project_path.to_string_lossy().to_string(),
        created_at: now,
        last_accessed: now,
        genealogy: crate::state::Genealogy {
            parent_session_id,
            depth,
        },
        tools: HashMap::new(),
        context_status: Default::default(),
        total_token_usage: None,
        phase: Default::default(),
        task_context: Default::default(),
    };

    // Write state file
    save_session_in(base_dir, &state)?;

    Ok(state)
}

/// Load an existing session
pub fn load_session(project_path: &Path, session_id: &str) -> Result<MetaSessionState> {
    let base_dir = get_session_root(project_path)?;
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
    let base_dir = get_session_root(project_path)?;
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
    let base_dir = get_session_root(project_path)?;
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
                    created_at: now,
                    last_accessed: now,
                    genealogy: crate::state::Genealogy {
                        parent_session_id: None,
                        depth: 0,
                    },
                    tools: HashMap::new(),
                    context_status: Default::default(),
                    total_token_usage: None,
                    phase: Default::default(),
                    task_context: Default::default(),
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
    let base_dir = get_session_root(project_path)?;
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
    let base_dir = get_session_root(project_path)?;
    resolve_resume_session_in(&base_dir, session_ref, tool)
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
    let base_dir = get_session_root(project_path)?;
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
    let base_dir = get_session_root(project_path)?;
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

/// Validate that the given tool can access this session.
/// Returns Ok(()) if access is allowed, Err if tool_locked and tool doesn't match.
pub fn validate_tool_access(project_path: &Path, session_id: &str, tool: &str) -> Result<()> {
    let base_dir = get_session_root(project_path)?;
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
    let base_dir = get_session_root(project_path)?;
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
    let base_dir = get_session_root(project_path)?;
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
    artifacts.sort();
    Ok(artifacts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_create_session() {
        let td = tempdir().unwrap();
        let state =
            create_session_in(td.path(), td.path(), Some("Test session"), None, None).unwrap();
        assert_eq!(state.description, Some("Test session".to_string()));
        assert_eq!(state.genealogy.depth, 0);
        assert!(state.genealogy.parent_session_id.is_none());
        let dir = get_session_dir_in(td.path(), &state.meta_session_id);
        assert!(dir.exists());
        assert!(dir.join(STATE_FILE_NAME).exists());
        assert!(dir.join("input").is_dir());
        assert!(dir.join("output").is_dir());
    }

    #[test]
    fn test_load_session() {
        let td = tempdir().unwrap();
        let created = create_session_in(td.path(), td.path(), Some("Test"), None, None).unwrap();
        let loaded = load_session_in(td.path(), &created.meta_session_id).unwrap();
        assert_eq!(loaded.meta_session_id, created.meta_session_id);
        assert_eq!(loaded.description, created.description);
    }

    #[test]
    fn test_delete_session() {
        let td = tempdir().unwrap();
        let state = create_session_in(td.path(), td.path(), None, None, None).unwrap();
        let dir = get_session_dir_in(td.path(), &state.meta_session_id);
        assert!(dir.exists());
        delete_session_in(td.path(), &state.meta_session_id).unwrap();
        assert!(!dir.exists());
    }

    #[test]
    fn test_list_all_sessions() {
        let td = tempdir().unwrap();
        create_session_in(td.path(), td.path(), Some("S1"), None, None).unwrap();
        create_session_in(td.path(), td.path(), Some("S2"), None, None).unwrap();
        assert_eq!(list_all_sessions_in(td.path()).unwrap().len(), 2);
    }

    #[test]
    fn test_list_sessions_with_tool_filter() {
        let td = tempdir().unwrap();
        let mut s1 = create_session_in(td.path(), td.path(), Some("S1"), None, None).unwrap();
        s1.tools.insert(
            "codex".to_string(),
            crate::state::ToolState {
                provider_session_id: Some("thread_123".to_string()),
                last_action_summary: "Test".to_string(),
                last_exit_code: 0,
                updated_at: Utc::now(),
                token_usage: None,
            },
        );
        save_session_in(td.path(), &s1).unwrap();
        create_session_in(td.path(), td.path(), Some("S2"), None, None).unwrap();
        let filtered = list_sessions_in(td.path(), Some(&["codex"])).unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].meta_session_id, s1.meta_session_id);
    }

    #[test]
    fn test_resolve_resume_session_with_provider_id() {
        let td = tempdir().unwrap();
        let mut state =
            create_session_in(td.path(), td.path(), Some("Resume"), None, None).unwrap();
        state.tools.insert(
            "codex".to_string(),
            crate::state::ToolState {
                provider_session_id: Some("provider_session_123".to_string()),
                last_action_summary: "resume".to_string(),
                last_exit_code: 0,
                updated_at: Utc::now(),
                token_usage: None,
            },
        );
        save_session_in(td.path(), &state).unwrap();

        let prefix = &state.meta_session_id[..10];
        let resolved = resolve_resume_session_in(td.path(), prefix, "codex").unwrap();

        assert_eq!(resolved.meta_session_id, state.meta_session_id);
        assert_eq!(
            resolved.provider_session_id,
            Some("provider_session_123".to_string())
        );
    }

    #[test]
    fn test_resolve_resume_session_without_provider_id() {
        let td = tempdir().unwrap();
        let state = create_session_in(td.path(), td.path(), Some("Resume"), None, None).unwrap();

        let resolved =
            resolve_resume_session_in(td.path(), &state.meta_session_id, "codex").unwrap();
        assert_eq!(resolved.meta_session_id, state.meta_session_id);
        assert!(resolved.provider_session_id.is_none());
    }

    #[test]
    fn test_resolve_resume_session_respects_tool_lock() {
        let td = tempdir().unwrap();
        let state =
            create_session_in(td.path(), td.path(), Some("Locked"), None, Some("codex")).unwrap();

        let err =
            resolve_resume_session_in(td.path(), &state.meta_session_id, "gemini-cli").unwrap_err();
        assert!(err.to_string().contains("locked to tool"));
    }

    #[test]
    fn test_create_child_session() {
        let td = tempdir().unwrap();
        let parent = create_session_in(td.path(), td.path(), Some("Parent"), None, None).unwrap();
        let child = create_session_in(
            td.path(),
            td.path(),
            Some("Child"),
            Some(&parent.meta_session_id),
            None,
        )
        .unwrap();
        assert_eq!(
            child.genealogy.parent_session_id,
            Some(parent.meta_session_id.clone())
        );
        assert_eq!(child.genealogy.depth, 1);
    }

    #[test]
    fn test_round_trip() {
        let td = tempdir().unwrap();
        let created =
            create_session_in(td.path(), td.path(), Some("Round trip"), None, None).unwrap();
        let loaded = load_session_in(td.path(), &created.meta_session_id).unwrap();
        assert_eq!(loaded.meta_session_id, created.meta_session_id);
        assert_eq!(loaded.description, created.description);
        assert_eq!(loaded.project_path, created.project_path);
        assert_eq!(loaded.genealogy.depth, created.genealogy.depth);
    }

    #[test]
    fn test_create_session_with_tool() {
        let td = tempdir().unwrap();
        let state =
            create_session_in(td.path(), td.path(), Some("Test"), None, Some("codex")).unwrap();
        let dir = get_session_dir_in(td.path(), &state.meta_session_id);
        assert!(dir.join("metadata.toml").exists());
        let meta = load_metadata_in(td.path(), &state.meta_session_id)
            .unwrap()
            .unwrap();
        assert_eq!(meta.tool, "codex");
        assert!(meta.tool_locked);
    }

    include!("manager_tests_tail.rs");
}
