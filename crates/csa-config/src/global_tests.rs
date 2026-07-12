use super::*;
use csa_core::{
    env::NO_FAILOVER_ENV_KEY,
    gemini::{
        API_KEY_ENV, API_KEY_FALLBACK_ENV_KEY, AUTH_MODE_ENV_KEY, AUTH_MODE_OAUTH,
        NO_FLASH_FALLBACK_ENV_KEY,
    },
};
use serial_test::serial;
use std::collections::HashMap;
use std::path::PathBuf;

struct EnvVarGuard {
    key: &'static str,
    original: Option<String>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let original = std::env::var(key).ok();
        // SAFETY: test-scoped env mutation is reverted in Drop.
        unsafe { std::env::set_var(key, value) };
        Self { key, original }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        // SAFETY: test-scoped env mutation is reverted in Drop.
        unsafe {
            match self.original.as_deref() {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

#[test]
fn test_default_config() {
    let config = GlobalConfig::default();
    assert_eq!(config.defaults.max_concurrent, 3);
    assert_eq!(config.kv_cache.frequent_poll_seconds, 60);
    assert_eq!(config.kv_cache.long_poll_seconds, 240);
    assert_eq!(
        config.tool_state_dirs.get("codex"),
        Some(&PathBuf::from("~/.codex"))
    );
    assert_eq!(
        config.tool_state_dirs.get("claude"),
        Some(&PathBuf::from("~/.claude"))
    );
    assert!(config.tools.is_empty());
}

#[test]
fn test_kv_cache_defaults_parse_when_section_omitted() {
    let config: GlobalConfig = toml::from_str("").unwrap();
    assert_eq!(config.kv_cache.frequent_poll_seconds, 60);
    assert_eq!(config.kv_cache.long_poll_seconds, 240);
}

#[test]
fn test_tool_state_dirs_defaults_parse_when_section_omitted() {
    let config: GlobalConfig = toml::from_str("").unwrap();
    assert_eq!(
        config.tool_state_dirs.get("codex"),
        Some(&PathBuf::from("~/.codex"))
    );
    assert_eq!(
        config.tool_state_dirs.get("claude"),
        Some(&PathBuf::from("~/.claude"))
    );
}

#[test]
fn test_tool_state_dirs_parse_from_global_config() {
    let config: GlobalConfig = toml::from_str(
        r#"
[tool_state_dirs]
codex = "/srv/codex-state"
claude = "/srv/claude-state"
"#,
    )
    .unwrap();
    assert_eq!(
        config.tool_state_dirs.get("codex"),
        Some(&PathBuf::from("/srv/codex-state"))
    );
    assert_eq!(
        config.tool_state_dirs.get("claude"),
        Some(&PathBuf::from("/srv/claude-state"))
    );
}

#[test]
fn test_tier_policy_defaults_to_force_bypass_disabled() {
    let config: GlobalConfig = toml::from_str("").unwrap();
    assert!(!config.tier_policy.allow_force_bypass);
}

#[test]
fn test_tier_policy_allow_force_bypass_parses_from_global_config() {
    let config: GlobalConfig = toml::from_str(
        r#"
[tier_policy]
allow_force_bypass = true
"#,
    )
    .unwrap();
    assert!(config.tier_policy.allow_force_bypass);
}

#[test]
fn test_session_wait_defaults_parse_when_section_omitted() {
    let config: GlobalConfig = toml::from_str("").unwrap();
    assert_eq!(config.session_wait.memory_warn_mb, None);
}

#[test]
fn test_session_wait_memory_warn_mb_parses_from_config() {
    let config: GlobalConfig = toml::from_str(
        r#"
[session_wait]
memory_warn_mb = 8192
"#,
    )
    .unwrap();
    assert_eq!(config.session_wait.memory_warn_mb, Some(8192));
}

#[test]
fn test_resolve_session_wait_long_poll_seconds_uses_configured_kv_cache_value() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[kv_cache]
long_poll_seconds = 3000
"#,
    )
    .unwrap();

    assert_eq!(
        GlobalConfig::resolve_session_wait_long_poll_seconds_from_path(Some(&path)),
        3000
    );
}

#[test]
fn test_resolve_session_wait_long_poll_seconds_uses_documented_default_without_kv_cache_section() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[review]
tool = "auto"
"#,
    )
    .unwrap();

    assert_eq!(
        GlobalConfig::resolve_session_wait_long_poll_seconds_from_path(Some(&path)),
        240
    );
    assert_eq!(
        GlobalConfig::resolve_session_wait_long_poll_seconds_from_path_with_source(Some(&path))
            .source,
        KvCacheValueSource::DocumentedDefault
    );
}

