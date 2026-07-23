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
    wait_provider: Option<&csa_config::ModelProvider>,
) -> Result<DaemonStartedOutput> {
    crate::run_cmd_daemon::verify_daemon_session_waitable(project_root, &result.session_id)?;
    let wait_command = crate::daemon_caller_hints::resolve_session_wait_command(
        &result.session_id,
        project_root,
        wait_provider,
    );
    let wait_cmd_attr = wait_command
        .command()
        .map(crate::daemon_caller_hints::escape_structured_comment_attr)
        .unwrap_or_default();
    let wait_hint = match wait_command.command() {
        Some(wait_cmd) => {
            let wait_cmd = crate::daemon_caller_hints::escape_structured_comment_attr(wait_cmd);
            format!(
                "<!-- CSA:CALLER_HINT action=\"wait\" rule=\"Call {wait_cmd} with run_in_background: true. Task-notification is your wake signal — no polling, no loops, one wait per Bash call.\" -->"
            )
        }
        None => wait_command.provider_selection_hint(),
    };
    let attach_cmd =
        crate::daemon_caller_hints::format_session_attach_command(&result.session_id, project_root);
    let kill_cmd =
        crate::daemon_caller_hints::format_session_kill_command(&result.session_id, project_root);
    let session_dir_attr = crate::daemon_caller_hints::escape_structured_comment_attr(
        &result.session_dir.display().to_string(),
    );
    let attach_cmd_attr = crate::daemon_caller_hints::escape_structured_comment_attr(&attach_cmd);
    let kill_cmd_attr = crate::daemon_caller_hints::escape_structured_comment_attr(&kill_cmd);
    let cancellation_hint = render_wait_cancellation_hint(&result.session_id, &kill_cmd);
    let mut stderr = format!(
        "<!-- CSA:SESSION_STARTED id={id} pid={pid} dir=\"{dir}\" \
         wait_cmd=\"{wait_cmd}\" \
         attach_cmd=\"{attach_cmd}\" \
         kill_cmd=\"{kill_cmd}\" -->\n\
         {wait_hint}\n\
         {cancellation_hint}\n",
        id = result.session_id,
        pid = result.pid,
        dir = session_dir_attr,
        wait_cmd = wait_cmd_attr,
        attach_cmd = attach_cmd_attr,
        kill_cmd = kill_cmd_attr,
    );
    stderr.push_str(&crate::process_tree::codex_yield_hint(
        wait_command.command(),
    ));
    Ok(DaemonStartedOutput {
        stdout: format!("{}\n", result.session_id),
        stderr,
    })
}

