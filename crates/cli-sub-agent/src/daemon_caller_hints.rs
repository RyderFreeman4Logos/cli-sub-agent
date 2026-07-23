use std::path::Path;

use csa_config::{GlobalConfig, ModelProvider, provider_ttl};

pub(crate) struct SessionWaitCommand {
    command: Option<String>,
    provider: Option<String>,
    legal_provider_keys: Vec<String>,
}

impl SessionWaitCommand {
    pub(crate) fn command(&self) -> Option<&str> {
        self.command.as_deref()
    }

    /// Render the structured host contract for a validated session-wait command.
    pub(crate) fn caller_hint(&self, action: &str) -> Option<String> {
        self.provider
            .as_deref()
            .map(|provider| render_session_wait_caller_hint(action, provider))
    }

    pub(crate) fn provider_selection_hint(&self) -> String {
        let legal_keys = if self.legal_provider_keys.is_empty() {
            "none".to_string()
        } else {
            self.legal_provider_keys.join(",")
        };
        let legal_keys = escape_structured_comment_attr(&legal_keys);
        format!(
            "<!-- CSA:CALLER_HINT action=\"select_wait_provider\" legal_keys=\"{legal_keys}\" \
             rule=\"Do not issue a bare session wait. Derive the caller provider and pass --model-provider with one legal configured key.\" -->"
        )
    }
}

/// Resolve a provider-qualified `csa session wait` command for caller hints.
///
/// A command is emitted only for the explicit launch or wait provider when it
/// has a positive TTL in the active user configuration. Otherwise the caller
/// must choose from the legal keys carried by
/// [`SessionWaitCommand::provider_selection_hint`].
pub(crate) fn resolve_session_wait_command(
    session_id: &str,
    project_root: &Path,
    preferred_provider: Option<&ModelProvider>,
) -> SessionWaitCommand {
    let config = GlobalConfig::load().ok();
    let legal_provider_keys = config
        .as_ref()
        .map(|config| {
            config
                .kv_cache
                .provider_ttls
                .0
                .iter()
                .filter(|(_, ttl)| **ttl > 0)
                .map(|(provider, _)| provider.clone())
                .collect()
        })
        .unwrap_or_default();
    let provider = preferred_provider
        .filter(|provider| {
            config
                .as_ref()
                .and_then(|config| provider_ttl(provider, &config.kv_cache))
                .is_some()
        })
        .map(|provider| provider.as_str().to_string());

    SessionWaitCommand {
        command: provider
            .as_deref()
            .map(|provider| format_session_wait_command(session_id, project_root, provider)),
        provider,
        legal_provider_keys,
    }
}

/// Render the compact structured contract required to wait for a CSA session.
///
/// The command itself remains in the adjacent durable `CSA:SESSION_STARTED`
/// or `CSA:SESSION_WAIT_KV_WARM` carrier marker, keeping this repeated hint
/// within the caller-context budget.
pub(crate) fn render_session_wait_caller_hint(action: &str, provider: &str) -> String {
    assert!(
        matches!(action, "wait" | "retry_wait"),
        "session wait caller hints only support wait actions"
    );
    let provider = escape_structured_comment_attr(provider);
    format!(
        "<!-- CSA:CALLER_HINT action=\"{action}\" provider=\"{provider}\" \
         background=true timeout_min_sec=7200 notify_on_complete=true \
         checkin_owner=CSA checkin_policy=provider_ttl \
         forbid=\"process.wait,process.poll,manual_status_loops,short_wrapper_timeouts\" -->"
    )
}

/// Return the normalized provider carried by explicit launch routing only.
///
/// A direct `--model-spec` wins. Otherwise, a trusted inherited subtree pin
/// may carry the launch identity for a nested CSA process. This intentionally
/// does not inspect ambient provider configuration or environment variables:
/// an unknown launch provider produces an actionable, non-runnable hint.
pub(crate) fn explicit_wait_provider_from_launch_routing(
    model_spec: Option<&str>,
    startup_env: &crate::startup_env::StartupSubtreeEnv,
) -> Option<ModelProvider> {
    let inherited_pin = crate::run_cmd_model_pin::inherited_model_pin_from_startup(startup_env);
    let model_spec =
        model_spec.or_else(|| inherited_pin.as_ref().map(|pin| pin.model_spec.as_str()))?;
    csa_executor::ModelSpec::parse(model_spec)
        .ok()
        .map(|spec| ModelProvider::new(&spec.provider))
        .filter(|provider| !provider.as_str().is_empty())
}

