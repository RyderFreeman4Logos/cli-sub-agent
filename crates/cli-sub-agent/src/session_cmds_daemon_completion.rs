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
const LEGACY_COMPLETE_FILE: &str = ".complete";
const MISSING_RESULT_TERMINATION_REASON: &str = "daemon_completion_missing_result";
const REVIEW_FIX_FINDING_TASK_TYPE: &str = "review_fix_finding";

#[cfg(test)]
#[path = "session_cmds_daemon_completion_resume_alias_tests.rs"]
mod resume_alias_tests;

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

    pub(crate) fn is_legacy_complete_marker(&self) -> bool {
        self.reason.as_deref() == Some("legacy_complete_marker")
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
        || legacy_complete_marker_path(session_dir).is_file()
}

pub(crate) fn legacy_complete_marker_is_valid(session_dir: &Path) -> bool {
    load_legacy_complete_marker_packet(session_dir).is_ok_and(|packet| packet.is_some())
}

pub(crate) fn load_daemon_completion_packet(
    session_dir: &Path,
) -> Result<Option<DaemonCompletionPacket>> {
    let path = daemon_completion_path(session_dir);
    if !path.is_file() {
        return load_legacy_complete_marker_packet(session_dir);
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
    if !packet.is_legacy_complete_marker() && super::session_has_terminal_process(session_dir) {
        debug!(
            path = %daemon_completion_path(session_dir).display(),
            "Ignoring daemon completion packet while session process is still live"
        );
        return Ok(None);
    }
    let session = load_session_state_from_dir(session_dir)?;
    let project_root = PathBuf::from(&session.project_path);
    if let Some(target) = resolve_fix_finding_resume_target(&project_root, session_dir)? {
        crate::session_cmds::ensure_terminal_result_for_dead_active_session(
            &project_root,
            &target.session_id,
            "daemon completion",
        )?;
        crate::session_cmds::retire_if_dead_with_result(
            &project_root,
            &target.session_id,
            "daemon completion",
        )?;
        return load_result_from_dir(&target.session_dir);
    }
    crate::session_cmds::ensure_terminal_result_for_dead_active_session(
        &project_root,
        &session.meta_session_id,
        "daemon completion",
    )?;
    crate::session_cmds::retire_if_dead_with_result(
        &project_root,
        &session.meta_session_id,
        "daemon completion",
    )?;
    load_result_from_dir(session_dir)
}

fn resolve_fix_finding_resume_target(
    project_root: &Path,
    wrapper_session_dir: &Path,
) -> Result<Option<csa_session::ResumeTargetResolution>> {
    let Some(target) =
        csa_session::resolve_resume_target_from_dir(project_root, wrapper_session_dir)?
    else {
        return Ok(None);
    };
    let target_session = load_session_state_from_dir(&target.session_dir)?;
    if target_session.task_context.task_type.as_deref() == Some(REVIEW_FIX_FINDING_TASK_TYPE) {
        return Ok(Some(target));
    }
    Ok(None)
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

fn legacy_complete_marker_path(session_dir: &Path) -> PathBuf {
    session_dir.join(LEGACY_COMPLETE_FILE)
}

fn load_legacy_complete_marker_packet(
    session_dir: &Path,
) -> Result<Option<DaemonCompletionPacket>> {
    let path = legacy_complete_marker_path(session_dir);
    if !path.is_file() {
        return Ok(None);
    }

    let content = fs::read_to_string(&path)?;
    let exit_code = content.trim().parse::<i32>().with_context(|| {
        format!(
            "Failed to parse legacy completion marker {} as exit code",
            path.display()
        )
    })?;
    Ok(Some(DaemonCompletionPacket::from_exit_code_and_reason(
        exit_code,
        Some("legacy_complete_marker"),
    )))
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
    if let Some(result) = super::review_diagnostic::review_result_from_existing_artifacts(
        project_root,
        session_dir,
        session,
        packet,
        completed_at,
    ) {
        return result;
    }

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
    let tool_note = if tool_name == "unknown" {
        "; no tool launch metadata was recorded"
    } else {
        ""
    };
    let effective = daemon_completion_effective_outcome(packet, session_dir);
    let missing_result_note = if effective.forced_missing_result_failure {
        "; treating daemon completion as failure because result.toml was missing"
    } else {
        ""
    };
    let summary_prefix = format!(
        "daemon completion recorded status={} exit_code={}{} before result.toml was written{missing_result_note}{tool_note}; committed or staged work may be salvageable on the session branch",
        packet.status,
        packet.exit_code,
        packet
            .reason
            .as_deref()
            .map(|reason| format!(" reason={reason}"))
            .unwrap_or_default()
    );

    let mut artifacts = crate::pipeline_post_exec::collect_fallback_result_artifacts(
        project_root,
        &session.meta_session_id,
    );
    super::review_diagnostic::append_review_no_result_diagnostic_artifacts(&mut artifacts, session);

    SessionResult {
        post_exec_gate: None,
        status: effective.status,
        exit_code: effective.exit_code,
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
        artifacts,
        raw_process_exit_code: effective.raw_process_exit_code,
        ..Default::default()
    }
}

struct DaemonCompletionEffectiveOutcome {
    status: String,
    exit_code: i32,
    raw_process_exit_code: Option<i32>,
    forced_missing_result_failure: bool,
}

fn daemon_completion_effective_outcome(
    packet: &DaemonCompletionPacket,
    session_dir: &Path,
) -> DaemonCompletionEffectiveOutcome {
    if packet.status == "success" || packet.exit_code == 0 {
        // ACP transport tools (e.g., hermes) do not write result.toml via
        // env contract. When the daemon exits cleanly and the session has
        // non-empty collected output (stdout), treat it as success rather
        // than forcing a misleading failure. (#2588)
        let stdout_path = session_dir.join("stdout.log");
        let has_output = fs::read_to_string(&stdout_path).is_ok_and(|s| !s.trim().is_empty());
        if has_output {
            return DaemonCompletionEffectiveOutcome {
                status: "success".to_string(),
                exit_code: 0,
                raw_process_exit_code: Some(packet.exit_code),
                forced_missing_result_failure: false,
            };
        }
        return DaemonCompletionEffectiveOutcome {
            status: "failure".to_string(),
            exit_code: 1,
            raw_process_exit_code: (packet.exit_code != 1).then_some(packet.exit_code),
            forced_missing_result_failure: true,
        };
    }

    DaemonCompletionEffectiveOutcome {
        status: packet.status.clone(),
        exit_code: packet.exit_code,
        raw_process_exit_code: None,
        forced_missing_result_failure: false,
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
                    MISSING_RESULT_TERMINATION_REASON.to_string()
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

    #[test]
    fn daemon_completion_result_fails_closed_when_packet_claims_success() -> Result<()> {
        let tmp = tempdir()?;
        let _env_lock = TEST_ENV_LOCK.blocking_lock();
        let state_home = tmp.path().join("xdg-state");
        std::fs::create_dir_all(&state_home)?;
        let _home_guard = ScopedEnvVarRestore::set("HOME", tmp.path());
        let _state_guard = ScopedEnvVarRestore::set("XDG_STATE_HOME", &state_home);
        let project = tmp.path();

        let session = create_session(
            project,
            Some("daemon-success-before-result"),
            None,
            Some("codex"),
        )?;
        let session_id = session.meta_session_id;
        let session_dir = get_session_dir(project, &session_id)?;
        let session = load_session(project, &session_id)?;
        let packet = DaemonCompletionPacket::from_exit_code(0);

        let result =
            daemon_completion_result(project, &session_dir, &session, &packet, chrono::Utc::now());

        assert_eq!(result.status, "failure");
        assert_eq!(result.exit_code, 1);
        assert_eq!(result.raw_process_exit_code, Some(0));
        assert_eq!(result.tool, "codex");
        assert!(
            result
                .summary
                .contains("treating daemon completion as failure"),
            "summary should explain fail-closed conversion: {}",
            result.summary
        );
        Ok(())
    }

    #[test]
    fn daemon_completion_result_marks_historical_turn_sidecar_display_only() -> Result<()> {
        let tmp = tempdir()?;
        let _env_lock = TEST_ENV_LOCK.blocking_lock();
        let state_home = tmp.path().join("xdg-state");
        std::fs::create_dir_all(&state_home)?;
        let _home_guard = ScopedEnvVarRestore::set("HOME", tmp.path());
        let _state_guard = ScopedEnvVarRestore::set("XDG_STATE_HOME", &state_home);
        let project = tmp.path();

        let session = create_session(
            project,
            Some("daemon historical sidecar"),
            None,
            Some("codex"),
        )?;
        let session_id = session.meta_session_id;
        let session_dir = get_session_dir(project, &session_id)?;
        let stale_artifact = csa_session::turn_contract_result_artifact_path(1);
        let stale_path = session_dir.join(&stale_artifact);
        std::fs::create_dir_all(stale_path.parent().expect("stale turn parent"))?;
        std::fs::write(
            &stale_path,
            "[report]\nwhat_was_done = \"stale daemon turn report\"\n",
        )?;
        let session = load_session(project, &session_id)?;
        let packet = DaemonCompletionPacket::from_exit_code(0);

        let result =
            daemon_completion_result(project, &session_dir, &session, &packet, chrono::Utc::now());

        assert!(
            result.manager_fields.as_sidecar().is_none(),
            "daemon-completion synthetic result must not own stale manager fields"
        );
        assert!(
            result
                .artifacts
                .iter()
                .any(|artifact| artifact.path == stale_artifact && artifact.display_only),
            "historical turn sidecar remains visible as display-only diagnostics"
        );
        Ok(())
    }

    #[test]
    fn daemon_completion_result_explicitly_reports_no_tool_launch_metadata() -> Result<()> {
        let tmp = tempdir()?;
        let _env_lock = TEST_ENV_LOCK.blocking_lock();
        let state_home = tmp.path().join("xdg-state");
        std::fs::create_dir_all(&state_home)?;
        let _home_guard = ScopedEnvVarRestore::set("HOME", tmp.path());
        let _state_guard = ScopedEnvVarRestore::set("XDG_STATE_HOME", &state_home);
        let project = tmp.path();

        let session = create_session(project, Some("daemon-no-tool-before-result"), None, None)?;
        let session_id = session.meta_session_id;
        let session_dir = get_session_dir(project, &session_id)?;
        let session = load_session(project, &session_id)?;
        let packet = DaemonCompletionPacket::from_exit_code(0);

        let result =
            daemon_completion_result(project, &session_dir, &session, &packet, chrono::Utc::now());

        assert_eq!(result.tool, "unknown");
        assert!(
            result
                .summary
                .contains("no tool launch metadata was recorded"),
            "summary should make unknown tool explicit: {}",
            result.summary
        );
        Ok(())
    }
}
