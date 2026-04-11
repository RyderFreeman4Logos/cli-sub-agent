use anyhow::{Result, anyhow};
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::Path;
use tracing::{info, warn};

use csa_session::{
    SessionPhase, SessionResult, get_session_dir, load_result, load_session, save_session_in,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeadActiveSessionReconciliation {
    NoChange,
    SynthesizedFailure,
    LateResultRetired,
}

impl DeadActiveSessionReconciliation {
    pub(crate) fn result_became_available(self) -> bool {
        matches!(self, Self::SynthesizedFailure | Self::LateResultRetired)
    }

    pub(crate) fn synthesized_failure(self) -> bool {
        matches!(self, Self::SynthesizedFailure)
    }
}

pub(crate) fn ensure_terminal_result_for_dead_active_session(
    project_root: &Path,
    session_id: &str,
    trigger: &str,
) -> Result<DeadActiveSessionReconciliation> {
    ensure_terminal_result_for_dead_active_session_impl(
        project_root,
        session_id,
        trigger,
        |_| {},
        |_| {},
    )
}

#[cfg(test)]
pub(crate) fn ensure_terminal_result_for_dead_active_session_with_before_write<F>(
    project_root: &Path,
    session_id: &str,
    trigger: &str,
    before_write: F,
) -> Result<DeadActiveSessionReconciliation>
where
    F: FnOnce(&Path),
{
    ensure_terminal_result_for_dead_active_session_impl(
        project_root,
        session_id,
        trigger,
        before_write,
        |_| {},
    )
}

fn ensure_terminal_result_for_dead_active_session_impl<F, B>(
    project_root: &Path,
    session_id: &str,
    trigger: &str,
    before_write: F,
    before_retire: B,
) -> Result<DeadActiveSessionReconciliation>
where
    F: FnOnce(&Path),
    B: FnOnce(&mut csa_session::MetaSessionState),
{
    let mut session = load_session(project_root, session_id)?;
    if !matches!(session.phase, SessionPhase::Active) {
        return Ok(DeadActiveSessionReconciliation::NoChange);
    }
    let session_dir = get_session_dir(project_root, session_id)?;
    if csa_process::ToolLiveness::has_live_process(&session_dir) {
        return Ok(DeadActiveSessionReconciliation::NoChange);
    }
    let result_path = session_dir.join(csa_session::result::RESULT_FILE_NAME);
    match load_result(project_root, session_id) {
        Ok(Some(_)) => return Ok(DeadActiveSessionReconciliation::NoChange),
        Ok(None) => {}
        Err(err) if result_path.is_file() => {
            warn!(
                session_id = %session_id,
                trigger = %trigger,
                reconciliation_reason = "late_result_write_unreadable",
                result_path = %result_path.display(),
                error = %err,
                "Result file appeared during dead-session reconciliation; preserving late writer and skipping synthetic fallback"
            );
            return Ok(DeadActiveSessionReconciliation::NoChange);
        }
        Err(err) => return Err(err),
    }

    let now = chrono::Utc::now();
    let tool_name = session
        .tools
        .iter()
        .max_by_key(|(_, state)| state.updated_at)
        .map(|(tool, _)| tool.clone())
        .unwrap_or_else(|| "unknown".to_string());
    let artifacts =
        crate::pipeline_post_exec::collect_fallback_result_artifacts(project_root, session_id);
    let output_log_mtime = format_optional_file_mtime(&session_dir.join("output.log"));
    let summary_prefix = format!(
        "synthetic failure by {trigger}: process dead, result.toml missing (reconciliation_reason=true_missing_result, output_log_mtime={})",
        output_log_mtime.as_deref().unwrap_or("missing")
    );
    let fallback = SessionResult {
        status: "failure".to_string(),
        exit_code: 1,
        summary: crate::pipeline_post_exec::build_fallback_result_summary(
            &session_dir,
            &summary_prefix,
        ),
        tool: tool_name,
        started_at: std::cmp::min(session.last_accessed, now),
        completed_at: now,
        events_count: 0,
        artifacts,
        peak_memory_mb: None,
    };
    let result_contents = toml::to_string_pretty(&fallback)
        .map_err(|err| anyhow!("Failed to serialize synthetic result for {session_id}: {err}"))?;
    match persist_new_result_file(&result_path, &result_contents, before_write)? {
        SyntheticResultPersistOutcome::AlreadyExists => {
            let retired = retire_if_dead_with_result(project_root, session_id, trigger)?;
            info!(
                session_id = %session_id,
                trigger = %trigger,
                reconciliation_reason = "late_result_write",
                result_path = %result_path.display(),
                result_mtime = %format_optional_file_mtime(&result_path).unwrap_or_else(|| "unknown".to_string()),
                "Late result.toml write won during dead-session reconciliation"
            );
            return Ok(if retired {
                DeadActiveSessionReconciliation::LateResultRetired
            } else {
                DeadActiveSessionReconciliation::NoChange
            });
        }
        SyntheticResultPersistOutcome::Created => {}
    }

    csa_session::write_cooldown_marker_from_session_dir(
        &session_dir,
        session_id,
        fallback.completed_at,
    );
    before_retire(&mut session);
    if let Err(err) = session.apply_phase_event(csa_session::PhaseEvent::Retired) {
        warn!(
            session_id = %session_id,
            trigger = %trigger,
            reconciliation_reason = "true_missing_result",
            error = %err,
            "Failed to transition orphaned session to Retired phase during reconciliation; leaving session state unchanged"
        );
        return Ok(DeadActiveSessionReconciliation::NoChange);
    }
    session.termination_reason = Some("orphaned_process".to_string());
    let session_root = derive_session_root(&session_dir)?;
    save_session_in(session_root, &session)?;
    warn!(
        session_id = %session_id,
        trigger = %trigger,
        reconciliation_reason = "true_missing_result",
        result_path = %result_path.display(),
        output_log_mtime = %output_log_mtime.unwrap_or_else(|| "missing".to_string()),
        "Recovered orphaned session with synthetic result"
    );
    Ok(DeadActiveSessionReconciliation::SynthesizedFailure)
}

pub(crate) fn retire_if_dead_with_result(
    project_root: &Path,
    session_id: &str,
    trigger: &str,
) -> Result<bool> {
    let mut session = load_session(project_root, session_id)?;
    if !matches!(session.phase, SessionPhase::Active) {
        return Ok(false);
    }
    let session_dir = get_session_dir(project_root, session_id)?;
    if csa_process::ToolLiveness::has_live_process(&session_dir)
        || load_result(project_root, session_id)?.is_none()
    {
        return Ok(false);
    }
    if session
        .apply_phase_event(csa_session::PhaseEvent::Retired)
        .is_err()
    {
        return Ok(false);
    }
    session
        .termination_reason
        .get_or_insert_with(|| "completed".to_string());
    let session_root = derive_session_root(&session_dir)?;
    save_session_in(session_root, &session)?;
    info!(
        session_id = %session_id,
        trigger = %trigger,
        "Retired dead Active session with result"
    );
    Ok(true)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyntheticResultPersistOutcome {
    Created,
    AlreadyExists,
}

fn derive_session_root(session_dir: &Path) -> Result<&Path> {
    session_dir
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| anyhow!("Invalid session dir layout: {}", session_dir.display()))
}

fn persist_new_result_file<F>(
    result_path: &Path,
    contents: &str,
    before_write: F,
) -> Result<SyntheticResultPersistOutcome>
where
    F: FnOnce(&Path),
{
    persist_new_result_file_with_writer(result_path, contents, before_write, |file, contents| {
        file.write_all(contents.as_bytes())?;
        file.sync_all()
    })
}

fn persist_new_result_file_with_writer<F, W>(
    result_path: &Path,
    contents: &str,
    before_write: F,
    write_contents: W,
) -> Result<SyntheticResultPersistOutcome>
where
    F: FnOnce(&Path),
    W: FnOnce(&mut fs::File, &str) -> std::io::Result<()>,
{
    before_write(result_path);
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(result_path)
    {
        Ok(mut file) => {
            if let Err(err) = write_contents(&mut file, contents) {
                fs::remove_file(result_path).ok();
                return Err(anyhow!(
                    "Failed to write or sync synthetic result for {}: {err}",
                    result_path.display()
                ));
            }
            Ok(SyntheticResultPersistOutcome::Created)
        }
        Err(err) if err.kind() == ErrorKind::AlreadyExists => {
            Ok(SyntheticResultPersistOutcome::AlreadyExists)
        }
        Err(err) => Err(anyhow!(
            "Failed to create synthetic result for {}: {err}",
            result_path.display()
        )),
    }
}

fn format_optional_file_mtime(path: &Path) -> Option<String> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    let modified = chrono::DateTime::<chrono::Utc>::from(modified);
    Some(modified.to_rfc3339())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_cmds_daemon::{WaitReconciliationOutcome, handle_session_wait_with_hooks};
    use crate::test_env_lock::TEST_ENV_LOCK;
    use chrono::Utc;
    use csa_session::{
        SessionPhase, SessionResult, create_session, get_session_dir, load_result, load_session,
        save_result,
    };
    use tempfile::tempdir;

    struct EnvVarGuard {
        key: &'static str,
        original: Option<String>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let original = std::env::var(key).ok();
            // SAFETY: test-scoped env mutation guarded by a process-wide mutex.
            unsafe { std::env::set_var(key, value) };
            Self { key, original }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            // SAFETY: test-scoped env mutation guarded by a process-wide mutex.
            unsafe {
                match self.original.as_deref() {
                    Some(value) => std::env::set_var(self.key, value),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

    fn make_result(status: &str, exit_code: i32) -> SessionResult {
        let now = Utc::now();
        SessionResult {
            status: status.to_string(),
            exit_code,
            summary: "summary".to_string(),
            tool: "codex".to_string(),
            started_at: now,
            completed_at: now,
            events_count: 0,
            artifacts: Vec::new(),
            peak_memory_mb: None,
        }
    }

    #[test]
    fn ensure_terminal_result_for_dead_active_session_leaves_state_unchanged_on_transition_failure()
    {
        let td = tempdir().expect("tempdir");
        let _env_lock = TEST_ENV_LOCK.lock().expect("session env lock poisoned");
        let state_home = td.path().join("xdg-state");
        std::fs::create_dir_all(&state_home).expect("create state home");
        let _home_guard = EnvVarGuard::set("HOME", td.path());
        let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
        let project = td.path();

        let session = create_session(project, Some("transition-failure"), None, None).unwrap();
        let session_id = session.meta_session_id;
        let session_dir = get_session_dir(project, &session_id).unwrap();
        let state_path = session_dir.join("state.toml");
        let original_state = fs::read_to_string(&state_path).unwrap();

        let reconciled = ensure_terminal_result_for_dead_active_session_impl(
            project,
            &session_id,
            "session list",
            |_| {},
            |session| {
                session.phase = SessionPhase::Retired;
            },
        )
        .unwrap();

        assert_eq!(reconciled, DeadActiveSessionReconciliation::NoChange);
        let persisted = load_session(project, &session_id).unwrap();
        assert_eq!(persisted.phase, SessionPhase::Active);
        assert_eq!(persisted.termination_reason, None);
        assert_eq!(fs::read_to_string(&state_path).unwrap(), original_state);
        assert!(
            load_result(project, &session_id).unwrap().is_some(),
            "synthetic result should remain available for later reconciliation"
        );
    }

    #[test]
    fn persist_new_result_file_removes_partial_file_when_write_fails() {
        let td = tempdir().expect("tempdir");
        let result_path = td.path().join("result.toml");

        let err = persist_new_result_file_with_writer(
            &result_path,
            "status = \"failure\"\n",
            |_| {},
            |file, _contents| {
                file.write_all(b"partial")?;
                Err(std::io::Error::other("boom"))
            },
        )
        .unwrap_err();

        assert!(
            err.to_string()
                .contains("Failed to write or sync synthetic result"),
            "unexpected error: {err:#}"
        );
        assert!(
            !result_path.exists(),
            "partial synthetic result should be removed after write failure"
        );
    }

    #[test]
    fn handle_session_wait_marks_late_real_result_completion_as_non_synthetic() {
        let td = tempdir().expect("tempdir");
        let _env_lock = TEST_ENV_LOCK.lock().expect("session env lock poisoned");
        let state_home = td.path().join("xdg-state");
        std::fs::create_dir_all(&state_home).expect("create state home");
        let _home_guard = EnvVarGuard::set("HOME", td.path());
        let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
        let project = td.path();

        let session =
            create_session(project, Some("wait-late-real-result"), None, Some("codex")).unwrap();
        let session_id = session.meta_session_id;
        let late_result = SessionResult {
            summary: "real terminal result".to_string(),
            ..make_result("success", 0)
        };
        let mut emitted_completion: Option<(String, String, i32, bool)> = None;

        let exit_code = handle_session_wait_with_hooks(
            session_id.clone(),
            Some(project.to_string_lossy().into_owned()),
            5,
            |project_root, current_session_id, trigger| {
                let reconciled = ensure_terminal_result_for_dead_active_session_with_before_write(
                    project_root,
                    current_session_id,
                    trigger,
                    |_| {
                        save_result(project_root, current_session_id, &late_result)
                            .expect("persist late real result");
                    },
                )?;
                Ok(WaitReconciliationOutcome {
                    result_became_available: reconciled.result_became_available(),
                    synthetic: reconciled.synthesized_failure(),
                })
            },
            |sid, status, exit_code, synthetic, _mirror_to_stdout| {
                emitted_completion =
                    Some((sid.to_string(), status.to_string(), exit_code, synthetic));
            },
        )
        .unwrap();

        assert_eq!(exit_code, 0);
        assert_eq!(
            emitted_completion,
            Some((session_id, "success".to_string(), 0, false))
        );
    }
}