/// Format every emitted shell wait command in one provider-aware form.
pub(crate) fn format_session_wait_command(
    session_id: &str,
    project_root: &Path,
    model_provider: &str,
) -> String {
    format!(
        "csa session wait --session {session_id} --model-provider {model_provider}{}",
        format_cd_arg(project_root)
    )
}

pub(crate) fn format_session_kill_command(session_id: &str, project_root: &Path) -> String {
    format!(
        "csa session kill --session {session_id}{}",
        format_cd_arg(project_root)
    )
}

pub(crate) fn format_session_attach_command(session_id: &str, project_root: &Path) -> String {
    format!(
        "csa session attach --session {}{}",
        session_id,
        format_cd_arg(project_root)
    )
}

pub(crate) fn format_cd_arg(project_root: &Path) -> String {
    let project_root = project_root.to_string_lossy();
    format!(" --cd {}", shell_escape_for_command(&project_root))
}

pub(crate) fn escape_structured_comment_attr(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn shell_escape_for_command(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::resolve_session_wait_command;
    use crate::test_env_lock::TEST_ENV_LOCK;
    use std::path::Path;

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
    fn wait_command_rejects_ambient_provider_detection_without_explicit_routing_context() {
        let _env_lock = TEST_ENV_LOCK.blocking_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let config_root = dir.path().join("xdg-config");
        std::fs::create_dir_all(&config_root).expect("create config root");
        let _home_guard = EnvVarGuard::set("HOME", dir.path());
        let _xdg_guard = EnvVarGuard::set("XDG_CONFIG_HOME", &config_root);
        let _provider_guard = EnvVarGuard::set("HERMES_MODEL_PROVIDER", "openai-codex");
        let config_path =
            csa_config::ProjectConfig::user_config_path().expect("resolve user config path");
        std::fs::create_dir_all(config_path.parent().expect("config parent"))
            .expect("create config parent");
        std::fs::write(config_path, "[kv_cache.provider_ttls]\nopenai = 17\n")
            .expect("write provider config");

        let command = resolve_session_wait_command(
            "01KAS6M5XG7V4M4M6YDRS7P8R9",
            Path::new("/tmp/repo"),
            None,
        );
        assert!(
            command.command().is_none(),
            "an initial or retry hint must not infer its provider from HERMES_MODEL_PROVIDER"
        );
        assert!(
            command
                .provider_selection_hint()
                .contains("legal_keys=\"openai\""),
            "missing explicit routing must fail closed with actionable legal keys"
        );
    }

    #[test]
    fn retry_command_reloads_provider_ttl_config_and_preserves_explicit_provider() {
        let _env_lock = TEST_ENV_LOCK.blocking_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let config_root = dir.path().join("xdg-config");
        std::fs::create_dir_all(&config_root).expect("create config root");
        let _home_guard = EnvVarGuard::set("HOME", dir.path());
        let _xdg_guard = EnvVarGuard::set("XDG_CONFIG_HOME", &config_root);
        let _provider_guard = EnvVarGuard::set("HERMES_MODEL_PROVIDER", "other");
        let config_path =
            csa_config::ProjectConfig::user_config_path().expect("resolve user config path");
        std::fs::create_dir_all(config_path.parent().expect("config parent"))
            .expect("create config parent");
        std::fs::write(&config_path, "[kv_cache.provider_ttls]\nxai = 3300\n")
            .expect("write provider config");

        let provider = csa_config::ModelProvider::new(" XAI ");
        let command = resolve_session_wait_command(
            "01KAS6M5XG7V4M4M6YDRS7P8R9",
            Path::new("/tmp/repo"),
            Some(&provider),
        );
        assert_eq!(
            command.command(),
            Some(
                "csa session wait --session 01KAS6M5XG7V4M4M6YDRS7P8R9 --model-provider xai --cd '/tmp/repo'"
            )
        );
        assert!(
            command
                .caller_hint("retry_wait")
                .expect("validated provider must be retained for the caller hint")
                .contains("provider=\"xai\""),
            "caller hint must retain the validated normalized provider"
        );

        std::fs::write(&config_path, "[kv_cache.provider_ttls]\nxai = 0\n")
            .expect("invalidate provider config");
        let reloaded = resolve_session_wait_command(
            "01KAS6M5XG7V4M4M6YDRS7P8R9",
            Path::new("/tmp/repo"),
            Some(&provider),
        );
        assert!(
            reloaded.command().is_none(),
            "each retry must re-read current provider TTL configuration"
        );
        assert!(
            reloaded
                .provider_selection_hint()
                .contains("legal_keys=\"none\""),
            "an invalidated explicit provider must fail closed"
        );
    }
}
