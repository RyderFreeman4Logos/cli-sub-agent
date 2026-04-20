//! Session CRUD operations

use crate::state::{MetaSessionState, SessionPhase};
use crate::validate::{new_session_id, resolve_session_prefix, validate_session_id};
use anyhow::{Context, Result, bail};
use chrono::Utc;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::process::Command;

#[path = "manager_audit.rs"]
mod manager_audit;
#[path = "manager_daemon.rs"]
mod manager_daemon;
#[path = "manager_legacy.rs"]
mod manager_legacy;
#[path = "manager_paths.rs"]
mod manager_paths;
#[path = "manager_result.rs"]
mod manager_result;

pub use manager_audit::{RepoWriteAudit, compute_repo_write_audit, write_audit_warning_artifact};
pub use manager_daemon::ResumeSessionResolution;
use manager_daemon::{SessionIdStrategy, preassigned_daemon_session_id};
pub use manager_legacy::decode_session_created_at;
#[cfg(test)]
use manager_paths::project_storage_key_from_path;
pub use manager_paths::{get_session_dir, get_session_root};
pub use manager_paths::{get_session_dir_global, list_all_project_session_roots};
use manager_paths::{get_session_dir_in, resolve_read_base_dir, resolve_write_base_dir};
use manager_paths::{legacy_session_root, normalize_project_path};
pub use manager_result::{
    CONTRACT_RESULT_ARTIFACT_PATH, LEGACY_USER_RESULT_ARTIFACT_PATH, RESULT_TOML_PATH_CONTRACT_ENV,
    SaveOptions, SessionResultView, clear_manager_sidecar, contract_result_path,
    legacy_user_result_path, list_artifacts, load_result, load_result_view,
    redact_result_sidecar_value, render_redacted_result_sidecar, save_result,
};
#[cfg(test)]
pub(crate) use manager_result::{
    list_artifacts_in, load_result_in, load_result_view_in, save_result_in,
};

const STATE_FILE_NAME: &str = "state.toml";
const DAEMON_SESSION_ID_ENV: &str = "CSA_DAEMON_SESSION_ID";
const DAEMON_SESSION_DIR_ENV: &str = "CSA_DAEMON_SESSION_DIR";
const DAEMON_PROJECT_ROOT_ENV: &str = "CSA_DAEMON_PROJECT_ROOT";

/// Create a new session. If `parent_id` is provided, this session is a child
/// of that parent with `depth = parent.depth + 1`. If `tool` is provided,
/// metadata.toml is created with tool ownership info.
pub fn create_session(
    project_path: &Path,
    description: Option<&str>,
    parent_id: Option<&str>,
    tool: Option<&str>,
) -> Result<MetaSessionState> {
    let base_dir = get_session_root(project_path)?;
    create_session_in_with_strategy(
        &base_dir,
        project_path,
        description,
        parent_id,
        tool,
        SessionIdStrategy::DaemonAware,
    )
}

/// Create a child session with a fresh ULID even when daemon session env vars
/// are present in the current process.
pub fn create_session_fresh(
    project_path: &Path,
    description: Option<&str>,
    parent_id: Option<&str>,
    tool: Option<&str>,
) -> Result<MetaSessionState> {
    let base_dir = get_session_root(project_path)?;
    create_session_in_with_strategy(
        &base_dir,
        project_path,
        description,
        parent_id,
        tool,
        SessionIdStrategy::Fresh,
    )
}

/// Internal implementation: create session in explicit base directory.
#[cfg(test)]
pub(crate) fn create_session_in(
    base_dir: &Path,
    project_path: &Path,
    description: Option<&str>,
    parent_id: Option<&str>,
    tool: Option<&str>,
) -> Result<MetaSessionState> {
    create_session_in_with_strategy(
        base_dir,
        project_path,
        description,
        parent_id,
        tool,
        SessionIdStrategy::DaemonAware,
    )
}

