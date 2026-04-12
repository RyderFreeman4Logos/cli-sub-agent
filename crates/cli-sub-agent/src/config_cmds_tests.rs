use super::*;
use crate::test_env_lock::TEST_ENV_LOCK;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

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
fn resolve_key_scalar() {
    let root: toml::Value = toml::from_str("[review]\ntool = \"auto\"\n").unwrap();
    let val = resolve_key(&root, "review.tool").unwrap();
    assert_eq!(val.as_str(), Some("auto"));
}

#[test]
fn resolve_key_nested() {
    let root: toml::Value = toml::from_str("[tools.codex]\nenabled = true\n").unwrap();
    let val = resolve_key(&root, "tools.codex.enabled").unwrap();
    assert_eq!(val.as_bool(), Some(true));
}

#[test]
fn resolve_key_missing() {
    let root: toml::Value = toml::from_str("[review]\ntool = \"auto\"\n").unwrap();
    assert!(resolve_key(&root, "nonexistent.key").is_none());
}

#[test]
fn resolve_key_partial_path() {
    let root: toml::Value = toml::from_str("[review]\ntool = \"auto\"\n").unwrap();
    // "review" is a table, not a leaf — resolve_key returns the table
    let val = resolve_key(&root, "review").unwrap();
    assert!(val.is_table());
}

#[test]
fn format_toml_value_string() {
    let v = toml::Value::String("hello".to_string());
    assert_eq!(format_toml_value(&v), "hello");
}

#[test]
fn format_toml_value_integer() {
    let v = toml::Value::Integer(42);
    assert_eq!(format_toml_value(&v), "42");
}

#[test]
fn format_toml_value_bool() {
    let v = toml::Value::Boolean(true);
    assert_eq!(format_toml_value(&v), "true");
}

#[test]
fn load_and_resolve_missing_file() {
    let result = load_and_resolve(std::path::Path::new("/nonexistent/config.toml"), "key");
    assert!(result.unwrap().is_none());
}

#[test]
fn load_and_resolve_invalid_toml() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad.toml");
    std::fs::write(&path, "{{invalid toml").unwrap();
    let result = load_and_resolve(&path, "key");
    assert!(result.is_err());
}

#[test]
fn build_project_display_toml_keeps_effective_execution_defaults_visible() {
    let config: ProjectConfig = toml::from_str("schema_version = 1\n").unwrap();
    let rendered = toml::to_string_pretty(&build_project_display_toml(&config).unwrap()).unwrap();
    assert!(rendered.contains("[execution]"));
    assert!(rendered.contains("min_timeout_seconds = 1800"));
    assert!(rendered.contains("auto_weave_upgrade = false"));
}

#[test]
fn build_project_display_json_keeps_effective_execution_defaults_visible() {
    let config: ProjectConfig = toml::from_str("schema_version = 1\n").unwrap();
    let rendered = build_project_display_json(&config).unwrap();
    assert_eq!(
        rendered
            .get("execution")
            .and_then(|value| value.get("min_timeout_seconds"))
            .and_then(|value| value.as_u64()),
        Some(1800)
    );
    assert_eq!(
        rendered
            .get("execution")
            .and_then(|value| value.get("auto_weave_upgrade"))
            .and_then(|value| value.as_bool()),
        Some(false)
    );
}

#[test]
fn resolve_effective_execution_key_uses_compile_default_when_no_config_exists() {
    let _env_lock = TEST_ENV_LOCK.lock().expect("config env lock poisoned");
    let dir = tempfile::tempdir().unwrap();
    let config_root = dir.path().join("xdg-config");
    std::fs::create_dir_all(&config_root).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", dir.path());
    let _xdg_guard = EnvVarGuard::set("XDG_CONFIG_HOME", &config_root);
    let value = resolve_effective_execution_key(dir.path(), "execution.min_timeout_seconds")
        .unwrap()
        .expect("effective timeout floor should exist");
    assert_eq!(value.as_integer(), Some(1800));
}

