use std::ffi::OsString;
use std::sync::{LazyLock, Mutex, MutexGuard};

pub(crate) static TEST_ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

pub(crate) struct ScopedTestEnvVar {
    key: &'static str,
    original: Option<OsString>,
    _lock: MutexGuard<'static, ()>,
}

impl ScopedTestEnvVar {
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
