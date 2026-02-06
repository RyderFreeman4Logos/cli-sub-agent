//! Session CRUD operations

use crate::state::MetaSessionState;
use crate::validate::{new_session_id, validate_session_id};
use anyhow::{bail, Context, Result};
use chrono::Utc;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

const STATE_FILE_NAME: &str = "state.toml";

/// Get the session root directory for a project
///
/// Uses XDG state directory: `~/.local/state/csa/{project_path}`
pub fn get_session_root(project_path: &Path) -> Result<PathBuf> {
    let proj_dirs = directories::ProjectDirs::from("", "", "csa")
        .context("Failed to determine project directories")?;

    let state_dir = proj_dirs
        .state_dir()
        .context("Failed to determine state directory")?;

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
pub fn create_session(
    project_path: &Path,
    description: Option<&str>,
    parent_id: Option<&str>,
) -> Result<MetaSessionState> {
    let base_dir = get_session_root(project_path)?;
    create_session_in(&base_dir, project_path, description, parent_id)
}

/// Internal implementation: create session in explicit base directory
pub(crate) fn create_session_in(
    base_dir: &Path,
    project_path: &Path,
    description: Option<&str>,
    parent_id: Option<&str>,
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

    // Create session directory
    fs::create_dir_all(&session_dir).with_context(|| {
        format!(
            "Failed to create session directory: {}",
            session_dir.display()
        )
    })?;

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

/// Internal implementation: save session in explicit base directory
pub(crate) fn save_session_in(base_dir: &Path, state: &MetaSessionState) -> Result<()> {
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

/// Internal implementation: list all sessions from explicit base directory
pub(crate) fn list_all_sessions_in(base_dir: &Path) -> Result<Vec<MetaSessionState>> {
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

        // Skip non-directory entries
        if !entry.file_type()?.is_dir() {
            continue;
        }

        // Try to load the session
        match load_session_in(base_dir, &session_id) {
            Ok(state) => {
                sessions.push(state);
            }
            Err(e) => {
                // BUG-11: Corrupt state.toml recovery
                let session_dir = get_session_dir_in(base_dir, &session_id);
                let state_path = session_dir.join(STATE_FILE_NAME);

                if state_path.exists() {
                    // Backup corrupt file
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
                        "Recovered corrupt state.toml, backed up to state.toml.corrupt"
                    );

                    // Create minimal valid state
                    let minimal_state = MetaSessionState {
                        meta_session_id: session_id.clone(),
                        description: Some("(recovered from corrupt state)".to_string()),
                        project_path: "(unknown)".to_string(),
                        created_at: chrono::Utc::now(),
                        last_accessed: chrono::Utc::now(),
                        genealogy: crate::state::Genealogy {
                            parent_session_id: None,
                            depth: 0,
                        },
                        tools: std::collections::HashMap::new(),
                        context_status: Default::default(),
                    };

                    // Save minimal state
                    if let Err(save_err) = save_session_in(base_dir, &minimal_state) {
                        tracing::warn!(
                            session_id = %session_id,
                            error = %save_err,
                            "Failed to save minimal state after recovery"
                        );
                        continue;
                    }

                    sessions.push(minimal_state);
                } else {
                    // No state.toml file at all - will be handled as orphan by GC
                    tracing::warn!(
                        session_id = %session_id,
                        "Session directory exists but has no state.toml"
                    );
                }
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

/// Update the last_accessed timestamp and save
pub fn update_last_accessed(state: &mut MetaSessionState) -> Result<()> {
    state.last_accessed = Utc::now();
    save_session(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_create_session() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let project_path = temp_dir.path();

        let state = create_session_in(temp_dir.path(), project_path, Some("Test session"), None)
            .expect("Failed to create session");

        assert_eq!(state.description, Some("Test session".to_string()));
        assert_eq!(state.genealogy.depth, 0);
        assert!(state.genealogy.parent_session_id.is_none());

        let session_dir = get_session_dir_in(temp_dir.path(), &state.meta_session_id);
        assert!(session_dir.exists());
        assert!(session_dir.join(STATE_FILE_NAME).exists());
    }

    #[test]
    fn test_load_session() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let project_path = temp_dir.path();

        let created = create_session_in(temp_dir.path(), project_path, Some("Test"), None)
            .expect("Failed to create session");

        let loaded = load_session_in(temp_dir.path(), &created.meta_session_id)
            .expect("Failed to load session");

        assert_eq!(loaded.meta_session_id, created.meta_session_id);
        assert_eq!(loaded.description, created.description);
    }

    #[test]
    fn test_delete_session() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let project_path = temp_dir.path();

        let state = create_session_in(temp_dir.path(), project_path, None, None)
            .expect("Failed to create session");

        let session_dir = get_session_dir_in(temp_dir.path(), &state.meta_session_id);
        assert!(session_dir.exists());

        delete_session_in(temp_dir.path(), &state.meta_session_id)
            .expect("Failed to delete session");

        assert!(!session_dir.exists());
    }

    #[test]
    fn test_list_all_sessions() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let project_path = temp_dir.path();

        create_session_in(temp_dir.path(), project_path, Some("Session 1"), None)
            .expect("Failed to create session 1");
        create_session_in(temp_dir.path(), project_path, Some("Session 2"), None)
            .expect("Failed to create session 2");

        let sessions = list_all_sessions_in(temp_dir.path()).expect("Failed to list sessions");

        assert_eq!(sessions.len(), 2);
    }

    #[test]
    fn test_list_sessions_with_tool_filter() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let project_path = temp_dir.path();

        let mut state1 = create_session_in(temp_dir.path(), project_path, Some("Session 1"), None)
            .expect("Failed to create session 1");

        state1.tools.insert(
            "codex".to_string(),
            crate::state::ToolState {
                provider_session_id: Some("thread_123".to_string()),
                last_action_summary: "Test".to_string(),
                last_exit_code: 0,
                updated_at: Utc::now(),
            },
        );
        save_session_in(temp_dir.path(), &state1).expect("Failed to save state1");

        create_session_in(temp_dir.path(), project_path, Some("Session 2"), None)
            .expect("Failed to create session 2");

        let filtered = list_sessions_in(temp_dir.path(), Some(&["codex"]))
            .expect("Failed to list filtered sessions");

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].meta_session_id, state1.meta_session_id);
    }

    #[test]
    fn test_create_child_session() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let project_path = temp_dir.path();

        let parent = create_session_in(temp_dir.path(), project_path, Some("Parent"), None)
            .expect("Failed to create parent");

        let child = create_session_in(
            temp_dir.path(),
            project_path,
            Some("Child"),
            Some(&parent.meta_session_id),
        )
        .expect("Failed to create child");

        assert_eq!(
            child.genealogy.parent_session_id,
            Some(parent.meta_session_id.clone())
        );
        assert_eq!(child.genealogy.depth, 1);
    }

    #[test]
    fn test_round_trip() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let project_path = temp_dir.path();

        let created =
            create_session_in(temp_dir.path(), project_path, Some("Round trip test"), None)
                .expect("Failed to create session");

        let loaded = load_session_in(temp_dir.path(), &created.meta_session_id)
            .expect("Failed to load session");

        assert_eq!(loaded.meta_session_id, created.meta_session_id);
        assert_eq!(loaded.description, created.description);
        assert_eq!(loaded.project_path, created.project_path);
        assert_eq!(loaded.genealogy.depth, created.genealogy.depth);
    }
}
