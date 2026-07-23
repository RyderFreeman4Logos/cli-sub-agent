//! Session subcommand dispatch — extracted from main.rs to stay under
//! the monolith file limit.

use std::io::Write;

use anyhow::Result;

use crate::cli::SessionCommands;
use crate::session_cmds;
use crate::startup_env::StartupSubtreeEnv;
use csa_config::{GlobalConfig, ModelProvider, ProjectConfig, detect_model_provider, provider_ttl};
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
    wait_caller_identity: session_cmds::WaitCallerIdentity,
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
            let wait_caller_identity = wait_caller_identity.validate_for_wait()?;
            let sid = resolve_session_id(session_id, session)?;
            let (wait_model_provider, wait_timeout) =
                resolve_wait_provider_and_ttl(model_provider)?;
            let resolved_memory_warn_mb =
                resolve_session_wait_memory_warn_mb(memory_warn_mb, cd.as_deref());
            let output_mode = session_cmds::SessionWaitOutputMode::from_flags(verbose, json);
            let exit_code = session_cmds::handle_session_wait_with_options(
                sid,
                cd,
                wait_timeout,
                resolved_memory_warn_mb,
                output_mode,
                wait_caller_identity,
                Some(wait_model_provider),
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
/// CLI provider override wins over best-effort provider detection. Both paths
/// must resolve to a configured `[kv_cache.provider_ttls]` key with TTL > 0.
#[cfg(test)]
fn resolve_wait_ttl(cli_model_provider: Option<ModelProvider>) -> Result<u64> {
    Ok(resolve_wait_provider_and_ttl(cli_model_provider)?.1)
}

fn resolve_wait_provider_and_ttl(
    cli_model_provider: Option<ModelProvider>,
) -> Result<(ModelProvider, u64)> {
    let detected_provider = detect_model_provider();
    let config = match GlobalConfig::load() {
        Ok(config) => config,
        Err(error) => {
            return Err(wait_provider_error(
                cli_model_provider.as_ref(),
                detected_provider.as_ref(),
                None,
                Some(&error),
            ));
        }
    };

    if let Some(provider) = cli_model_provider.as_ref() {
        return provider_ttl(provider, &config.kv_cache)
            .map(|ttl| (provider.clone(), ttl))
            .ok_or_else(|| {
                wait_provider_error(
                    Some(provider),
                    detected_provider.as_ref(),
                    Some(&config),
                    None,
                )
            });
    }

    if let Some(provider) = detected_provider.as_ref()
        && let Some(ttl) = provider_ttl(provider, &config.kv_cache)
    {
        return Ok((provider.clone(), ttl));
    }

    Err(wait_provider_error(
        None,
        detected_provider.as_ref(),
        Some(&config),
        None,
    ))
}

fn wait_provider_error(
    requested_provider: Option<&ModelProvider>,
    detected_provider: Option<&ModelProvider>,
    config: Option<&GlobalConfig>,
    config_error: Option<&anyhow::Error>,
) -> anyhow::Error {
    use std::fmt::Write as _;

    let config_path = ProjectConfig::user_config_path()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "unavailable global config path".to_string());
    let mut message = format!(
        "csa session wait requires --model-provider <key>.\n\
         Configured keys (from {config_path} [kv_cache.provider_ttls]):\n"
    );

    let mut legal_key_count = 0;
    if let Some(config) = config {
        for (provider, ttl) in &config.kv_cache.provider_ttls.0 {
            if *ttl > 0 {
                let _ = writeln!(message, "  {provider}={ttl}");
                legal_key_count += 1;
            }
        }
    }
    if legal_key_count == 0 {
        message.push_str("  (none with TTL > 0)\n");
    }

    if let Some(provider) = requested_provider {
        let _ = writeln!(
            message,
            "Requested key: {} (missing from provider_ttls or TTL is not > 0).",
            provider.as_str()
        );
    }
    match detected_provider {
        Some(provider) => {
            let configured = config
                .and_then(|config| provider_ttl(provider, &config.kv_cache))
                .is_some();
            let status = if configured {
                "configured"
            } else {
                "not a configured key with TTL > 0"
            };
            let _ = writeln!(
                message,
                "Detected hints (best-effort, not authoritative): {} ({status}).",
                provider.as_str()
            );
        }
        None => message.push_str("Detected hints (best-effort, not authoritative): none.\n"),
    }
    if let Some(error) = config_error {
        let _ = writeln!(message, "Config load error: {error:#}");
    }
    message.push_str(
        "Action: re-run with --model-provider <one configured key> matching the caller model provider.\n\
         Do not omit this flag; provider can change mid-session when the user switches models.\n\
         <!-- CSA:CALLER_HINT action=\"select_wait_provider\" rule=\"Derive the caller model provider dynamically on every wait; pass --model-provider with a configured provider_ttls key.\" -->",
    );
    anyhow::anyhow!(message)
}

