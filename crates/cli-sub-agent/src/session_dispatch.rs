//! Session subcommand dispatch — extracted from main.rs to stay under
//! the monolith file limit.

use std::io::Write;

use anyhow::Result;

use crate::cli::SessionCommands;
use crate::session_cmds;
use csa_config::{GlobalConfig, KvCacheValueSource, ProjectConfig};
use csa_core::types::OutputFormat;

/// Resolve session ID from positional arg or --session flag.
/// Positional takes precedence when both are provided.
fn resolve_session_id(positional: Option<String>, flag: Option<String>) -> Result<String> {
    positional
        .or(flag)
        .ok_or_else(|| anyhow::anyhow!("session ID is required (positional or --session)"))
}

pub(crate) fn dispatch(cmd: SessionCommands, output_format: OutputFormat) -> Result<()> {
    match cmd {
        SessionCommands::List {
            cd,
            branch,
            tool,
            tree,
            all_projects,
            limit,
            since,
            status,
        } => {
            session_cmds::handle_session_list(
                cd,
                branch,
                tool,
                tree,
                all_projects,
                session_cmds::SessionListFilters {
                    limit,
                    since,
                    status,
                },
                output_format,
            )?;
        }
        SessionCommands::Compress {
            session_id,
            session,
            cd,
        } => {
            let sid = resolve_session_id(session_id, session)?;
            session_cmds::handle_session_compress(sid, cd)?;
        }
        SessionCommands::Delete {
            session_id,
            session,
            cd,
        } => {
            let sid = resolve_session_id(session_id, session)?;
            session_cmds::handle_session_delete(sid, cd)?;
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
            session_id,
            session,
            tail,
            events,
            cd,
        } => {
            let sid = resolve_session_id(session_id, session)?;
            session_cmds::handle_session_logs(sid, tail, events, cd)?;
        }
        SessionCommands::IsAlive {
            session_id,
            session,
            cd,
        } => {
            let sid = resolve_session_id(session_id, session)?;
            let alive = session_cmds::handle_session_is_alive(sid, cd)?;
            let _ = std::io::stdout().flush();
            let _ = std::io::stderr().flush();
            std::process::exit(if alive { 0 } else { 1 });
        }
        SessionCommands::Result {
            session_id,
            session,
            json,
            summary,
            section,
            full,
            cd,
        } => {
            let sid = resolve_session_id(session_id, session)?;
            session_cmds::handle_session_result(
                sid,
                json,
                cd,
                session_cmds::StructuredOutputOpts {
                    summary,
                    section,
                    full,
                },
            )?;
        }
        SessionCommands::Artifacts {
            session_id,
            session,
            cd,
        } => {
            let sid = resolve_session_id(session_id, session)?;
            session_cmds::handle_session_artifacts(sid, cd)?;
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
        SessionCommands::Wait {
            session_id,
            session,
            cd,
        } => {
            let sid = resolve_session_id(session_id, session)?;
            let wait_timeout = resolve_daemon_wait_timeout(cd.as_deref());
            let exit_code = session_cmds::handle_session_wait(sid, cd, wait_timeout)?;
            let _ = std::io::stdout().flush();
            let _ = std::io::stderr().flush();
            std::process::exit(exit_code);
        }
        SessionCommands::Kill {
            session_id,
            session,
            cd,
        } => {
            let sid = resolve_session_id(session_id, session)?;
            session_cmds::handle_session_kill(sid, cd)?;
        }
        SessionCommands::Attach {
            session_id,
            session,
            stderr,
            cd,
        } => {
            let sid = resolve_session_id(session_id, session)?;
            let exit_code = session_cmds::handle_session_attach(sid, stderr, cd)?;
            let _ = std::io::stdout().flush();
            let _ = std::io::stderr().flush();
            std::process::exit(exit_code);
        }
    }
    Ok(())
}

/// Resolve the `csa session wait` cap from global KV cache config.
///
/// Use the global KV cache setting when it differs from the documented default.
/// Deprecated `session.daemon_wait_seconds` remains a compatibility fallback.
fn resolve_daemon_wait_timeout(cd: Option<&str>) -> u64 {
    let global_timeout = GlobalConfig::resolve_session_wait_long_poll_seconds_with_source();
    if !matches!(global_timeout.source, KvCacheValueSource::DocumentedDefault) {
        return global_timeout.seconds;
    }

    resolve_legacy_session_wait_timeout(cd).unwrap_or(global_timeout.seconds)
}

fn resolve_legacy_session_wait_timeout(cd: Option<&str>) -> Option<u64> {
    let project_root = crate::pipeline::determine_project_root(cd).ok();
    let project_path = project_root
        .as_deref()
        .map(ProjectConfig::config_path)
        .filter(|path| path.exists());
    let user_path = ProjectConfig::user_config_path().filter(|path| path.exists());

    if let Some(timeout) = project_path
        .as_deref()
        .and_then(|path| read_legacy_session_wait_timeout(path, "project"))
    {
        return Some(timeout);
    }

    user_path
        .as_deref()
        .and_then(|path| read_legacy_session_wait_timeout(path, "user"))
}

fn read_legacy_session_wait_timeout(path: &std::path::Path, source: &str) -> Option<u64> {
    let content = std::fs::read_to_string(path).ok()?;
    let raw: toml::Value = toml::from_str(&content).ok()?;
    let value = raw
        .get("session")
        .and_then(|session| session.get("daemon_wait_seconds"))
        .and_then(toml::Value::as_integer)?;

    if value <= 0 {
        tracing::warn!(
            path = %path.display(),
            source,
            "Ignoring deprecated session.daemon_wait_seconds because it is not > 0"
        );
        return None;
    }

    let timeout = value as u64;
    tracing::warn!(
        path = %path.display(),
        source,
        timeout,
        "Using deprecated session.daemon_wait_seconds; migrate to global kv_cache.long_poll_seconds"
    );
    Some(timeout)
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
    fn resolve_daemon_wait_timeout_uses_documented_default_without_kv_cache_section() {
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

        assert_eq!(resolve_daemon_wait_timeout(None), 240);
    }

    #[test]
    fn resolve_daemon_wait_timeout_prefers_global_kv_cache_over_legacy_session_key() {
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

        let csa_dir = dir.path().join(".csa");
        std::fs::create_dir_all(&csa_dir).unwrap();
        std::fs::write(
            csa_dir.join("config.toml"),
            r#"
schema_version = 1
[session]
daemon_wait_seconds = 600
"#,
        )
        .unwrap();

        assert_eq!(
            resolve_daemon_wait_timeout(Some(dir.path().to_str().unwrap())),
            3000
        );
    }

    #[test]
    fn resolve_daemon_wait_timeout_treats_explicit_default_as_higher_priority_than_legacy_key() {
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
long_poll_seconds = 240
"#,
        )
        .unwrap();

        let csa_dir = dir.path().join(".csa");
        std::fs::create_dir_all(&csa_dir).unwrap();
        std::fs::write(
            csa_dir.join("config.toml"),
            r#"
schema_version = 1
[session]
daemon_wait_seconds = 600
"#,
        )
        .unwrap();

        assert_eq!(
            resolve_daemon_wait_timeout(Some(dir.path().to_str().unwrap())),
            240
        );
    }

    #[test]
    fn resolve_daemon_wait_timeout_treats_kv_cache_section_default_as_higher_priority_than_legacy_key()
     {
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
frequent_poll_seconds = 45
"#,
        )
        .unwrap();

        let csa_dir = dir.path().join(".csa");
        std::fs::create_dir_all(&csa_dir).unwrap();
        std::fs::write(
            csa_dir.join("config.toml"),
            r#"
schema_version = 1
[session]
daemon_wait_seconds = 600
"#,
        )
        .unwrap();

        assert_eq!(
            resolve_daemon_wait_timeout(Some(dir.path().to_str().unwrap())),
            240
        );
    }

    #[test]
    fn resolve_daemon_wait_timeout_honors_legacy_project_session_override_with_warning_path() {
        let _env_lock = TEST_ENV_LOCK.lock().expect("config env lock poisoned");
        let dir = tempfile::tempdir().unwrap();
        let config_root = dir.path().join("xdg-config");
        std::fs::create_dir_all(&config_root).unwrap();
        let _home_guard = EnvVarGuard::set("HOME", dir.path());
        let _xdg_guard = EnvVarGuard::set("XDG_CONFIG_HOME", &config_root);

        let csa_dir = dir.path().join(".csa");
        std::fs::create_dir_all(&csa_dir).unwrap();
        std::fs::write(
            csa_dir.join("config.toml"),
            r#"
schema_version = 1
[session]
daemon_wait_seconds = 600
"#,
        )
        .unwrap();

        assert_eq!(
            resolve_daemon_wait_timeout(Some(dir.path().to_str().unwrap())),
            600
        );
    }

    #[test]
    fn resolve_daemon_wait_timeout_honors_legacy_user_session_override_when_project_missing() {
        let _env_lock = TEST_ENV_LOCK.lock().expect("config env lock poisoned");
        let dir = tempfile::tempdir().unwrap();
        let config_root = dir.path().join("xdg-config");
        std::fs::create_dir_all(&config_root).unwrap();
        let _home_guard = EnvVarGuard::set("HOME", dir.path());
        let _xdg_guard = EnvVarGuard::set("XDG_CONFIG_HOME", &config_root);

        let global_path =
            csa_config::ProjectConfig::user_config_path().expect("resolve user config path");
        std::fs::create_dir_all(global_path.parent().expect("config parent")).unwrap();
        std::fs::write(
            global_path,
            r#"
schema_version = 1
[session]
daemon_wait_seconds = 480
"#,
        )
        .unwrap();

        assert_eq!(
            resolve_daemon_wait_timeout(Some(dir.path().to_str().unwrap())),
            480
        );
    }
}