fn create_session_in_with_strategy(
    base_dir: &Path,
    project_path: &Path,
    description: Option<&str>,
    parent_id: Option<&str>,
    tool: Option<&str>,
    session_id_strategy: SessionIdStrategy,
) -> Result<MetaSessionState> {
    // Daemon child processes pre-assign a session ID via env so that the
    // pipeline session directory matches the daemon spool directory. Nested
    // child sessions must opt out of that binding so they do not collapse onto
    // the caller's own daemon session.
    let session_id = match session_id_strategy {
        SessionIdStrategy::DaemonAware => match preassigned_daemon_session_id() {
            Some(id) => {
                validate_session_id(&id)?;
                id
            }
            None => new_session_id(),
        },
        SessionIdStrategy::Fresh => new_session_id(),
    };
    let session_dir = get_session_dir_in(base_dir, &session_id);
    let normalized_project_path = normalize_project_path(project_path);

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
            runtime_binary: None,
        };
        let metadata_path = session_dir.join(crate::metadata::METADATA_FILE_NAME);
        let contents =
            toml::to_string_pretty(&metadata).context("Failed to serialize session metadata")?;
        fs::write(&metadata_path, contents)
            .with_context(|| format!("Failed to write metadata: {}", metadata_path.display()))?;
    }

    let now = Utc::now();

    // Unified VCS identity capture — single backend call replaces three
    let vcs_backend = crate::vcs_backends::create_vcs_backend(&normalized_project_path);
    let identity = vcs_backend.identity(&normalized_project_path).ok();

    // Populate legacy fields from identity for backward compatibility
    let branch = identity
        .as_ref()
        .and_then(|id| id.ref_name.clone())
        .or_else(|| detect_current_branch(&normalized_project_path));
    let git_head = identity
        .as_ref()
        .and_then(|id| id.commit_id.clone())
        .or_else(|| detect_git_head(&normalized_project_path));
    let pre_session_porcelain = detect_git_status_porcelain(&normalized_project_path);
    let change_id = identity
        .as_ref()
        .and_then(|id| {
            // For jj: use change_id; for git: use commit_id (matches legacy behavior)
            id.change_id.clone().or(id.commit_id.clone())
        })
        .or_else(|| detect_change_id(&normalized_project_path));

    let state = MetaSessionState {
        meta_session_id: session_id,
        description: description.map(|s| s.to_string()),
        project_path: normalized_project_path.to_string_lossy().to_string(),
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
        git_head_at_creation: git_head,
        pre_session_porcelain,
        last_return_packet: None,
        change_id,
        spec_id: None,
        vcs_identity: identity,
        identity_version: 2,
        fork_call_timestamps: Vec::new(),
    };

    // Write state file
    save_session_in(base_dir, &state)?;

    Ok(state)
}
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

fn detect_git_status_porcelain(project_path: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["status", "--porcelain=v1", "-z"])
        .current_dir(project_path)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    String::from_utf8(output.stdout).ok()
}

/// Detect a VCS change identifier for session-change binding using the active backend.
fn detect_change_id(project_path: &Path) -> Option<String> {
    let backend = crate::vcs_backends::create_vcs_backend(project_path);
    backend.head_id(project_path).ok().flatten()
}

fn detect_current_branch(project_path: &Path) -> Option<String> {
    let backend = crate::vcs_backends::create_vcs_backend(project_path);
    backend.current_branch(project_path).ok().flatten()
}

/// Load an existing session
pub fn load_session(project_path: &Path, session_id: &str) -> Result<MetaSessionState> {
    let base_dir = resolve_read_base_dir(project_path, Some(session_id))?;
    load_session_in(&base_dir, session_id)
}

