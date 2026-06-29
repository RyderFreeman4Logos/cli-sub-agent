//! Session subcommand dispatch — extracted from main.rs to stay under
//! the monolith file limit.

use std::io::Write;

use anyhow::Result;

use crate::cli::SessionCommands;
use crate::session_cmds;
use crate::startup_env::StartupSubtreeEnv;
use csa_config::{
    GlobalConfig, KvCacheValueSource, ModelProvider, ProjectConfig, detect_model_provider,
    provider_ttl,
};
use csa_core::types::OutputFormat;

/// Resolve session ID from positional arg or --session flag.
/// Positional takes precedence when both are provided.
fn resolve_session_id(positional: Option<String>, flag: Option<String>) -> Result<String> {
    positional
        .or(flag)
        .ok_or_else(|| anyhow::anyhow!("session ID is required (positional or --session)"))
}

pub(crate) fn dispatch(
    cmd: SessionCommands,
    output_format: OutputFormat,
    startup_env: &StartupSubtreeEnv,
) -> Result<()> {
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
            csa_version,
            show_version,
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
                    csa_version,
                    show_version,
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
        SessionCommands::Peek {
            session_id,
            session,
            operations,
            cd,
        } => {
            let sid = resolve_session_id(session_id, session)?;
            session_cmds::handle_session_peek(sid, Some(operations), cd, output_format)?;
        }
        SessionCommands::Stats {
            since,
            by_issue,
            by_tool,
            cost,
            cd,
        } => {
            session_cmds::handle_session_stats(since, by_issue, by_tool, cost, cd, output_format)?;
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
        SessionCommands::Checkpoint {
            session_id,
            session,
            cd,
            all,
        } => {
            let sid = resolve_session_id(session_id, session)?;
            let found = session_cmds::handle_session_checkpoint(sid, all, cd)?;
            let _ = std::io::stdout().flush();
            let _ = std::io::stderr().flush();
            std::process::exit(if found { 0 } else { 1 });
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
            memory_warn_mb,
            model_provider,
            verbose,
            json,
            cd,
        } => {
            let sid = resolve_session_id(session_id, session)?;
            let wait_timeout = resolve_wait_ttl(model_provider, cd.as_deref());
            let resolved_memory_warn_mb =
                resolve_session_wait_memory_warn_mb(memory_warn_mb, cd.as_deref());
            let output_mode = session_cmds::SessionWaitOutputMode::from_flags(verbose, json);
            let exit_code = session_cmds::handle_session_wait_with_options(
                sid,
                cd,
                wait_timeout,
                resolved_memory_warn_mb,
                output_mode,
            )?;
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
            prompt,
            prompt_flag,
            prompt_file,
            stderr,
            cd,
        } => {
            let sid = resolve_session_id(session_id, session)?;
            let exit_code = if prompt.is_none() && prompt_flag.is_none() && prompt_file.is_none() {
                session_cmds::handle_session_attach(sid, stderr, cd, startup_env)?
            } else {
                session_cmds::handle_session_attach_with_prompt(
                    sid,
                    stderr,
                    cd,
                    prompt,
                    prompt_flag,
                    prompt_file,
                    startup_env,
                )?
            };
            let _ = std::io::stdout().flush();
            let _ = std::io::stderr().flush();
            std::process::exit(exit_code);
        }
    }
    Ok(())
}

/// Resolve the `csa session wait` cap from provider-aware KV cache config.
///
/// CLI provider override wins over best-effort provider detection. If no provider
/// can be detected, fall back to the global default TTL and then the deprecated
/// project/user `session.daemon_wait_seconds` compatibility key.
fn resolve_wait_ttl(cli_model_provider: Option<ModelProvider>, cd: Option<&str>) -> u64 {
    if let Some(provider) = cli_model_provider.or_else(detect_model_provider) {
        let config = match GlobalConfig::load() {
            Ok(config) => config,
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "Failed to load global config while resolving provider wait TTL; using defaults"
                );
                GlobalConfig::default()
            }
        };
        return provider_ttl(provider, &config.kv_cache);
    }

    resolve_daemon_wait_timeout(cd)
}

/// Resolve the `csa session wait` fallback cap from global KV cache config.
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

fn resolve_session_wait_memory_warn_mb(cli_override: Option<u64>, cd: Option<&str>) -> Option<u64> {
    if let Some(limit_mb) = cli_override {
        return (limit_mb > 0).then_some(limit_mb);
    }

    let project_root = crate::pipeline::determine_project_root(cd).ok()?;
    ProjectConfig::resolve_session_wait_memory_warn_mb(&project_root)
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
        "Using deprecated session.daemon_wait_seconds; migrate to global kv_cache.default_ttl_seconds"
    );
    Some(timeout)
}

#[cfg(test)]
mod tests {
    use super::{resolve_daemon_wait_timeout, resolve_wait_ttl};
    use crate::test_env_lock::TEST_ENV_LOCK;
    use csa_config::ModelProvider;

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