#[test]
fn resolve_effective_execution_key_prefers_project_override() {
    let dir = tempfile::tempdir().unwrap();
    let csa_dir = dir.path().join(".csa");
    std::fs::create_dir_all(&csa_dir).unwrap();
    std::fs::write(
        csa_dir.join("config.toml"),
        r#"
schema_version = 1
[execution]
min_timeout_seconds = 2400
auto_weave_upgrade = true
"#,
    )
    .unwrap();

    let timeout = resolve_effective_execution_key(dir.path(), "execution.min_timeout_seconds")
        .unwrap()
        .expect("project timeout should resolve");
    let auto_upgrade = resolve_effective_execution_key(dir.path(), "execution.auto_weave_upgrade")
        .unwrap()
        .expect("project auto upgrade should resolve");

    assert_eq!(timeout.as_integer(), Some(2400));
    assert_eq!(auto_upgrade.as_bool(), Some(true));
}

#[test]
fn resolve_effective_execution_key_uses_global_fallback_when_project_missing() {
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
schema_version = 1
[execution]
min_timeout_seconds = 3600
auto_weave_upgrade = true
"#,
    )
    .unwrap();

    let timeout = resolve_effective_execution_key(dir.path(), "execution.min_timeout_seconds")
        .unwrap()
        .expect("global timeout should resolve");
    let auto_upgrade = resolve_effective_execution_key(dir.path(), "execution.auto_weave_upgrade")
        .unwrap()
        .expect("global auto upgrade should resolve");

    assert_eq!(timeout.as_integer(), Some(3600));
    assert_eq!(auto_upgrade.as_bool(), Some(true));
}

#[test]
fn build_config_get_lookup_resolves_effective_project_nested_resource_keys() {
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
[resources]
memory_max_mb = 1024
"#,
    )
    .unwrap();

    let lookup = build_config_get_lookup(
        Some(dir.path()),
        "resources.slot_wait_timeout_seconds",
        false,
        false,
    )
    .unwrap();
    let value = resolve_lookup_sources(&lookup.sources, "resources.slot_wait_timeout_seconds")
        .unwrap()
        .expect("effective resources default should resolve");

    assert_eq!(value.as_integer(), Some(250));
    assert!(
        collect_lookup_keys(&lookup.sources)
            .unwrap()
            .contains("resources.slot_wait_timeout_seconds"),
        "effective lookup should advertise nested resource keys visible in config show"
    );
}

#[test]
fn resolve_effective_global_key_uses_kv_cache_defaults_when_global_config_missing() {
    let _env_lock = TEST_ENV_LOCK.lock().expect("config env lock poisoned");
    let dir = tempfile::tempdir().unwrap();
    let config_root = dir.path().join("xdg-config");
    std::fs::create_dir_all(&config_root).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", dir.path());
    let _xdg_guard = EnvVarGuard::set("XDG_CONFIG_HOME", &config_root);

    let frequent = resolve_effective_global_key("kv_cache.frequent_poll_seconds")
        .unwrap()
        .expect("global frequent poll should resolve");
    let long = resolve_effective_global_key("kv_cache.long_poll_seconds")
        .unwrap()
        .expect("global long poll should resolve");

    assert_eq!(frequent.as_integer(), Some(60));
    assert_eq!(long.as_integer(), Some(240));
}

#[test]
fn resolve_effective_global_key_uses_configured_kv_cache_values() {
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
long_poll_seconds = 3000
"#,
    )
    .unwrap();

    let frequent = resolve_effective_global_key("kv_cache.frequent_poll_seconds")
        .unwrap()
        .expect("configured frequent poll should resolve");
    let long = resolve_effective_global_key("kv_cache.long_poll_seconds")
        .unwrap()
        .expect("configured long poll should resolve");

    assert_eq!(frequent.as_integer(), Some(45));
    assert_eq!(long.as_integer(), Some(3000));
}

