//! Session subcommand dispatch — extracted from main.rs to stay under
//! the monolith file limit.

use std::io::Write;

use anyhow::Result;

use crate::cli::SessionCommands;
use crate::session_cmds;
use csa_config::GlobalConfig;
use csa_core::types::OutputFormat;

pub(crate) fn dispatch(cmd: SessionCommands, output_format: OutputFormat) -> Result<()> {
    match cmd {
        SessionCommands::List {
            cd,
            branch,
            tool,
            tree,
            all_projects,
        } => {
            session_cmds::handle_session_list(cd, branch, tool, tree, all_projects, output_format)?;
        }
        SessionCommands::Compress { session, cd } => {
            session_cmds::handle_session_compress(session, cd)?;
        }
        SessionCommands::Delete { session, cd } => {
            session_cmds::handle_session_delete(session, cd)?;
        }
        SessionCommands::Clean {
            days,
            dry_run,
            tool,
            cd,
        } => {
            session_cmds::handle_session_clean(days, dry_run, tool, cd)?;
        }
        SessionCommands::Logs {
            session,
            tail,
            events,
            cd,
        } => {
            session_cmds::handle_session_logs(session, tail, events, cd)?;
        }
        SessionCommands::IsAlive { session, cd } => {
            let alive = session_cmds::handle_session_is_alive(session, cd)?;
            let _ = std::io::stdout().flush();
            let _ = std::io::stderr().flush();
            std::process::exit(if alive { 0 } else { 1 });
        }
        SessionCommands::Result {
            session,
            json,
            summary,
            section,
            full,
            cd,
        } => {
            session_cmds::handle_session_result(
                session,
                json,
                cd,
                session_cmds::StructuredOutputOpts {
                    summary,
                    section,
                    full,
                },
            )?;
        }
        SessionCommands::Artifacts { session, cd } => {
            session_cmds::handle_session_artifacts(session, cd)?;
        }
        SessionCommands::Log { session, cd } => {
            session_cmds::handle_session_log(session, cd)?;
        }
        SessionCommands::Checkpoint { session, cd } => {
            session_cmds::handle_session_checkpoint(session, cd)?;
        }
        SessionCommands::Checkpoints { cd } => {
            session_cmds::handle_session_checkpoints(cd)?;
        }
        SessionCommands::Measure { session, json, cd } => {
            session_cmds::handle_session_measure(session, json, cd)?;
        }
        SessionCommands::ToolOutput {
            session,
            index,
            list,
            cd,
        } => {
            session_cmds::handle_session_tool_output(session, index, list, cd)?;
        }
        SessionCommands::Wait { session, cd } => {
            let wait_timeout = resolve_daemon_wait_timeout(cd.as_deref());
            let exit_code = session_cmds::handle_session_wait(session, cd, wait_timeout)?;
            let _ = std::io::stdout().flush();
            let _ = std::io::stderr().flush();
            std::process::exit(exit_code);
        }
        SessionCommands::Kill { session, cd } => {
            session_cmds::handle_session_kill(session, cd)?;
        }
        SessionCommands::Attach {
            session,
            stderr,
            cd,
        } => {
            let exit_code = session_cmds::handle_session_attach(session, stderr, cd)?;
            let _ = std::io::stdout().flush();
            let _ = std::io::stderr().flush();
            std::process::exit(exit_code);
        }
    }
    Ok(())
}

/// Resolve the `csa session wait` cap from global KV cache config.
///
/// Compatibility rule: if `[kv_cache]` is absent, keep the legacy 250s wait cap.
fn resolve_daemon_wait_timeout(cd: Option<&str>) -> u64 {
    let _ = cd;
    GlobalConfig::resolve_session_wait_long_poll_seconds()
}

#[cfg(test)]
mod tests {
    use super::resolve_daemon_wait_timeout;
    use crate::test_env_lock::TEST_ENV_LOCK;

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

    #[test]
    fn resolve_daemon_wait_timeout_uses_global_kv_cache_long_poll_seconds() {
        let _env_lock = TEST_ENV_LOCK.lock().expect("config env lock poisoned");
        let dir = tempfile::tempdir().unwrap();
        let config_root = dir.path().join("xdg-config");
        std::fs::create_dir_all(&config_root).unwrap();
        let _home_guard = EnvVarGuard::set("HOME", dir.path());
        let _xdg_guard = EnvVarGuard::set("XDG_CONFIG_HOME", &config_root);

        let global_dir = config_root.join("cli-sub-agent");
        std::fs::create_dir_all(&global_dir).unwrap();
        std::fs::write(
            global_dir.join("config.toml"),
            r#"
[kv_cache]
long_poll_seconds = 3000
"#,
        )
        .unwrap();

        assert_eq!(resolve_daemon_wait_timeout(None), 3000);
    }

    #[test]
    fn resolve_daemon_wait_timeout_keeps_legacy_fallback_without_kv_cache_section() {
        let _env_lock = TEST_ENV_LOCK.lock().expect("config env lock poisoned");
        let dir = tempfile::tempdir().unwrap();
        let config_root = dir.path().join("xdg-config");
        std::fs::create_dir_all(&config_root).unwrap();
        let _home_guard = EnvVarGuard::set("HOME", dir.path());
        let _xdg_guard = EnvVarGuard::set("XDG_CONFIG_HOME", &config_root);

        let global_dir = config_root.join("cli-sub-agent");
        std::fs::create_dir_all(&global_dir).unwrap();
        std::fs::write(
            global_dir.join("config.toml"),
            r#"
[review]
tool = "auto"
"#,
        )
        .unwrap();

        assert_eq!(resolve_daemon_wait_timeout(None), 250);
    }
}
