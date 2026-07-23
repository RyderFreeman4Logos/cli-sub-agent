use std::path::Path;

use csa_config::{GlobalConfig, ModelProvider, detect_model_provider, provider_ttl};

pub(crate) struct SessionWaitCommand {
    command: Option<String>,
    legal_provider_keys: Vec<String>,
}

impl SessionWaitCommand {
    pub(crate) fn command(&self) -> Option<&str> {
        self.command.as_deref()
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
/// Explicit caller context takes precedence over best-effort current-process
/// detection. A command is emitted only when its provider has a positive TTL
/// in the active user configuration; otherwise the caller must choose from the
/// legal keys carried by [`SessionWaitCommand::provider_selection_hint`].
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
        .cloned()
        .or_else(detect_model_provider)
        .filter(|provider| {
            config
                .as_ref()
                .and_then(|config| provider_ttl(provider, &config.kv_cache))
                .is_some()
        });

    SessionWaitCommand {
        command: provider.as_ref().map(|provider| {
            format_session_wait_command(session_id, project_root, provider.as_str())
        }),
        legal_provider_keys,
    }
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
    fn wait_command_includes_detected_configured_model_provider() {
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

        assert_eq!(
            resolve_session_wait_command(
                "01KAS6M5XG7V4M4M6YDRS7P8R9",
                Path::new("/tmp/repo"),
                None,
            )
            .command(),
            Some(
                "csa session wait --session 01KAS6M5XG7V4M4M6YDRS7P8R9 --model-provider openai --cd '/tmp/repo'"
            )
        );
    }
}
