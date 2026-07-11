use std::io::Write;
use std::path::Path;

use anyhow::Result;

#[derive(Debug)]
pub(crate) struct DaemonStartedOutput {
    stdout: String,
    stderr: String,
}

pub(crate) fn prepare(
    result: &csa_process::daemon::DaemonSpawnResult,
    project_root: &Path,
) -> Result<DaemonStartedOutput> {
    crate::run_cmd_daemon::verify_daemon_session_waitable(project_root, &result.session_id)?;
    let wait_cmd =
        crate::daemon_caller_hints::format_session_wait_command(&result.session_id, project_root);
    let attach_cmd =
        crate::daemon_caller_hints::format_session_attach_command(&result.session_id, project_root);
    let session_dir_attr = crate::daemon_caller_hints::escape_structured_comment_attr(
        &result.session_dir.display().to_string(),
    );
    let wait_cmd_attr = crate::daemon_caller_hints::escape_structured_comment_attr(&wait_cmd);
    let attach_cmd_attr = crate::daemon_caller_hints::escape_structured_comment_attr(&attach_cmd);
    let mut stderr = format!(
        "<!-- CSA:SESSION_STARTED id={id} pid={pid} dir=\"{dir}\" \
         wait_cmd=\"{wait_cmd}\" \
         attach_cmd=\"{attach_cmd}\" -->\n\
         <!-- CSA:CALLER_HINT action=\"wait\" rule=\"Call {wait_cmd} with run_in_background: true. Task-notification is your wake signal — no polling, no loops, one wait per Bash call.\" -->\n",
        id = result.session_id,
        pid = result.pid,
        dir = session_dir_attr,
        wait_cmd = wait_cmd_attr,
        attach_cmd = attach_cmd_attr,
    );
    stderr.push_str(&crate::process_tree::codex_yield_hint());
    Ok(DaemonStartedOutput {
        stdout: format!("{}\n", result.session_id),
        stderr,
    })
}

pub(crate) fn publish(output: DaemonStartedOutput) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    let mut stderr = std::io::stderr().lock();
    publish_to(&mut stdout, &mut stderr, output)
}

fn publish_to(
    stdout: &mut impl Write,
    stderr: &mut impl Write,
    output: DaemonStartedOutput,
) -> Result<()> {
    // The machine-readable session ID is the publication commit point. Before
    // it is flushed, an error returns to the still-owned spawner for cleanup.
    // Once committed, keep the daemon alive even if the supplementary stderr
    // marker cannot be written: the caller already has a waitable session ID.
    stdout.write_all(output.stdout.as_bytes())?;
    stdout.flush()?;
    if let Err(error) = stderr
        .write_all(output.stderr.as_bytes())
        .and_then(|()| stderr.flush())
    {
        tracing::warn!(%error, "failed to publish supplementary daemon start marker");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan_spawn_result(
        project_root: &Path,
    ) -> (
        String,
        std::path::PathBuf,
        csa_process::daemon::DaemonSpawnResult,
    ) {
        let session_id = csa_session::new_session_id();
        let session_root =
            csa_session::get_session_root(project_root).expect("session root should resolve");
        let session_dir = csa_session::get_session_dir(project_root, &session_id)
            .expect("session dir should resolve");
        let result = csa_process::daemon::DaemonSpawnResult {
            pid: 42,
            session_id: session_id.clone(),
            session_dir,
        };
        (session_id, session_root, result)
    }

    #[test]
    fn started_marker_requires_waitable_placeholder() {
        let _lock = crate::test_env_lock::TEST_ENV_LOCK
            .clone()
            .blocking_lock_owned();
        let temp = tempfile::tempdir().expect("tempdir");
        let project_root = temp.path().join("project");
        std::fs::create_dir_all(&project_root).expect("project root should exist");
        let (session_id, session_root, result) = plan_spawn_result(&project_root);
        csa_session::create_session_with_daemon_env(
            &project_root,
            Some("plan: workflow.toml"),
            None,
            None,
            Some(&session_id),
            Some(&result.session_dir),
            Some(&project_root),
        )
        .expect("plan placeholder should persist");

        let output = prepare(&result, &project_root)
            .expect("waitable plan placeholder should permit marker rendering");
        assert_eq!(output.stdout, format!("{session_id}\n"));
        assert_eq!(output.stderr.matches("CSA:SESSION_STARTED").count(), 1);
        assert!(output.stderr.contains("CSA:CALLER_HINT"));
        let _ = std::fs::remove_dir_all(session_root);
    }

    #[test]
    fn started_marker_is_suppressed_when_placeholder_is_unreadable() {
        let _lock = crate::test_env_lock::TEST_ENV_LOCK
            .clone()
            .blocking_lock_owned();
        let temp = tempfile::tempdir().expect("tempdir");
        let project_root = temp.path().join("project");
        std::fs::create_dir_all(&project_root).expect("project root should exist");
        let (_session_id, session_root, result) = plan_spawn_result(&project_root);
        std::fs::create_dir_all(&result.session_dir)
            .expect("session dir should exist without state.toml");

        prepare(&result, &project_root)
            .expect_err("unreadable placeholder must block marker rendering");
        let _ = std::fs::remove_dir_all(session_root);
    }

    struct FailWriter;

    impl Write for FailWriter {
        fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "test writer failure",
            ))
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn publishes_complete_started_payload() {
        let output = DaemonStartedOutput {
            stdout: "SESSION\n".to_string(),
            stderr: "MARKER\n".to_string(),
        };
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        publish_to(&mut stdout, &mut stderr, output).expect("publish output");

        assert_eq!(stdout, b"SESSION\n");
        assert_eq!(stderr, b"MARKER\n");
    }

    #[test]
    fn stdout_failure_occurs_before_publication_commit() {
        let output = DaemonStartedOutput {
            stdout: "SESSION\n".to_string(),
            stderr: "MARKER\n".to_string(),
        };
        let mut stdout = FailWriter;
        let mut stderr = Vec::new();

        publish_to(&mut stdout, &mut stderr, output)
            .expect_err("stdout failure must fail before detaching the daemon");
        assert!(stderr.is_empty());
    }

    #[test]
    fn stderr_failure_after_id_commit_does_not_retract_the_daemon() {
        let output = DaemonStartedOutput {
            stdout: "SESSION\n".to_string(),
            stderr: "MARKER\n".to_string(),
        };
        let mut stdout = Vec::new();
        let mut stderr = FailWriter;

        publish_to(&mut stdout, &mut stderr, output)
            .expect("committed session ID keeps the daemon usable");
        assert_eq!(stdout, b"SESSION\n");
    }
}
