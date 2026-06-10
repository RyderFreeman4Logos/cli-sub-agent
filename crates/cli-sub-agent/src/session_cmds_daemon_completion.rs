//! Daemon completion packet handling and terminal result synthesis.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use csa_session::{MetaSessionState, PhaseEvent, SessionPhase, SessionResult};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

const DAEMON_SESSION_DIR_ENV: &str = "CSA_DAEMON_SESSION_DIR";
const DAEMON_PROJECT_ROOT_ENV: &str = "CSA_DAEMON_PROJECT_ROOT";
const DAEMON_COMPLETION_FILE: &str = "daemon-completion.toml";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct DaemonCompletionPacket {
    pub(crate) exit_code: i32,
    pub(crate) status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reason: Option<String>,
}

impl DaemonCompletionPacket {
    fn from_exit_code(exit_code: i32) -> Self {
        Self::from_exit_code_and_reason(exit_code, None)
    }

    fn from_exit_code_and_reason(exit_code: i32, reason: Option<&str>) -> Self {
        Self {
            exit_code,
            status: csa_session::SessionResult::status_from_exit_code(exit_code),
            reason: reason
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned),
        }
    }
}

pub(crate) fn seed_daemon_session_env(session_id: &str, cd: Option<&str>) {
    let project_root = match crate::pipeline::determine_project_root(cd) {
        Ok(root) => root,
        Err(_) => return,
    };
    let session_dir = match csa_session::get_session_dir(&project_root, session_id) {
        Ok(dir) => dir,
        Err(_) => return,
    };

    // SAFETY: daemon child sets process-scoped env before async worker tasks rely on it.
    unsafe {
        std::env::set_var(DAEMON_PROJECT_ROOT_ENV, &project_root);
        std::env::set_var(DAEMON_SESSION_DIR_ENV, &session_dir);
    }
}

pub(crate) fn persist_daemon_completion_from_env(exit_code: i32) {
    persist_daemon_completion_from_env_with_reason(exit_code, None);
}

pub(crate) fn persist_daemon_completion_from_env_with_reason(exit_code: i32, reason: Option<&str>) {
    let Some(session_dir) = resolve_daemon_session_dir_from_env() else {
        return;
    };
    let packet = match reason {
        Some(reason) => DaemonCompletionPacket::from_exit_code_and_reason(exit_code, Some(reason)),
        None => DaemonCompletionPacket::from_exit_code(exit_code),
    };
    if let Err(err) = persist_daemon_completion(&session_dir, &packet) {
        warn!(
            path = %daemon_completion_path(&session_dir).display(),
            error = %err,
            "Failed to persist daemon completion packet"
        );
        return;
    }
    if let Err(err) = finalize_daemon_completion_if_present(&session_dir) {
        warn!(
            path = %session_dir.display(),
            exit_code,
            status = %packet.status,
            error = %err,
            "Failed to synthesize terminal session result from daemon completion packet"
        );
    }
}

pub(crate) fn daemon_completion_exists(session_dir: &Path) -> bool {
    daemon_completion_path(session_dir).is_file()
}

pub(crate) fn load_daemon_completion_packet(
    session_dir: &Path,
) -> Result<Option<DaemonCompletionPacket>> {
    let path = daemon_completion_path(session_dir);
    if !path.is_file() {
        return Ok(None);
    }

    let content = fs::read_to_string(&path)?;
    let packet = toml::from_str(&content)?;
    Ok(Some(packet))
}

pub(crate) fn finalize_daemon_completion_if_present(
    session_dir: &Path,
) -> Result<Option<SessionResult>> {
    if load_daemon_completion_packet(session_dir)?.is_none() {
        return Ok(None);
    }
    if super::session_has_terminal_process(session_dir) {
        debug!(
            path = %daemon_completion_path(session_dir).display(),
            "Ignoring daemon completion packet while session process is still live"
        );
        return Ok(None);
    }
    let session = load_session_state_from_dir(session_dir)?;
    let project_root = PathBuf::from(&session.project_path);
    crate::session_cmds::ensure_terminal_result_for_dead_active_session(
        &project_root,
        &session.meta_session_id,
        "daemon completion",
    )?;
    load_result_from_dir(session_dir)
}