#[test]
fn resolve_effective_global_key_sanitizes_zero_kv_cache_values() {
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
frequent_poll_seconds = 0
long_poll_seconds = 0
"#,
    )
    .unwrap();

    let frequent = resolve_effective_global_key("kv_cache.frequent_poll_seconds")
        .unwrap()
        .expect("zero-valued frequent poll should sanitize to default");
    let long = resolve_effective_global_key("kv_cache.long_poll_seconds")
        .unwrap()
        .expect("zero-valued long poll should sanitize to default");

    assert_eq!(frequent.as_integer(), Some(60));
    assert_eq!(long.as_integer(), Some(240));
}

#[test]
fn resolve_effective_key_ignores_project_kv_cache_override() {
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
[kv_cache]
long_poll_seconds = 9999
"#,
    )
    .unwrap();

    let expected = resolve_effective_global_key("kv_cache.long_poll_seconds")
        .unwrap()
        .expect("global kv cache long poll should resolve");
    let value = resolve_effective_key(Some(dir.path()), "kv_cache.long_poll_seconds", false, false)
        .unwrap()
        .expect("kv cache long poll should resolve from the effective global config");

    assert_eq!(value, expected);
    assert_eq!(value.as_integer(), Some(3000));
}

#[test]
fn build_config_get_lookup_preserves_unknown_raw_project_sections() {
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
[pr_review]
cloud_bot_name = "gemini-code-assist"
cloud_bot_trigger = "comment"
merge_strategy = "merge"
delete_branch = false
"#,
    )
    .unwrap();

    let lookup =
        build_config_get_lookup(Some(dir.path()), "pr_review.cloud_bot_name", false, false)
            .unwrap();

    assert_eq!(
        resolve_lookup_sources(&lookup.sources, "pr_review.cloud_bot_name")
            .unwrap()
            .and_then(|value| value.as_str().map(str::to_string)),
        Some("gemini-code-assist".to_string())
    );
    assert_eq!(
        resolve_lookup_sources(&lookup.sources, "pr_review.cloud_bot_trigger")
            .unwrap()
            .and_then(|value| value.as_str().map(str::to_string)),
        Some("comment".to_string())
    );
    assert_eq!(
        resolve_lookup_sources(&lookup.sources, "pr_review.merge_strategy")
            .unwrap()
            .and_then(|value| value.as_str().map(str::to_string)),
        Some("merge".to_string())
    );
    assert_eq!(
        resolve_lookup_sources(&lookup.sources, "pr_review.delete_branch")
            .unwrap()
            .and_then(|value| value.as_bool()),
        Some(false)
    );
}

#[test]
fn build_config_get_lookup_prefers_effective_values_for_known_sections() {
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
[tools.codex]
enabled = false
"#,
    )
    .unwrap();

    let csa_dir = dir.path().join(".csa");
    std::fs::create_dir_all(&csa_dir).unwrap();
    std::fs::write(
        csa_dir.join("config.toml"),
        r#"
schema_version = 1
[tools.codex]
enabled = true
"#,
    )
    .unwrap();

    let lookup =
        build_config_get_lookup(Some(dir.path()), "tools.codex.enabled", false, false).unwrap();
    let value = resolve_lookup_sources(&lookup.sources, "tools.codex.enabled")
        .unwrap()
        .expect("effective tool enablement should resolve");

    assert_eq!(value.as_bool(), Some(false));
}

#[test]
fn resolve_effective_key_redacts_global_memory_api_keys_in_project_scoped_lookups() {
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
[memory.llm]
enabled = true
api_key = "sk-super-secret-5982"
"#,
    )
    .unwrap();

    let csa_dir = dir.path().join(".csa");
    std::fs::create_dir_all(&csa_dir).unwrap();
    std::fs::write(
        csa_dir.join("config.toml"),
        r#"
schema_version = 1
[memory]
inject = true
"#,
    )
    .unwrap();

    let value = resolve_effective_key(Some(dir.path()), "memory", false, false)
        .unwrap()
        .expect("merged memory config should resolve");
    let rendered = format_toml_value(&value);

    assert!(
        !rendered.contains("sk-super-secret-5982"),
        "project-scoped lookup leaked raw api key: {rendered}"
    );
    assert!(
        rendered.contains("api_key") && rendered.contains("..."),
        "project-scoped lookup should include a masked api key: {rendered}"
    );
}