fn resolve_session_wait_memory_warn_mb(cli_override: Option<u64>, cd: Option<&str>) -> Option<u64> {
    if let Some(limit_mb) = cli_override {
        return (limit_mb > 0).then_some(limit_mb);
    }

    let project_root = crate::pipeline::determine_project_root(cd).ok()?;
    ProjectConfig::resolve_session_wait_memory_warn_mb(&project_root)
}

#[cfg(test)]
mod tests {
    use super::resolve_wait_ttl;
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

        assert_eq!(
            resolve_wait_ttl(Some(ModelProvider::new("openai"))).unwrap(),
            1666
        );
    }

    #[test]
    fn resolve_wait_ttl_fails_closed_without_provider() {
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

[kv_cache.provider_ttls]
custom = 1666
"#,
        );

        let error = resolve_wait_ttl(None).expect_err("missing provider must fail closed");
        let message = error.to_string();
        assert!(message.contains("requires --model-provider <key>"));
        assert!(message.contains("custom=1666"));
        assert!(
            !message.contains("capped at 3000 seconds"),
            "explicit-only provider_ttls must not inherit an implicit clamped default: {message}"
        );
        assert!(message.contains("CSA:CALLER_HINT"));
        assert!(message.contains("dynamically on every wait"));
        assert!(!message.contains("default_ttl_seconds = 555"));
    }

    #[test]
    fn resolve_wait_ttl_rejects_unconfigured_and_zero_ttl_providers() {
        let _env_lock = TEST_ENV_LOCK.blocking_lock();
        let dir = tempfile::tempdir().unwrap();
        let config_root = dir.path().join("xdg-config");
        std::fs::create_dir_all(&config_root).unwrap();
        let _provider_guard = EnvVarGuard::remove("HERMES_MODEL_PROVIDER");
        let _home_guard = EnvVarGuard::set("HOME", dir.path());
        let _xdg_guard = EnvVarGuard::set("XDG_CONFIG_HOME", &config_root);

        write_user_config(
            r#"
[kv_cache.provider_ttls]
custom = 1666
disabled = 0
"#,
        );

        for provider in ["missing", "disabled"] {
            let error = resolve_wait_ttl(Some(ModelProvider::new(provider)))
                .expect_err("provider must be configured with TTL > 0");
            let message = error.to_string();
            assert!(message.contains(&format!("Requested key: {provider}")));
            assert!(message.contains("custom=1666"));
            assert!(!message.contains("disabled=0"));
        }
    }

    #[test]
    fn resolve_wait_ttl_accepts_only_a_detected_configured_provider() {
        let _env_lock = TEST_ENV_LOCK.blocking_lock();
        let dir = tempfile::tempdir().unwrap();
        let config_root = dir.path().join("xdg-config");
        std::fs::create_dir_all(&config_root).unwrap();
        let _provider_guard = EnvVarGuard::set("HERMES_MODEL_PROVIDER", "custom");
        let _home_guard = EnvVarGuard::set("HOME", dir.path());
        let _xdg_guard = EnvVarGuard::set("XDG_CONFIG_HOME", &config_root);

        write_user_config(
            r#"
[kv_cache.provider_ttls]
custom = 1666
"#,
        );

        assert_eq!(resolve_wait_ttl(None).unwrap(), 1666);
    }

    #[test]
    fn resolve_wait_ttl_maps_openai_codex_hint_to_openai() {
        let _env_lock = TEST_ENV_LOCK.blocking_lock();
        let dir = tempfile::tempdir().unwrap();
        let config_root = dir.path().join("xdg-config");
        std::fs::create_dir_all(&config_root).unwrap();
        let _provider_guard = EnvVarGuard::set("HERMES_MODEL_PROVIDER", "openai-codex");
        let _home_guard = EnvVarGuard::set("HOME", dir.path());
        let _xdg_guard = EnvVarGuard::set("XDG_CONFIG_HOME", &config_root);

        write_user_config(
            r#"
[kv_cache.provider_ttls]
openai = 1666
"#,
        );

        assert_eq!(resolve_wait_ttl(None).unwrap(), 1666);
    }
}
