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

fn write_global_config(contents: &str) {
    let read_path =
        csa_config::ProjectConfig::user_config_path().expect("resolve user config path");
    std::fs::create_dir_all(read_path.parent().expect("user config dir")).unwrap();
    std::fs::write(&read_path, contents).unwrap();

    let write_path = csa_config::GlobalConfig::config_path().expect("resolve global config path");
    if write_path != read_path {
        std::fs::create_dir_all(write_path.parent().expect("global config dir")).unwrap();
        std::fs::write(&write_path, contents).unwrap();
    }
}

#[test]
fn resolve_effective_global_key_uses_experimental_defaults_when_global_config_missing() {
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let dir = tempfile::tempdir().unwrap();
    let config_root = dir.path().join("xdg-config");
    std::fs::create_dir_all(&config_root).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", dir.path());
    let _xdg_guard = EnvVarGuard::set("XDG_CONFIG_HOME", &config_root);

    let value = resolve_effective_global_key("experimental.enable_prompt_caching")
        .unwrap()
        .expect("global prompt caching flag should resolve");
    let max_loops = resolve_effective_global_key("experimental.max_goal_loops")
        .unwrap()
        .expect("global goal loop limit should resolve");
    let max_tokens = resolve_effective_global_key("experimental.max_goal_tokens")
        .unwrap()
        .expect("global goal token limit should resolve");

    assert_eq!(value.as_bool(), Some(false));
    assert_eq!(max_loops.as_integer(), Some(3));
    assert_eq!(max_tokens.as_integer(), Some(500_000));
}

#[test]
fn resolve_effective_global_key_uses_configured_experimental_prompt_caching() {
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let dir = tempfile::tempdir().unwrap();
    let config_root = dir.path().join("xdg-config");
    std::fs::create_dir_all(&config_root).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", dir.path());
    let _xdg_guard = EnvVarGuard::set("XDG_CONFIG_HOME", &config_root);

    write_global_config(
        r#"
[experimental]
enable_prompt_caching = true
max_goal_loops = 7
max_goal_tokens = 42
"#,
    );

    let value = resolve_effective_global_key("experimental.enable_prompt_caching")
        .unwrap()
        .expect("configured prompt caching flag should resolve");
    let max_loops = resolve_effective_global_key("experimental.max_goal_loops")
        .unwrap()
        .expect("configured goal loop limit should resolve");
    let max_tokens = resolve_effective_global_key("experimental.max_goal_tokens")
        .unwrap()
        .expect("configured goal token limit should resolve");

    assert_eq!(value.as_bool(), Some(true));
    assert_eq!(max_loops.as_integer(), Some(7));
    assert_eq!(max_tokens.as_integer(), Some(42));
}
