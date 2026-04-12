use super::*;
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
fn build_config_get_lookup_global_kv_cache_returns_not_found_when_key_is_absent() {
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

    let lookup = build_config_get_lookup(None, "kv_cache.long_poll_seconds", false, true).unwrap();
    let value = resolve_lookup_sources(&lookup.sources, "kv_cache.long_poll_seconds").unwrap();

    assert!(
        value.is_none(),
        "kv_cache lookups should not synthesize defaults"
    );
}

#[test]
fn resolve_lookup_sources_global_raw_match_survives_invalid_unrelated_global_field() {
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

[defaults]
max_concurrent = "oops"
"#,
    )
    .unwrap();

    let lookup = build_config_get_lookup(None, "review.tool", false, true).unwrap();
    let value = resolve_lookup_sources(&lookup.sources, "review.tool")
        .unwrap()
        .and_then(|value| value.as_str().map(str::to_string));

    assert_eq!(value, Some("auto".to_string()));
}

#[test]
fn resolve_lookup_sources_warns_when_raw_global_fallback_follows_effective_project_parse_error() {
    let _env_lock = TEST_ENV_LOCK.lock().expect("config env lock poisoned");
    let dir = tempfile::tempdir().unwrap();
    let config_root = dir.path().join("xdg-config");
    std::fs::create_dir_all(&config_root).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", dir.path());
    let _xdg_guard = EnvVarGuard::set("XDG_CONFIG_HOME", &config_root);

    let project_root = dir.path().join("project");
    std::fs::create_dir_all(&project_root).unwrap();

    let global_dir = config_root.join("cli-sub-agent");
    std::fs::create_dir_all(&global_dir).unwrap();
    let global_config_path = global_dir.join("config.toml");
    std::fs::write(
        &global_config_path,
        r#"
[review]
tool = "auto"

[execution]
min_timeout_seconds = "oops"
"#,
    )
    .unwrap();

    let sources = vec![
        LookupSourceSpec::RawProject {
            path: ProjectConfig::config_path(&project_root),
        },
        LookupSourceSpec::EffectiveProject {
            project_root: project_root.clone(),
            include_global_fallback: true,
        },
        LookupSourceSpec::RawGlobal {
            path: global_config_path,
        },
        LookupSourceSpec::EffectiveGlobal {
            allow_raw_fallback: false,
        },
    ];

    let resolved = resolve_lookup_sources_with_diagnostics(&sources, "review.tool").unwrap();
    let value = resolved
        .value
        .as_ref()
        .and_then(toml::Value::as_str)
        .map(str::to_string);

    assert_eq!(value, Some("auto".to_string()));
    assert!(
        resolved
            .diagnostics
            .raw_global_value_from_invalid_effective_project
    );
    assert!(
        !resolved
            .diagnostics
            .raw_global_value_from_invalid_effective_global
    );
    assert!(resolved.diagnostics.should_warn_raw_global_parse_fallback());
}