/// Load a session via global exact ULID lookup (cross-project, read-only).
///
/// Returns `None` if no session with this exact ULID is found anywhere.
/// Unlike `load_session`, this bypasses project path validation.
pub fn load_session_global_exact(session_id: &str) -> Result<Option<MetaSessionState>> {
    use manager_paths::resolve_read_base_dir_global_exact;
    if let Some((base_dir, _)) = resolve_read_base_dir_global_exact(session_id)? {
        match load_session_in(&base_dir, session_id) {
            Ok(state) => return Ok(Some(state)),
            Err(_) => return Ok(None),
        }
    }
    Ok(None)
}

/// List sessions from all projects (for `session list --all-projects`).
///
/// Returns sessions from every project directory in the state dir,
/// with no project-scope filtering.
pub fn list_all_sessions_all_projects() -> Result<Vec<MetaSessionState>> {
    let roots = list_all_project_session_roots()?;
    let mut all_sessions = Vec::new();
    for (root, _key) in roots {
        match list_all_sessions_in_readonly(&root) {
            Ok(sessions) => all_sessions.extend(sessions),
            Err(err) => {
                tracing::debug!(
                    root = %root.display(),
                    error = %err,
                    "Skipping project root with unreadable sessions"
                );
            }
        }
    }
    all_sessions.sort_by_key(|session| std::cmp::Reverse(session.last_accessed));
    Ok(all_sessions)
}

/// Internal implementation: load session from explicit base directory
pub(crate) fn load_session_in(base_dir: &Path, session_id: &str) -> Result<MetaSessionState> {
    validate_session_id(session_id)?;

    let session_dir = get_session_dir_in(base_dir, session_id);
    let state_path = session_dir.join(STATE_FILE_NAME);

    if !state_path.exists() {
        bail!("Session '{session_id}' not found");
    }

    let contents = fs::read_to_string(&state_path)
        .with_context(|| format!("Failed to read state file: {}", state_path.display()))?;

    let state: MetaSessionState = match toml::from_str(&contents) {
        Ok(state) => state,
        Err(primary_err) => {
            manager_legacy::load_session_with_created_at_fallback(&contents, session_id)
                .with_context(|| format!("Failed to parse state file: {}", state_path.display()))
                .or_else(|_| {
                    Err(primary_err).with_context(|| {
                        format!("Failed to parse state file: {}", state_path.display())
                    })
                })?
        }
    };

    Ok(state)
}

/// Save session state to disk
pub fn save_session(state: &MetaSessionState) -> Result<()> {
    let project_path = Path::new(&state.project_path);
    let base_dir = resolve_write_base_dir(project_path, &state.meta_session_id)?;
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
        bail!("Session '{session_id}' not found");
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
                    pre_session_porcelain: None,
                    last_return_packet: None,
                    change_id: None,
                    spec_id: None,
                    fork_call_timestamps: Vec::new(),
                    vcs_identity: None,
                    identity_version: 1,
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
        let normalized_key = normalize_project_path(path).to_string_lossy().to_string();
        let raw_key = path.to_string_lossy().to_string();
        sessions.retain(|session| {
            session.project_path == normalized_key || session.project_path == raw_key
        });
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

    sessions.sort_by_key(|session| std::cmp::Reverse(session.last_accessed));
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
    let read_base = resolve_read_base_dir(project_path, Some(session_ref))?;
    match resolve_resume_session_in(&read_base, session_ref, tool) {
        Ok(resolution) => Ok(resolution),
        Err(read_error) => {
            let primary = get_session_root(project_path)?;
            if primary != read_base
                && let Ok(resolution) = resolve_resume_session_in(&primary, session_ref, tool)
            {
                return Ok(resolution);
            }
            let Some(legacy) = legacy_session_root(project_path) else {
                return Err(read_error);
            };
            if !legacy.join("sessions").exists() {
                return Err(read_error);
            }
            resolve_resume_session_in(&legacy, session_ref, tool).map_err(|_| read_error)
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

#[cfg(test)]
#[path = "manager_tests.rs"]
mod tests;