#[test]
fn build_config_get_lookup_project_only_uses_effective_project_defaults() {
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
[resources]
memory_max_mb = 1024
"#,
    )
    .unwrap();

    let lookup = build_config_get_lookup(
        Some(dir.path()),
        "resources.slot_wait_timeout_seconds",
        true,
        false,
    )
    .unwrap();
    let value = resolve_lookup_sources(&lookup.sources, "resources.slot_wait_timeout_seconds")
        .unwrap()
        .expect("project-only lookups should expose effective project defaults");

    assert_eq!(value.as_integer(), Some(250));
}

#[test]
fn resolve_lookup_sources_falls_back_to_raw_project_for_known_sections_when_global_is_invalid() {
    let _env_lock = TEST_ENV_LOCK.lock().expect("config env lock poisoned");
    let dir = tempfile::tempdir().unwrap();
    let config_root = dir.path().join("xdg-config");
    std::fs::create_dir_all(&config_root).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", dir.path());
    let _xdg_guard = EnvVarGuard::set("XDG_CONFIG_HOME", &config_root);

    let global_dir = config_root.join("cli-sub-agent");
    std::fs::create_dir_all(&global_dir).unwrap();
    std::fs::write(global_dir.join("config.toml"), "{{invalid toml").unwrap();

    let csa_dir = dir.path().join(".csa");
    std::fs::create_dir_all(&csa_dir).unwrap();
    std::fs::write(
        csa_dir.join("config.toml"),
        r#"
schema_version = 1
[resources]
memory_max_mb = 1024
"#,
    )
    .unwrap();

    let lookup =
        build_config_get_lookup(Some(dir.path()), "resources.memory_max_mb", false, false).unwrap();
    let value = resolve_lookup_sources(&lookup.sources, "resources.memory_max_mb")
        .unwrap()
        .expect("raw project value should survive invalid global config");

    assert_eq!(value.as_integer(), Some(1024));
}

#[test]
fn resolve_lookup_sources_returns_project_raw_match_before_global_parse() {
    let _env_lock = TEST_ENV_LOCK.lock().expect("config env lock poisoned");
    let dir = tempfile::tempdir().unwrap();
    let config_root = dir.path().join("xdg-config");
    std::fs::create_dir_all(&config_root).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", dir.path());
    let _xdg_guard = EnvVarGuard::set("XDG_CONFIG_HOME", &config_root);

    let global_dir = config_root.join("cli-sub-agent");
    std::fs::create_dir_all(&global_dir).unwrap();
    std::fs::write(global_dir.join("config.toml"), "{{invalid toml").unwrap();

    let csa_dir = dir.path().join(".csa");
    std::fs::create_dir_all(&csa_dir).unwrap();
    std::fs::write(
        csa_dir.join("config.toml"),
        r#"
schema_version = 1
[pr_review]
cloud_bot_name = "gemini-code-assist"
"#,
    )
    .unwrap();

    let lookup =
        build_config_get_lookup(Some(dir.path()), "pr_review.cloud_bot_name", false, false)
            .unwrap();
    let value = resolve_lookup_sources(&lookup.sources, "pr_review.cloud_bot_name")
        .unwrap()
        .and_then(|value| value.as_str().map(str::to_string));

    assert_eq!(value, Some("gemini-code-assist".to_string()));
}