#[test]
fn test_resolve_session_wait_long_poll_seconds_sanitizes_zero_value_to_default() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[kv_cache]
long_poll_seconds = 0
"#,
    )
    .unwrap();

    assert_eq!(
        GlobalConfig::resolve_session_wait_long_poll_seconds_from_path(Some(&path)),
        240
    );
    assert_eq!(
        GlobalConfig::resolve_session_wait_long_poll_seconds_from_path_with_source(Some(&path))
            .source,
        KvCacheValueSource::SectionDefault
    );
}

#[test]
fn test_resolve_session_wait_long_poll_seconds_tracks_explicit_default_source() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[kv_cache]
long_poll_seconds = 240
"#,
    )
    .unwrap();

    let resolved =
        GlobalConfig::resolve_session_wait_long_poll_seconds_from_path_with_source(Some(&path));
    assert_eq!(resolved.seconds, 240);
    assert_eq!(resolved.source, KvCacheValueSource::Configured);
}

#[test]
fn test_resolve_session_wait_long_poll_seconds_tracks_section_default_source() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[kv_cache]
frequent_poll_seconds = 45
"#,
    )
    .unwrap();

    let resolved =
        GlobalConfig::resolve_session_wait_long_poll_seconds_from_path_with_source(Some(&path));
    assert_eq!(resolved.seconds, 240);
    assert_eq!(resolved.source, KvCacheValueSource::SectionDefault);
}

#[test]
#[serial]
fn test_resolve_session_wait_long_poll_seconds_uses_legacy_config_dir_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let config_root = dir.path().join("xdg-config");
    let _home_guard = EnvVarGuard::set("HOME", dir.path());
    let _xdg_guard = EnvVarGuard::set("XDG_CONFIG_HOME", &config_root);
    let legacy_dir = crate::paths::legacy_config_dir().expect("legacy config dir");
    std::fs::create_dir_all(&legacy_dir).unwrap();
    std::fs::write(
        legacy_dir.join("config.toml"),
        r#"
[kv_cache]
long_poll_seconds = 3000
"#,
    )
    .unwrap();

    let config_dir = crate::paths::config_dir();
    assert_eq!(
        GlobalConfig::resolve_session_wait_long_poll_seconds_from_dir(config_dir.as_deref()),
        3000
    );
}

#[test]
fn test_max_concurrent_default() {
    let config = GlobalConfig::default();
    assert_eq!(config.max_concurrent("gemini-cli"), 3);
    assert_eq!(config.max_concurrent("codex"), 3);
}

#[test]
fn test_max_concurrent_tool_override() {
    let mut config = GlobalConfig::default();
    config.tools.insert(
        "gemini-cli".to_string(),
        GlobalToolConfig {
            max_concurrent: Some(5),
            env: HashMap::new(),
            ..Default::default()
        },
    );
    assert_eq!(config.max_concurrent("gemini-cli"), 5);
    assert_eq!(config.max_concurrent("codex"), 3); // falls back to default
}

#[test]
fn test_env_vars() {
    let mut config = GlobalConfig::default();
    let mut env = HashMap::new();
    env.insert("GEMINI_API_KEY".to_string(), "test-key".to_string());
    config.tools.insert(
        "gemini-cli".to_string(),
        GlobalToolConfig {
            max_concurrent: None,
            env,
            ..Default::default()
        },
    );

    let vars = config.env_vars("gemini-cli").unwrap();
    assert_eq!(vars.get("GEMINI_API_KEY").unwrap(), "test-key");
    assert!(config.env_vars("codex").is_none());
}

#[test]
fn test_env_vars_empty_returns_none() {
    let mut config = GlobalConfig::default();
    config.tools.insert(
        "codex".to_string(),
        GlobalToolConfig {
            max_concurrent: Some(2),
            env: HashMap::new(),
            ..Default::default()
        },
    );
    assert!(config.env_vars("codex").is_none());
}

#[test]
fn test_build_execution_env_adds_gemini_fallback_and_oauth_mode() {
    let mut config = GlobalConfig::default();
    config.tools.insert(
        "gemini-cli".to_string(),
        GlobalToolConfig {
            api_key: Some("fallback-key".to_string()),
            ..Default::default()
        },
    );

    let env = config
        .build_execution_env("gemini-cli", ExecutionEnvOptions::default())
        .unwrap();
    assert_eq!(
        env.get(API_KEY_FALLBACK_ENV_KEY).map(String::as_str),
        Some("fallback-key")
    );
    assert_eq!(
        env.get(AUTH_MODE_ENV_KEY).map(String::as_str),
        Some(AUTH_MODE_OAUTH)
    );
}

