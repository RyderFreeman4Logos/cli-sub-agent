use super::*;

struct DropProbe(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl Drop for DropProbe {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

#[test]
fn signal_interrupted_transport_result_models_sigterm_as_incomplete_turn() {
    let result = signal_interrupted_transport_result(
        143,
        Some(libc::SIGTERM),
        "sigterm",
        "Execution interrupted by SIGTERM",
    );

    assert_eq!(result.execution.exit_code, 143);
    assert_eq!(result.execution.raw_process_exit_code, Some(143));
    assert_eq!(result.execution.exit_signal, Some(libc::SIGTERM));
    assert_eq!(result.execution.terminal_reason.as_deref(), Some("sigterm"));
    assert_eq!(result.execution.model_completed, Some(false));
    assert!(result.events.is_empty());
}

#[tokio::test]
async fn cancelled_transport_cleanup_error_preserves_target_admission() {
    let cleanup_error = await_cancelled_transport(
        async { Err::<(), _>(anyhow::anyhow!("cleanup failed")) },
        Duration::ZERO,
    )
    .await
    .expect_err("cleanup error must propagate");
    let dropped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut admission = Some(DropProbe(std::sync::Arc::clone(&dropped)));

    let result =
        preserve_target_admission_on_cleanup_error::<(), _>(Err(cleanup_error), &mut admission);

    assert!(result.is_err());
    assert!(admission.is_none());
    assert!(!dropped.load(std::sync::atomic::Ordering::SeqCst));
}

#[cfg(unix)]
#[tokio::test]
async fn acp_initialize_codex_config_error_is_terminal_and_redacted() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().expect("tempdir");
    let fake_codex = temp.path().join("codex-acp");
    std::fs::write(
        &fake_codex,
        "#!/bin/sh\nprintf '%s\\n' 'Error loading config.toml: duplicate key' 'OPENAI_API_KEY=providerfixture12345' >&2\nexit 1\n",
    )
    .expect("write fake codex ACP");
    let mut permissions = std::fs::metadata(&fake_codex)
        .expect("fake codex metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&fake_codex, permissions).expect("make fake codex executable");

    let mut session = csa_session::MetaSessionState::default();
    session.meta_session_id = ulid::Ulid::new().to_string();
    session.project_path = temp.path().display().to_string();
    session.task_context.task_type = Some("debate".to_string());
    let executor = csa_executor::Executor::Codex {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: csa_executor::CodexRuntimeMetadata::from_transport(
            csa_executor::CodexTransport::Acp,
        ),
    };
    let path = std::env::var("PATH").unwrap_or_default();
    let extra_env = std::collections::HashMap::from([(
        "PATH".to_string(),
        format!("{}:{path}", temp.path().display()),
    )]);
    let options = csa_executor::ExecuteOptions::new(csa_process::StreamMode::BufferOnly, 1)
        .with_acp_init_timeout_seconds(2)
        .with_acp_crash_max_attempts(1);

    let error = execute_transport_with_signal(
        &executor,
        "prompt",
        None,
        &session,
        Some(&extra_env),
        options,
        None,
        temp.path(),
        &mut None,
        chrono::Utc::now(),
        None,
    )
    .await
    .expect_err("ACP initialize failure must return an error");

    let session_dir = csa_session::manager::get_session_dir(temp.path(), &session.meta_session_id)
        .expect("session directory");
    let result_path = session_dir.join(csa_session::result::RESULT_FILE_NAME);
    let result_contents = std::fs::read_to_string(&result_path).expect("persisted result");
    let result: csa_session::SessionResult = toml::from_str(&result_contents).expect("result TOML");
    assert_eq!(result.status, "failure");
    assert_eq!(result.tool, "codex");
    assert_eq!(result.exit_code, 1);
    assert!(result.summary.contains("Error loading config.toml"));
    assert!(!result.summary.contains("providerfixture12345"));
    assert!(result.summary.contains("[REDACTED]"));
    let error_chain = format!("{error:#}");
    assert!(!error_chain.contains("providerfixture12345"));
    assert!(error_chain.contains("[REDACTED]"));

    let lock_path = session_dir.join("locks/codex.lock");
    std::fs::create_dir_all(lock_path.parent().expect("lock parent")).expect("create locks");
    std::fs::write(&lock_path, format!("{{\"pid\": {}}}", std::process::id()))
        .expect("write live diagnostic lock");
    assert_eq!(
        crate::debate_errors::classify_execution_error(&error, Some(&session_dir)),
        crate::debate_errors::DebateErrorKind::Deterministic(
            "Codex config.toml parse failure".to_string()
        )
    );
}

#[cfg(unix)]
#[tokio::test]
async fn cancelled_transport_cleanup_timeout_preserves_admission_with_term_resistant_descendant() {
    let temp = tempfile::tempdir().expect("tempdir");
    let descendant_pid_file = temp.path().join("descendant.pid");
    let mut command = tokio::process::Command::new("sh");
    command
        .arg("-c")
        .arg(
            "trap '' TERM; sh -c 'trap \"\" TERM; while :; do sleep 1; done' & \
             printf '%s' \"$!\" >\"$1\"; wait",
        )
        .arg("sh")
        .arg(&descendant_pid_file)
        .process_group(0);
    let mut child = command.spawn().expect("spawn TERM-resistant process group");
    let process_group = i32::try_from(child.id().expect("child pid")).expect("pid_t");
    let descendant_pid = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if let Ok(pid) = std::fs::read_to_string(&descendant_pid_file) {
                break pid.parse::<i32>().expect("numeric descendant pid");
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("descendant ready");
    // SAFETY: process_group is the positive PID of the owned process-group leader.
    assert_eq!(unsafe { libc::kill(-process_group, libc::SIGTERM) }, 0);

    let cleanup_error = await_cancelled_transport(
        async {
            child
                .wait()
                .await
                .context("wait for TERM-resistant process group")
                .map(|_| ())
        },
        Duration::from_millis(1),
    )
    .await
    .expect_err("TERM-resistant cleanup must time out");
    let descendant_stat = std::fs::read_to_string(format!("/proc/{descendant_pid}/stat"))
        .expect("descendant remains past cleanup deadline");
    let descendant_was_live = descendant_stat.split_whitespace().nth(2) != Some("Z");
    let dropped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut admission = Some(DropProbe(std::sync::Arc::clone(&dropped)));
    let result =
        preserve_target_admission_on_cleanup_error::<(), _>(Err(cleanup_error), &mut admission);

    // SAFETY: the group is still anchored by the owned, unreaped child.
    let _ = unsafe { libc::kill(-process_group, libc::SIGKILL) };
    let _ = child.wait().await;

    assert!(descendant_was_live);
    assert!(result.is_err());
    assert!(admission.is_none());
    assert!(!dropped.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(
        timeout_transport_result(1).execution.exit_code,
        RUN_TIMEOUT_EXIT_CODE
    );
}