#[test]
fn resolve_lookup_sources_invalid_project_raw_still_errors_before_global_match() {
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
[execution]
min_timeout_seconds = 3600
"#,
    )
    .unwrap();

    let csa_dir = dir.path().join(".csa");
    std::fs::create_dir_all(&csa_dir).unwrap();
    std::fs::write(csa_dir.join("config.toml"), "{{invalid toml").unwrap();

    let lookup = build_config_get_lookup(
        Some(dir.path()),
        "execution.min_timeout_seconds",
        false,
        false,
    )
    .unwrap();
    let error = resolve_lookup_sources(&lookup.sources, "execution.min_timeout_seconds")
        .expect_err("broken project config should fail before global fallback");

    let message = error.to_string();
    assert!(
        message.contains("Failed to parse project config") || message.contains("TOML parse error"),
        "expected project parse failure, got: {message}"
    );
}

#[test]
fn suggest_key_paths_returns_closest_matches() {
    let candidates = std::collections::BTreeSet::from([
        "pr_review.cloud_bot_name".to_string(),
        "pr_review.cloud_bot_trigger".to_string(),
        "resources.slot_wait_timeout_seconds".to_string(),
    ]);

    let suggestions = suggest_key_paths("pr_review.cloud_bot_nam", &candidates);

    assert_eq!(
        suggestions.first().map(String::as_str),
        Some("pr_review.cloud_bot_name")
    );
}

#[test]
fn format_missing_key_message_includes_suggestions() {
    let message = format_missing_key_message(
        "pr_review.cloud_bot_nam",
        &[
            "pr_review.cloud_bot_name".to_string(),
            "pr_review.cloud_bot_trigger".to_string(),
        ],
    );

    assert!(message.contains("Key not found: pr_review.cloud_bot_nam"));
    assert!(message.contains("Closest matches:"));
    assert!(message.contains("pr_review.cloud_bot_name"));
    assert!(message.contains("pr_review.cloud_bot_trigger"));
}

#[test]
fn parse_editor_command_supports_embedded_args() {
    let (program, args) = parse_editor_command("code --wait").unwrap();
    assert_eq!(program, "code");
    assert_eq!(args, vec!["--wait"]);
}

#[test]
fn parse_editor_command_supports_plain_editor() {
    let (program, args) = parse_editor_command("vim").unwrap();
    assert_eq!(program, "vim");
    assert!(args.is_empty());
}

#[test]
fn parse_editor_command_rejects_whitespace_only_value() {
    let error = parse_editor_command("   ").unwrap_err();
    assert!(error.to_string().contains("$EDITOR is set but empty"));
}

#[cfg(unix)]
#[test]
fn handle_config_edit_supports_quoted_editor_path_with_embedded_args() {
    let _env_lock = TEST_ENV_LOCK.lock().expect("config env lock poisoned");
    let dir = tempfile::tempdir().unwrap();
    let config_dir = dir.path().join(".csa");
    std::fs::create_dir_all(&config_dir).unwrap();

    let config_path = config_dir.join("config.toml");
    std::fs::write(&config_path, "schema_version = 1\n").unwrap();

    let editor_dir = dir.path().join("editor bin");
    std::fs::create_dir_all(&editor_dir).unwrap();
    let editor_path = editor_dir.join("mock editor.sh");
    write_recording_editor(&editor_path).unwrap();

    let captured_args_path = dir.path().join("captured-args.txt");
    let _capture_guard = EnvVarGuard::set("CSA_TEST_EDITOR_ARGS_FILE", &captured_args_path);
    let editor_value = format!("'{}' --wait", editor_path.display());
    let _editor_guard = EnvVarGuard::set("EDITOR", &editor_value);

    handle_config_edit(Some(dir.path().display().to_string())).unwrap();

    let captured_args = std::fs::read_to_string(&captured_args_path).unwrap();
    let captured_lines: Vec<_> = captured_args.lines().collect();
    assert_eq!(
        captured_lines,
        vec!["--wait", config_path.to_str().unwrap()]
    );
}

#[cfg(unix)]
fn write_recording_editor(path: &std::path::Path) -> Result<()> {
    std::fs::write(
        path,
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$CSA_TEST_EDITOR_ARGS_FILE\"\n",
    )?;
    let mut permissions = std::fs::metadata(path)?.permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions)?;
    Ok(())
}