#[test]
fn test_build_execution_env_promotes_legacy_api_key_to_fallback_and_no_flash() {
    let mut config = GlobalConfig::default();
    let mut env = HashMap::new();
    env.insert(API_KEY_ENV.to_string(), "configured-key".to_string());
    env.insert("OTHER_VAR".to_string(), "keep".to_string());
    config.tools.insert(
        "gemini-cli".to_string(),
        GlobalToolConfig {
            env,
            ..Default::default()
        },
    );

    let env = config
        .build_execution_env("gemini-cli", ExecutionEnvOptions::with_no_flash_fallback())
        .unwrap();
    assert!(
        !env.contains_key(API_KEY_ENV),
        "legacy GEMINI_API_KEY should not force API key mode on a fresh invocation"
    );
    assert_eq!(
        env.get(API_KEY_FALLBACK_ENV_KEY).map(String::as_str),
        Some("configured-key")
    );
    assert_eq!(
        env.get(AUTH_MODE_ENV_KEY).map(String::as_str),
        Some(AUTH_MODE_OAUTH)
    );
    assert_eq!(
        env.get(NO_FLASH_FALLBACK_ENV_KEY).map(String::as_str),
        Some("1")
    );
    assert_eq!(env.get("OTHER_VAR").map(String::as_str), Some("keep"));
}

#[test]
fn test_build_execution_env_marks_no_failover_when_requested() {
    let mut config = GlobalConfig::default();
    config.tools.insert(
        "codex".to_string(),
        GlobalToolConfig {
            env: HashMap::from([("OTHER_VAR".to_string(), "keep".to_string())]),
            ..Default::default()
        },
    );

    let env = config
        .build_execution_env("codex", ExecutionEnvOptions::default().with_no_failover())
        .unwrap();
    assert_eq!(env.get(NO_FAILOVER_ENV_KEY).map(String::as_str), Some("1"));
    assert_eq!(env.get("OTHER_VAR").map(String::as_str), Some("keep"));
}

#[test]
fn test_build_execution_env_drops_user_configured_no_failover_spoof() {
    let mut config = GlobalConfig::default();
    config.tools.insert(
        "codex".to_string(),
        GlobalToolConfig {
            env: HashMap::from([
                (NO_FAILOVER_ENV_KEY.to_string(), "1".to_string()),
                ("OTHER_VAR".to_string(), "keep".to_string()),
            ]),
            ..Default::default()
        },
    );

    // Without --no-failover, the CLI should win: user's spoofed value is dropped.
    let env = config
        .build_execution_env("codex", ExecutionEnvOptions::default())
        .unwrap();
    assert!(
        !env.contains_key(NO_FAILOVER_ENV_KEY),
        "user config must not be able to spoof _CSA_NO_FAILOVER"
    );
    assert_eq!(env.get("OTHER_VAR").map(String::as_str), Some("keep"));
}

#[test]
fn test_build_execution_env_drops_user_configured_subtree_pin_spoof() {
    // #1741 round-3: tool-config extra_env must not be able to spoof a CSA-owned
    // subtree model pin. build_execution_env is the single funnel for user env,
    // so all reserved pin keys must be stripped here regardless of value.
    let mut config = GlobalConfig::default();
    config.tools.insert(
        "codex".to_string(),
        GlobalToolConfig {
            env: HashMap::from([
                (
                    "CSA_MODEL_SPEC".to_string(),
                    "gemini-cli/google/gemini-3.1-pro-preview/xhigh".to_string(),
                ),
                ("CSA_FORCE_IGNORE_TIER_SETTING".to_string(), "1".to_string()),
                ("CSA_NO_FAILOVER".to_string(), "1".to_string()),
                ("OTHER_VAR".to_string(), "keep".to_string()),
            ]),
            ..Default::default()
        },
    );

    let env = config
        .build_execution_env("codex", ExecutionEnvOptions::default())
        .unwrap();
    for key in csa_core::env::SUBTREE_PIN_ENV_KEYS {
        assert!(
            !env.contains_key(*key),
            "user config must not be able to spoof CSA-owned subtree-pin key {key}"
        );
    }
    // Non-reserved user env is preserved.
    assert_eq!(env.get("OTHER_VAR").map(String::as_str), Some("keep"));
}

include!("global_tests_split.rs");
