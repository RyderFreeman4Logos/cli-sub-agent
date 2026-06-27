// NOTE #1858: #[path]-included by tests; no `crate::`, no binary-only methods (dead_code).
use std::ffi::OsString;
use std::path::Path;
use std::sync::{Arc, LazyLock};
use tokio::sync::{Mutex, OwnedMutexGuard};

#[allow(dead_code)]
pub(crate) static TEST_ENV_LOCK: LazyLock<Arc<Mutex<()>>> =
    LazyLock::new(|| Arc::new(Mutex::new(())));

fn restore_env_snapshot(key: &'static str, previous: Option<OsString>) {
    // SAFETY: all tests in this crate that mutate process env must hold TEST_ENV_LOCK
    // for the entire lifetime of the restore guard; private per-module env locks are forbidden.
    unsafe {
        match previous {
            Some(value) => std::env::set_var(key, value),
            None => std::env::remove_var(key),
        }
    }
}

#[allow(dead_code)]
pub(crate) struct ScopedEnvVarRestore {
    key: &'static str,
    original: Option<OsString>,
}

#[allow(dead_code)]
impl ScopedEnvVarRestore {
    #[allow(dead_code)]
    pub(crate) fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let original = std::env::var_os(key);
        // SAFETY: all tests in this crate that touch process env must hold TEST_ENV_LOCK;
        // private per-module env locks are forbidden because env is process-wide.
        unsafe { std::env::set_var(key, value) };
        Self { key, original }
    }

    #[allow(dead_code)]
    pub(crate) fn unset(key: &'static str) -> Self {
        let original = std::env::var_os(key);
        // SAFETY: all tests in this crate that touch process env must hold TEST_ENV_LOCK;
        // private per-module env locks are forbidden because env is process-wide.
        unsafe { std::env::remove_var(key) };
        Self { key, original }
    }
}

#[allow(dead_code)]
pub(crate) fn isolate_user_config(
    project_root: &Path,
) -> (ScopedEnvVarRestore, ScopedEnvVarRestore) {
    let config_home = project_root.join("xdg-config");
    std::fs::create_dir_all(&config_home).expect("test config home should be created");
    let home_guard = ScopedEnvVarRestore::set("HOME", project_root);
    let config_guard = ScopedEnvVarRestore::set("XDG_CONFIG_HOME", &config_home);
    (home_guard, config_guard)
}

#[allow(dead_code)]
pub(crate) struct ScopedUserConfigIsolation {
    _config_guard: ScopedEnvVarRestore,
    _home_guard: ScopedEnvVarRestore,
    _lock: OwnedMutexGuard<()>,
}

#[allow(dead_code)]
pub(crate) async fn isolate_user_config_locked(project_root: &Path) -> ScopedUserConfigIsolation {
    let lock = TEST_ENV_LOCK.clone().lock_owned().await;
    let (home_guard, config_guard) = isolate_user_config(project_root);
    ScopedUserConfigIsolation {
        _config_guard: config_guard,
        _home_guard: home_guard,
        _lock: lock,
    }
}

impl Drop for ScopedEnvVarRestore {
    fn drop(&mut self) {
        restore_env_snapshot(self.key, self.original.take());
    }
}

#[allow(dead_code)]
pub(crate) struct ScopedTestEnvVar {
    key: &'static str,
    original: Option<OsString>,
    _lock: OwnedMutexGuard<()>,
}

impl ScopedTestEnvVar {
    #[allow(dead_code)]
    pub(crate) fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let lock = TEST_ENV_LOCK.clone().blocking_lock_owned();
        let original = std::env::var_os(key);
        // SAFETY: test-scoped env mutation guarded by a process-wide mutex.
        unsafe { std::env::set_var(key, value) };
        Self {
            key,
            original,
            _lock: lock,
        }
    }
}

impl Drop for ScopedTestEnvVar {
    fn drop(&mut self) {
        restore_env_snapshot(self.key, self.original.take());
    }
}
