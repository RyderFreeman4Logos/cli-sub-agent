//! Daemon completion packet handling and terminal result synthesis.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
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
}

impl DaemonCompletionPacket {
    fn from_exit_code(exit_code: i32) -> Self {
        Self {
            exit_code,
            status: csa_session::SessionResult::status_from_exit_code(exit_code),
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
    let Some(session_dir) = resolve_daemon_session_dir_from_env() else {
        return;
    };
    let packet = DaemonCompletionPacket::from_exit_code(exit_code);
    if let Err(err) = persist_daemon_completion(&session_dir, &packet) {
        warn!(
            path = %daemon_completion_path(&session_dir).display(),
            error = %err,
            "Failed to persist daemon completion packet"
        );
        return;
    }
    if let Err(err) = finalize_daemon_completion(&session_dir, &packet) {
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
    let Some(packet) = load_daemon_completion_packet(session_dir)? else {
        return Ok(None);
    };
    finalize_daemon_completion(session_dir, &packet)?;
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

fn finalize_daemon_completion(session_dir: &Path, packet: &DaemonCompletionPacket) -> Result<()> {
    let mut session = load_session_state_from_dir(session_dir)?;
    let project_root = PathBuf::from(&session.project_path);
    let completed_at = chrono::Utc::now();

    if load_result_from_dir(session_dir)?.is_none() {
        let result =
            daemon_completion_result(&project_root, session_dir, &session, packet, completed_at);
        persist_result_if_absent(session_dir, &result)?;
    }

    retire_session_from_daemon_completion(&mut session, packet, completed_at);
    persist_session_state_to_dir(session_dir, &session)?;
    csa_session::write_cooldown_marker_from_session_dir(
        session_dir,
        &session.meta_session_id,
        completed_at,
    );
    Ok(())
}

fn daemon_completion_result(
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
        .unwrap_or_else(|| "unknown".to_string());
    let summary_prefix = format!(
        "daemon completion recorded status={} exit_code={} before result.toml was written; committed or staged work may be salvageable on the session branch",
        packet.status, packet.exit_code
    );

    SessionResult {
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
        peak_memory_mb: None,
        fallback_chain: None,
        gate_timeout: false,
        warnings: Vec::new(),
        raw_process_exit_code: None,
        uncommitted_changes: None,
        manager_fields: Default::default(),
    }
}

fn retire_session_from_daemon_completion(
    session: &mut MetaSessionState,
    packet: &DaemonCompletionPacket,
    completed_at: chrono::DateTime<chrono::Utc>,
) {
    session.last_accessed = completed_at;
    session.termination_reason.get_or_insert_with(|| {
        if packet.exit_code == 0 {
            "completed".to_string()
        } else {
            "daemon_completion".to_string()
        }
    });
    if matches!(
        session.phase,
        SessionPhase::Active | SessionPhase::Available
    ) && let Err(err) = session.apply_phase_event(PhaseEvent::Retired)
    {
        warn!(
            session = %session.meta_session_id,
            phase = ?session.phase,
            error = %err,
            "Failed to transition daemon-completed session to Retired; forcing terminal phase"
        );
        session.phase = SessionPhase::Retired;
    }
}

fn load_session_state_from_dir(session_dir: &Path) -> Result<MetaSessionState> {
    let state_path = session_dir.join("state.toml");
    let contents = fs::read_to_string(&state_path)
        .with_context(|| format!("Failed to read state file: {}", state_path.display()))?;
    toml::from_str(&contents)
        .with_context(|| format!("Failed to parse state file: {}", state_path.display()))
}

fn persist_session_state_to_dir(session_dir: &Path, session: &MetaSessionState) -> Result<()> {
    let state_path = session_dir.join("state.toml");
    let contents = toml::to_string_pretty(session).context("Failed to serialize session state")?;
    fs::write(&state_path, contents)
        .with_context(|| format!("Failed to write state file: {}", state_path.display()))
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

fn persist_result_if_absent(session_dir: &Path, result: &SessionResult) -> Result<()> {
    let result_path = session_dir.join(csa_session::result::RESULT_FILE_NAME);
    let contents = toml::to_string_pretty(result).context("Failed to serialize daemon result")?;
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&result_path)
    {
        Ok(mut file) => {
            file.write_all(contents.as_bytes()).with_context(|| {
                format!("Failed to write result file: {}", result_path.display())
            })?;
            file.sync_all().with_context(|| {
                format!("Failed to sync result file: {}", result_path.display())
            })?;
            Ok(())
        }
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
            debug!(
                path = %result_path.display(),
                "Result appeared while finalizing daemon completion; preserving existing result"
            );
            Ok(())
        }
        Err(err) => Err(anyhow!(
            "Failed to create result file {}: {}",
            result_path.display(),
            err
        )),
    }
}
