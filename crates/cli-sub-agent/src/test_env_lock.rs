use std::ffi::OsString;
use std::sync::{LazyLock, Mutex, MutexGuard};

#[allow(dead_code)]
pub(crate) static TEST_ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

pub(crate) struct ScopedEnvVarRestore {
    key: &'static str,
    original: Option<OsString>,
}

impl ScopedEnvVarRestore {
    pub(crate) fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let original = std::env::var_os(key);
        // SAFETY: caller must hold TEST_ENV_LOCK or another equivalent test-only env lock.
        unsafe { std::env::set_var(key, value) };
        Self { key, original }
    }

    pub(crate) fn unset(key: &'static str) -> Self {
        let original = std::env::var_os(key);
        // SAFETY: caller must hold TEST_ENV_LOCK or another equivalent test-only env lock.
        unsafe { std::env::remove_var(key) };
        Self { key, original }
    }
}

impl Drop for ScopedEnvVarRestore {
    fn drop(&mut self) {
        // SAFETY: caller holds TEST_ENV_LOCK or another equivalent test-only env lock
        // for the entire lifetime of this guard.
        unsafe {
            match self.original.take() {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
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
        // SAFETY: restoration of test-scoped env mutation guarded by a process-wide mutex.
        unsafe {
            match self.original.take() {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }
}