        fn remove(key: &'static str) -> Self {
            let original = std::env::var(key).ok();
            // SAFETY: test-scoped env mutation guarded by a process-wide mutex.
            unsafe { std::env::remove_var(key) };
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

    fn write_user_config(contents: &str) {
        let global_path =
            csa_config::ProjectConfig::user_config_path().expect("resolve user config path");
        std::fs::create_dir_all(global_path.parent().expect("config parent")).unwrap();
        std::fs::write(global_path, contents).unwrap();
    }

    #[test]
    fn resolve_wait_ttl_uses_cli_model_provider_override() {
        let _env_lock = TEST_ENV_LOCK.blocking_lock();
        let dir = tempfile::tempdir().unwrap();
        let config_root = dir.path().join("xdg-config");
        std::fs::create_dir_all(&config_root).unwrap();
        let _home_guard = EnvVarGuard::set("HOME", dir.path());
        let _xdg_guard = EnvVarGuard::set("XDG_CONFIG_HOME", &config_root);

        write_user_config(
            r#"
[kv_cache]
default_ttl_seconds = 555

[kv_cache.provider_ttls]
openai = 1666
"#,
        );

        assert_eq!(resolve_wait_ttl(Some(ModelProvider::OpenAI), None), 1666);
    }

    #[test]
    fn resolve_wait_ttl_falls_back_to_default_ttl_without_provider() {
        let _env_lock = TEST_ENV_LOCK.blocking_lock();
        let dir = tempfile::tempdir().unwrap();
        let config_root = dir.path().join("xdg-config");
        std::fs::create_dir_all(&config_root).unwrap();
        let _provider_guard = EnvVarGuard::remove("HERMES_MODEL_PROVIDER");
        let _home_guard = EnvVarGuard::set("HOME", dir.path());
        let _xdg_guard = EnvVarGuard::set("XDG_CONFIG_HOME", &config_root);

        write_user_config(
            r#"
[kv_cache]
default_ttl_seconds = 555
"#,
        );

        assert_eq!(resolve_wait_ttl(None, None), 555);
    }

    #[test]
    fn resolve_daemon_wait_timeout_uses_global_kv_cache_long_poll_seconds() {
        let _env_lock = TEST_ENV_LOCK.blocking_lock();
        let dir = tempfile::tempdir().unwrap();
        let config_root = dir.path().join("xdg-config");
        std::fs::create_dir_all(&config_root).unwrap();
        let _home_guard = EnvVarGuard::set("HOME", dir.path());
        let _xdg_guard = EnvVarGuard::set("XDG_CONFIG_HOME", &config_root);

        write_user_config(
            r#"
[kv_cache]
long_poll_seconds = 3000
"#,
        );

        assert_eq!(resolve_daemon_wait_timeout(None), 3000);
    }

    #[test]
    fn resolve_daemon_wait_timeout_uses_documented_default_without_kv_cache_section() {
        let _env_lock = TEST_ENV_LOCK.blocking_lock();
        let dir = tempfile::tempdir().unwrap();
        let config_root = dir.path().join("xdg-config");
        std::fs::create_dir_all(&config_root).unwrap();
        let _home_guard = EnvVarGuard::set("HOME", dir.path());
        let _xdg_guard = EnvVarGuard::set("XDG_CONFIG_HOME", &config_root);

        write_user_config(
            r#"
[review]
tool = "auto"
"#,
        );

        assert_eq!(resolve_daemon_wait_timeout(None), 240);
    }

    #[test]
    fn resolve_daemon_wait_timeout_prefers_global_kv_cache_over_legacy_session_key() {
        let _env_lock = TEST_ENV_LOCK.blocking_lock();
        let dir = tempfile::tempdir().unwrap();
        let config_root = dir.path().join("xdg-config");
        std::fs::create_dir_all(&config_root).unwrap();
        let _home_guard = EnvVarGuard::set("HOME", dir.path());
        let _xdg_guard = EnvVarGuard::set("XDG_CONFIG_HOME", &config_root);

        write_user_config(
            r#"
[kv_cache]
long_poll_seconds = 3000
"#,
        );

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
        let _env_lock = TEST_ENV_LOCK.blocking_lock();
        let dir = tempfile::tempdir().unwrap();
        let config_root = dir.path().join("xdg-config");
        std::fs::create_dir_all(&config_root).unwrap();
        let _home_guard = EnvVarGuard::set("HOME", dir.path());
        let _xdg_guard = EnvVarGuard::set("XDG_CONFIG_HOME", &config_root);

        write_user_config(
            r#"
[kv_cache]
long_poll_seconds = 240
"#,
        );

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
        let _env_lock = TEST_ENV_LOCK.blocking_lock();
        let dir = tempfile::tempdir().unwrap();
        let config_root = dir.path().join("xdg-config");
        std::fs::create_dir_all(&config_root).unwrap();
        let _home_guard = EnvVarGuard::set("HOME", dir.path());
        let _xdg_guard = EnvVarGuard::set("XDG_CONFIG_HOME", &config_root);

        write_user_config(
            r#"
[kv_cache]
frequent_poll_seconds = 45
"#,
        );

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
        let _env_lock = TEST_ENV_LOCK.blocking_lock();
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
        let _env_lock = TEST_ENV_LOCK.blocking_lock();
        let dir = tempfile::tempdir().unwrap();
        let config_root = dir.path().join("xdg-config");
        std::fs::create_dir_all(&config_root).unwrap();
        let _home_guard = EnvVarGuard::set("HOME", dir.path());
        let _xdg_guard = EnvVarGuard::set("XDG_CONFIG_HOME", &config_root);

        write_user_config(
            r#"
schema_version = 1
[session]
daemon_wait_seconds = 480
"#,
        );

        assert_eq!(
            resolve_daemon_wait_timeout(Some(dir.path().to_str().unwrap())),
            480
        );
    }
}
