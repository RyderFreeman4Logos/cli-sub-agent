use std::ffi::OsString;
use std::sync::{LazyLock, Mutex, MutexGuard};

#[allow(dead_code)]
pub(crate) static TEST_ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

fn restore_env_var(key: &'static str, original: &mut Option<OsString>) {
    // SAFETY: all tests in this crate that mutate process env must hold TEST_ENV_LOCK
    // for the entire lifetime of the restore guard; private per-module env locks are forbidden.
    unsafe {
        match original.take() {
            Some(value) => std::env::set_var(key, value),
            None => std::env::remove_var(key),
        }
    }
}

pub(crate) struct ScopedEnvVarRestore {
    key: &'static str,
    original: Option<OsString>,
}

impl ScopedEnvVarRestore {
    pub(crate) fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let original = std::env::var_os(key);
        // SAFETY: all tests in this crate that touch process env must hold TEST_ENV_LOCK;
        // private per-module env locks are forbidden because env is process-wide.
        unsafe { std::env::set_var(key, value) };
        Self { key, original }
    }

    pub(crate) fn unset(key: &'static str) -> Self {
        let original = std::env::var_os(key);
        // SAFETY: all tests in this crate that touch process env must hold TEST_ENV_LOCK;
        // private per-module env locks are forbidden because env is process-wide.
        unsafe { std::env::remove_var(key) };
        Self { key, original }
    }
}

impl Drop for ScopedEnvVarRestore {
    fn drop(&mut self) {
        restore_env_var(self.key, &mut self.original);
    }
}

#[allow(dead_code)]
pub(crate) struct ScopedTestEnvVar {
    key: &'static str,
    original: Option<OsString>,
    _lock: MutexGuard<'static, ()>,
}

impl ScopedTestEnvVar {
    #[allow(dead_code)]
    pub(crate) fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let lock = TEST_ENV_LOCK.lock().unwrap();
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
        restore_env_var(self.key, &mut self.original);
    }
}
