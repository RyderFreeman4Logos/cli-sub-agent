use std::path::Path;

use anyhow::{Context, Result};
use csa_session::{MetaSessionState, PhaseEvent, SessionPhase, save_session};

const FAILED_LAUNCH_DESCRIPTION: &str =
    "daemon launch failed before start marker; inspect session spool logs for diagnostics";

pub(crate) fn retire_failed_placeholder(project_root: &Path, session_id: &str) -> Result<()> {
    let mut session: MetaSessionState = csa_session::load_session(project_root, session_id)
        .with_context(|| {
            format!("failed to load daemon placeholder {session_id} for retirement")
        })?;
    session.description = Some(FAILED_LAUNCH_DESCRIPTION.to_string());
    if session.phase != SessionPhase::Retired {
        session
            .apply_phase_event(PhaseEvent::Retired)
            .map_err(anyhow::Error::msg)
            .with_context(|| format!("failed to retire daemon placeholder {session_id}"))?;
    }
    save_session(&session)
        .with_context(|| format!("failed to persist retired daemon placeholder {session_id}"))
}

pub(crate) fn attach_retirement_context(
    launch_error: anyhow::Error,
    project_root: &Path,
    session_id: &str,
) -> anyhow::Error {
    match retire_failed_placeholder(project_root, session_id) {
        Ok(()) => launch_error,
        Err(retire_error) => launch_error.context(format!(
            "daemon launch failed and placeholder retirement also failed: {retire_error:#}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_placeholder_is_retired_without_deleting_diagnostics() {
        let _lock = crate::test_env_lock::TEST_ENV_LOCK
            .clone()
            .blocking_lock_owned();
        let temp = tempfile::tempdir().expect("tempdir");
        let project_root = temp.path().join("project");
        std::fs::create_dir_all(&project_root).expect("project root");
        let session_id = csa_session::new_session_id();
        let session_root =
            csa_session::get_session_root(&project_root).expect("session root should resolve");
        let session_dir = csa_session::get_session_dir(&project_root, &session_id)
            .expect("session dir should resolve");
        csa_session::create_session_with_daemon_env(
            &project_root,
            Some("initializing daemon run"),
            None,
            None,
            Some(&session_id),
            Some(&session_dir),
            Some(&project_root),
        )
        .expect("placeholder should persist");
        std::fs::write(session_dir.join("stderr.log"), "launch diagnostics\n")
            .expect("diagnostic log");

        retire_failed_placeholder(&project_root, &session_id).expect("retire failed placeholder");

        let session = csa_session::load_session(&project_root, &session_id)
            .expect("retired placeholder remains readable");
        assert_eq!(session.phase, SessionPhase::Retired);
        assert_eq!(
            session.description.as_deref(),
            Some(FAILED_LAUNCH_DESCRIPTION)
        );
        assert_eq!(
            std::fs::read_to_string(session_dir.join("stderr.log")).expect("diagnostics retained"),
            "launch diagnostics\n"
        );
        let _ = std::fs::remove_dir_all(session_root);
    }
}