fn persist_daemon_completion(session_dir: &Path, packet: &DaemonCompletionPacket) -> Result<()> {
    let path = daemon_completion_path(session_dir);
    let temp_path = path.with_extension("toml.tmp");
    fs::write(&temp_path, toml::to_string_pretty(packet)?)?;
    fs::rename(temp_path, path)?;
    Ok(())
}

fn daemon_completion_path(session_dir: &Path) -> PathBuf {
    session_dir.join(DAEMON_COMPLETION_FILE)
}

fn resolve_daemon_session_dir_from_env() -> Option<PathBuf> {
    if let Some(session_dir) = std::env::var_os(DAEMON_SESSION_DIR_ENV) {
        return Some(PathBuf::from(session_dir));
    }

    let session_id = std::env::var("CSA_DAEMON_SESSION_ID").ok()?;
    let project_root = std::env::var_os(DAEMON_PROJECT_ROOT_ENV)
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())?;
    csa_session::get_session_dir(&project_root, &session_id).ok()
}

pub(crate) fn daemon_completion_result(
    project_root: &Path,
    session_dir: &Path,
    session: &MetaSessionState,
    packet: &DaemonCompletionPacket,
    completed_at: chrono::DateTime<chrono::Utc>,
) -> SessionResult {
    let tool_name = session
        .tools
        .iter()
        .max_by_key(|(_, state)| state.updated_at)
        .map(|(tool, _)| tool.clone())
        .or_else(|| {
            let metadata_path = session_dir.join(csa_session::metadata::METADATA_FILE_NAME);
            std::fs::read_to_string(metadata_path)
                .ok()
                .and_then(|c| toml::from_str::<csa_session::metadata::SessionMetadata>(&c).ok())
                .map(|m| m.tool)
        })
        .unwrap_or_else(|| "unknown".to_string());
    let summary_prefix = format!(
        "daemon completion recorded status={} exit_code={}{} before result.toml was written; committed or staged work may be salvageable on the session branch",
        packet.status,
        packet.exit_code,
        packet
            .reason
            .as_deref()
            .map(|reason| format!(" reason={reason}"))
            .unwrap_or_default()
    );

    SessionResult {
        post_exec_gate: None,
        status: packet.status.clone(),
        exit_code: packet.exit_code,
        summary: crate::pipeline_post_exec::build_fallback_result_summary(
            session_dir,
            &summary_prefix,
        ),
        tool: tool_name,
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: std::cmp::min(session.last_accessed, completed_at),
        completed_at,
        events_count: 0,
        artifacts: crate::pipeline_post_exec::collect_fallback_result_artifacts(
            project_root,
            &session.meta_session_id,
        ),
        ..Default::default()
    }
}

pub(crate) fn retire_session_from_daemon_completion(
    session: &mut MetaSessionState,
    packet: &DaemonCompletionPacket,
    completed_at: chrono::DateTime<chrono::Utc>,
) -> bool {
    if !matches!(session.phase, SessionPhase::Active) {
        return false;
    }

    session.last_accessed = completed_at;
    session.termination_reason.get_or_insert_with(|| {
        packet.reason.as_deref().map_or_else(
            || {
                if packet.exit_code == 0 {
                    "completed".to_string()
                } else {
                    "daemon_completion".to_string()
                }
            },
            ToOwned::to_owned,
        )
    });
    if let Err(err) = session.apply_phase_event(PhaseEvent::Retired) {
        warn!(
            session = %session.meta_session_id,
            phase = ?session.phase,
            error = %err,
            "Failed to transition daemon-completed session to Retired; forcing terminal phase"
        );
        session.phase = SessionPhase::Retired;
    }
    true
}

fn load_session_state_from_dir(session_dir: &Path) -> Result<MetaSessionState> {
    let state_path = session_dir.join("state.toml");
    let contents = fs::read_to_string(&state_path)
        .with_context(|| format!("Failed to read state file: {}", state_path.display()))?;
    toml::from_str(&contents)
        .with_context(|| format!("Failed to parse state file: {}", state_path.display()))
}

