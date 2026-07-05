use super::*;
use std::{ffi::OsString, path::PathBuf};

struct ScopedEnvVar {
    key: &'static str,
    previous: Option<OsString>,
}

impl ScopedEnvVar {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let previous = std::env::var_os(key);
        // SAFETY: test holds ENV_LOCK while mutating process environment.
        unsafe {
            std::env::set_var(key, value);
        }
        Self { key, previous }
    }

    fn unset(key: &'static str) -> Self {
        let previous = std::env::var_os(key);
        // SAFETY: test holds ENV_LOCK while mutating process environment.
        unsafe {
            std::env::remove_var(key);
        }
        Self { key, previous }
    }
}

impl Drop for ScopedEnvVar {
    fn drop(&mut self) {
        // SAFETY: guard lifetime is contained by the locked test section.
        unsafe {
            if let Some(value) = &self.previous {
                std::env::set_var(self.key, value);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }
}

#[test]
fn tool_defaults_do_not_expose_usr_local_as_rust_state_home() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    std::fs::create_dir_all(home.join(".cargo")).unwrap();
    std::fs::create_dir_all(home.join(".rustup")).unwrap();
    let _home = ScopedEnvVar::set("HOME", &home);
    let _xdg = ScopedEnvVar::unset("XDG_STATE_HOME");
    let _cargo_home = ScopedEnvVar::set(csa_core::env::CARGO_HOME_ENV_KEY, "/usr/local");
    let _rustup_home = ScopedEnvVar::set(csa_core::env::RUSTUP_HOME_ENV_KEY, "/usr/local");

    let plan = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_filesystem_capability(FilesystemCapability::Bwrap)
        .with_tool_defaults(
            "claude-code",
            &PathBuf::from("/tmp/project"),
            &PathBuf::from("/tmp/session"),
        )
        .build()
        .expect("should succeed");

    assert!(plan.writable_paths.contains(&home.join(".cargo")));
    assert!(plan.writable_paths.contains(&home.join(".rustup")));
    assert!(!plan.writable_paths.contains(&PathBuf::from("/usr/local")));
}

#[test]
fn tool_defaults_override_cargo_home_env_when_usr_local_is_readonly() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    std::fs::create_dir_all(home.join(".cargo")).unwrap();
    std::fs::create_dir_all(home.join(".rustup")).unwrap();
    let _home = ScopedEnvVar::set("HOME", &home);
    let _xdg = ScopedEnvVar::unset("XDG_STATE_HOME");
    let _cargo_home = ScopedEnvVar::set(csa_core::env::CARGO_HOME_ENV_KEY, "/usr/local");
    let _rustup_home = ScopedEnvVar::set(csa_core::env::RUSTUP_HOME_ENV_KEY, "/usr/local");

    let plan = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_filesystem_capability(FilesystemCapability::Bwrap)
        .with_tool_defaults(
            "codex",
            &PathBuf::from("/tmp/project"),
            &PathBuf::from("/tmp/session"),
        )
        .build()
        .expect("should succeed");

    // The env var must be overridden to the writable default (#2607).
    assert_eq!(
        plan.env_overrides.get(csa_core::env::CARGO_HOME_ENV_KEY),
        Some(&home.join(".cargo").to_string_lossy().to_string()),
        "CARGO_HOME must be overridden to writable ~/.cargo when original is /usr/local"
    );
    assert_eq!(
        plan.env_overrides.get(csa_core::env::RUSTUP_HOME_ENV_KEY),
        Some(&home.join(".rustup").to_string_lossy().to_string()),
        "RUSTUP_HOME must be overridden to writable ~/.rustup when original is /usr/local"
    );
}
