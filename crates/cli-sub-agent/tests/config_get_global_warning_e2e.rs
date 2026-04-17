use serial_test::serial;
use std::path::Path;
use std::process::Command;

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

fn global_config_path(tmp: &Path) -> std::path::PathBuf {
    // Mirror the production resolver so the test writes the same platform-specific
    // global path that `csa config get --global` reads on Linux and macOS.
    let _home_guard = EnvVarGuard::set("HOME", tmp);
    let _xdg_guard = EnvVarGuard::set("XDG_CONFIG_HOME", tmp.join(".config"));
    csa_config::GlobalConfig::config_path().expect("resolve global config path")
}

fn csa_cmd(tmp: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_csa"));
    cmd.env("HOME", tmp)
        .env("XDG_STATE_HOME", tmp.join(".local/state"))
        .env("XDG_CONFIG_HOME", tmp.join(".config"));
    cmd
}

#[test]
#[serial]
fn config_get_global_warns_when_falling_back_to_raw_invalid_global_config() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let global_config_path = global_config_path(tmp.path());
    let global_dir = global_config_path.parent().expect("global config dir");
    std::fs::create_dir_all(global_dir).expect("create global config dir");
    std::fs::write(
        &global_config_path,
        r#"
[review]
tool = "auto"

[defaults]
max_concurrent = "bad"
"#,
    )
    .expect("write global config");

    let output = csa_cmd(tmp.path())
        .args(["config", "get", "review.tool", "--global"])
        .current_dir(tmp.path())
        .output()
        .expect("run csa config get");

    assert!(
        output.status.success(),
        "config get should still succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "auto");
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("warning: global config has parse errors; showing raw value"),
        "stderr should surface the raw-value fallback warning, got: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