fn load_result_from_dir(session_dir: &Path) -> Result<Option<SessionResult>> {
    let result_path = session_dir.join(csa_session::result::RESULT_FILE_NAME);
    if !result_path.is_file() {
        return Ok(None);
    }
    let contents = fs::read_to_string(&result_path)
        .with_context(|| format!("Failed to read result file: {}", result_path.display()))?;
    toml::from_str(&contents)
        .map(Some)
        .with_context(|| format!("Failed to parse result file: {}", result_path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_env_lock::{ScopedEnvVarRestore, TEST_ENV_LOCK};
    use csa_session::{
        SessionPhase, create_session, get_session_dir, load_result, load_session, save_result,
        save_session,
    };
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn persist_result_if_absent_removes_partial_temp_when_write_fails() -> Result<()> {
        let tmp = tempdir()?;
        let result_path = tmp.path().join(csa_session::result::RESULT_FILE_NAME);

        let err = crate::session_result_publish::publish_result_file_if_absent_with_writer(
            &result_path,
            "status = \"failure\"\nexit_code = 17\n",
            "daemon result",
            |file, _contents| {
                file.write_all(b"partial")?;
                Err(std::io::Error::other("boom"))
            },
        )
        .unwrap_err();

        assert!(
            err.to_string()
                .contains("Failed to write or sync daemon result"),
            "unexpected error: {err:#}"
        );
        assert!(
            !result_path.exists(),
            "partial daemon result should not be published after write failure"
        );
        let entries = fs::read_dir(tmp.path())?.collect::<std::io::Result<Vec<_>>>()?;
        assert!(
            entries.is_empty(),
            "temporary daemon result should be cleaned up after write failure"
        );
        Ok(())
    }

    #[test]
    fn finalize_daemon_completion_preserves_available_session_and_existing_result() -> Result<()> {
        let tmp = tempdir()?;
        let _env_lock = TEST_ENV_LOCK.blocking_lock();
        let state_home = tmp.path().join("xdg-state");
        std::fs::create_dir_all(&state_home)?;
        let _home_guard = ScopedEnvVarRestore::set("HOME", tmp.path());
        let _state_guard = ScopedEnvVarRestore::set("XDG_STATE_HOME", &state_home);
        let project = tmp.path();

        let session = create_session(
            project,
            Some("daemon-available-completion-packet"),
            None,
            Some("codex"),
        )?;
        let session_id = session.meta_session_id;
        let session_dir = get_session_dir(project, &session_id)?;

        let mut persisted = load_session(project, &session_id)?;
        persisted.phase = SessionPhase::Available;
        save_session(&persisted)?;
        let state_path = session_dir.join("state.toml");
        let old_mtime = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let state_file = fs::File::options().write(true).open(&state_path)?;
        state_file.set_times(std::fs::FileTimes::new().set_modified(old_mtime))?;
        drop(state_file);
        let state_before = fs::read_to_string(&state_path)?;
        let state_modified_before = fs::metadata(&state_path)?.modified()?;

        let now = chrono::Utc::now();
        let existing = SessionResult {
            post_exec_gate: None,
            status: "success".to_string(),
            exit_code: 0,
            summary: "existing compact result".to_string(),
            tool: "codex".to_string(),
            original_tool: None,
            fallback_tool: None,
            fallback_reason: None,
            started_at: now,
            completed_at: now,
            events_count: 0,
            artifacts: Vec::new(),
            ..Default::default()
        };
        save_result(project, &session_id, &existing)?;
        let result_path = session_dir.join(csa_session::result::RESULT_FILE_NAME);
        let result_before = fs::read_to_string(&result_path)?;
        let result_modified_before = fs::metadata(&result_path)?.modified()?;

        fs::write(
            daemon_completion_path(&session_dir),
            "exit_code = 17\nstatus = \"failure\"\n",
        )?;

        let finalized = finalize_daemon_completion_if_present(&session_dir)?
            .expect("existing result should remain visible");
        assert_eq!(finalized.status, "success");
        assert_eq!(finalized.exit_code, 0);

        let persisted = load_session(project, &session_id)?;
        assert_eq!(persisted.phase, SessionPhase::Available);
        assert_eq!(fs::read_to_string(&state_path)?, state_before);
        assert_eq!(
            fs::metadata(&state_path)?.modified()?,
            state_modified_before
        );
        assert_eq!(fs::read_to_string(&result_path)?, result_before);
        assert_eq!(
            fs::metadata(&result_path)?.modified()?,
            result_modified_before
        );

        let result = load_result(project, &session_id)?.expect("existing result should remain");
        assert_eq!(result.status, "success");
        assert_eq!(result.exit_code, 0);
        Ok(())
    }
}