fn render_wait_cancellation_hint(_session_id: &str, _kill_cmd: &str) -> String {
    // SESSION_STARTED immediately precedes this hint and holds the exact,
    // project-scoped kill command. Keep this repeat warning path-independent.
    "<!-- CSA:CALLER_HINT action=\"cancel_session\" \
     rule=\"IMPORTANT: stopping a background wait does NOT stop the session. To cancel it, explicitly run CSA:SESSION_STARTED kill_cmd.\" -->"
        .to_string()
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
    use crate::test_env_lock::{ScopedEnvVarRestore, TEST_ENV_LOCK};

    fn isolate_daemon_started_env(
        temp: &tempfile::TempDir,
    ) -> (
        ScopedEnvVarRestore,
        ScopedEnvVarRestore,
        ScopedEnvVarRestore,
        ScopedEnvVarRestore,
    ) {
        let config_home = temp.path().join("xdg-config");
        let state_home = temp.path().join("xdg-state");
        std::fs::create_dir_all(&config_home).expect("test config home should exist");
        std::fs::create_dir_all(&state_home).expect("test state home should exist");
        (
            ScopedEnvVarRestore::set("HOME", temp.path()),
            ScopedEnvVarRestore::set("XDG_CONFIG_HOME", &config_home),
            ScopedEnvVarRestore::set("XDG_STATE_HOME", &state_home),
            ScopedEnvVarRestore::unset("HERMES_MODEL_PROVIDER"),
        )
    }

    fn plan_spawn_result(
        project_root: &Path,
    ) -> (
        String,
        std::path::PathBuf,
        csa_process::daemon::DaemonSpawnResult,
    ) {
        let session_id = "01KAS6M5XG7V4M4M6YDRS7P8R9".to_string();
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
        let _lock = TEST_ENV_LOCK.clone().blocking_lock_owned();
        let temp = tempfile::tempdir().expect("tempdir");
        let project_root = temp.path().join("project");
        std::fs::create_dir_all(&project_root).expect("project root should exist");
        let (_home_guard, _config_guard, _state_guard, _provider_guard) =
            isolate_daemon_started_env(&temp);
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

        let output = prepare(&result, &project_root, None)
            .expect("waitable plan placeholder should permit marker rendering");
        assert_eq!(output.stdout, format!("{session_id}\n"));
        assert_eq!(
            output.stderr.matches("<!-- CSA:SESSION_STARTED ").count(),
            1,
            "exactly one structured session-start marker must be emitted"
        );
        assert!(output.stderr.contains("CSA:CALLER_HINT"));
        let _ = std::fs::remove_dir_all(session_root);
    }

    #[test]
    fn root_initial_wait_hint_propagates_normalized_explicit_provider() {
        let _lock = TEST_ENV_LOCK.clone().blocking_lock_owned();
        let temp = tempfile::tempdir().expect("tempdir");
        let project_root = temp.path().join("project");
        std::fs::create_dir_all(&project_root).expect("create project root");
        let (_home_guard, _config_guard, _state_guard, _provider_guard) =
            isolate_daemon_started_env(&temp);
        let _ambient_provider = ScopedEnvVarRestore::set("HERMES_MODEL_PROVIDER", "custom");
        let config_path =
            csa_config::ProjectConfig::user_config_path().expect("resolve user config path");
        std::fs::create_dir_all(config_path.parent().expect("config parent"))
            .expect("create config parent");
        std::fs::write(config_path, "[kv_cache.provider_ttls]\nxai = 17\n")
            .expect("write provider config");
        let startup_env =
            crate::startup_env::StartupSubtreeEnv::from_values(std::collections::HashMap::new());
        let provider = crate::daemon_caller_hints::explicit_wait_provider_from_launch_routing(
            Some("codex/XAI/gpt-5.5/xhigh"),
            &startup_env,
        )
        .expect("explicit root model spec must carry a provider");
        let (session_id, session_root, result) = plan_spawn_result(&project_root);
        csa_session::create_session_with_daemon_env(
            &project_root,
            Some("run: explicit provider"),
            None,
            None,
            Some(&session_id),
            Some(&result.session_dir),
            Some(&project_root),
        )
        .expect("run placeholder should persist");

        let output =
            prepare(&result, &project_root, Some(&provider)).expect("render root started marker");

        let expected_wait_command = format!(
            "wait_cmd=\"csa session wait --session {session_id} --model-provider xai --cd '{}'\"",
            project_root.display(),
        );
        assert!(
            output.stderr.contains(&expected_wait_command),
            "root initial wait hint must preserve its normalized launch provider: {:#?}",
            output.stderr
        );
        assert!(
            !output.stderr.contains("--model-provider custom"),
            "root initial wait hint must not use the ambient provider: {:#?}",
            output.stderr
        );
        let _ = std::fs::remove_dir_all(session_root);
    }

    const CALLER_HINT_MAX_BYTES: usize = 300;

    #[test]
    fn wait_cancellation_hint_uses_started_marker_kill_command_within_budget() {
        let session_id = "01KAS6M5XG7V4M4M6YDRS7P8R9";
        let kill_cmd = crate::daemon_caller_hints::format_session_kill_command(
            session_id,
            Path::new("/home/obj/project/github/RyderFreeman4Logos/cli-sub-agent"),
        );

        let hint = render_wait_cancellation_hint(session_id, &kill_cmd);

        assert!(hint.contains("action=\"cancel_session\""), "{hint}");
        assert!(!hint.contains(session_id), "{hint}");
        assert!(!hint.contains(&kill_cmd), "{hint}");
        assert!(hint.contains("does NOT stop the session"), "{hint}");
        assert!(hint.contains("CSA:SESSION_STARTED"), "{hint}");
        let rendered_bytes = hint.as_bytes().len();
        assert!(
            rendered_bytes <= CALLER_HINT_MAX_BYTES,
            "rendered cancellation hint is {} bytes, exceeds the {} byte budget: {hint}",
            rendered_bytes,
            CALLER_HINT_MAX_BYTES,
        );
    }

    #[test]
    fn wait_cancellation_hint_excludes_escaped_special_project_path_within_budget() {
        let session_id = "01KAS6M5XG7V4M4M6YDRS7P8R9";
        let kill_cmd = crate::daemon_caller_hints::format_session_kill_command(
            session_id,
            Path::new(
                "/home/obj/project/github/RyderFreeman4Logos/cli-sub-agent/it's-\"quoted\"&<special>",
            ),
        );
        let escaped_kill_cmd =
            crate::daemon_caller_hints::escape_structured_comment_attr(&kill_cmd);

        let hint = render_wait_cancellation_hint(session_id, &kill_cmd);

        assert!(
            escaped_kill_cmd.contains("&quot;")
                && escaped_kill_cmd.contains("&amp;")
                && escaped_kill_cmd.contains("&lt;")
                && escaped_kill_cmd.contains("&gt;"),
            "special project path must be escaped in the durable started marker: {escaped_kill_cmd}"
        );
        assert!(!hint.contains(&escaped_kill_cmd), "{hint}");
        assert!(hint.contains("does NOT stop the session"), "{hint}");
        let rendered_bytes = hint.as_bytes().len();
        assert!(
            rendered_bytes <= CALLER_HINT_MAX_BYTES,
            "rendered cancellation hint is {} bytes, exceeds the {} byte budget: {hint}",
            rendered_bytes,
            CALLER_HINT_MAX_BYTES,
        );
    }

    #[test]
    fn started_marker_fails_closed_when_no_configured_provider_matches_context() {
        let _lock = TEST_ENV_LOCK.clone().blocking_lock_owned();
        let temp = tempfile::tempdir().expect("tempdir");
        let project_root = temp.path().join("project");
        std::fs::create_dir_all(&project_root).expect("create project root");
        let (_home_guard, _config_guard, _state_guard, _provider_guard) =
            isolate_daemon_started_env(&temp);
        let _ambient_provider = ScopedEnvVarRestore::set("HERMES_MODEL_PROVIDER", "custom");
        let config_path =
            csa_config::ProjectConfig::user_config_path().expect("resolve user config path");
        std::fs::create_dir_all(config_path.parent().expect("config parent"))
            .expect("create config parent");
        std::fs::write(config_path, "[kv_cache.provider_ttls]\ncustom = 17\n")
            .expect("write provider config");
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

        let output = prepare(&result, &project_root, None).expect("render started marker");

        assert!(
            output
                .stderr
                .contains("CSA:CALLER_HINT action=\"select_wait_provider\""),
            "{:#?}",
            output.stderr
        );
        assert!(
            output.stderr.contains("legal_keys=\"custom\""),
            "{:#?}",
            output.stderr
        );
        assert!(
            !output.stderr.contains("wait_cmd=\"csa session wait"),
            "a providerless command must not be emitted: {:#?}",
            output.stderr
        );
        let _ = std::fs::remove_dir_all(session_root);
    }

    #[test]
    fn started_marker_is_suppressed_when_placeholder_is_unreadable() {
        let _lock = TEST_ENV_LOCK.clone().blocking_lock_owned();
        let temp = tempfile::tempdir().expect("tempdir");
        let project_root = temp.path().join("project");
        std::fs::create_dir_all(&project_root).expect("project root should exist");
        let (_home_guard, _config_guard, _state_guard, _provider_guard) =
            isolate_daemon_started_env(&temp);
        let (_session_id, session_root, result) = plan_spawn_result(&project_root);
        std::fs::create_dir_all(&result.session_dir)
            .expect("session dir should exist without state.toml");

        prepare(&result, &project_root, None)
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
