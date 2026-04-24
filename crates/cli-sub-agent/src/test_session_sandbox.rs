/// Redirect session state/cache I/O into a tempdir so tests work in sandboxed
/// CI environments (avoids read-only host XDG paths leaking into test runs).
///
/// Also clears `CSA_DAEMON_*` env vars to prevent daemon session ID leaking
/// into fresh test sessions.
///
/// Internally acquires [`TEST_ENV_LOCK`] to serialise all env-mutating tests
/// across the process. Callers do NOT need to acquire any additional lock.
use std::ffi::OsString;
use tokio::sync::OwnedMutexGuard;

use crate::test_env_lock::TEST_ENV_LOCK;

/// RAII guard that sandboxes session env vars and restores them on drop.
///
/// Holds [`TEST_ENV_LOCK`] for its entire lifetime so concurrent tests cannot
/// observe partially-mutated environment state.
pub(crate) struct ScopedSessionSandbox {
    originals: Vec<(&'static str, Option<OsString>)>,
    // Guard is held alive until drop (ordering: restored env *then* lock released).
    _lock: OwnedMutexGuard<()>,
}

impl ScopedSessionSandbox {
    pub(crate) async fn new(tmp: &tempfile::TempDir) -> Self {
        let lock = TEST_ENV_LOCK.clone().lock_owned().await;
        Self::from_guard(tmp, lock)
    }

    pub(crate) fn new_blocking(tmp: &tempfile::TempDir) -> Self {
        let lock = TEST_ENV_LOCK.clone().blocking_lock_owned();
        Self::from_guard(tmp, lock)
    }

    fn from_guard(tmp: &tempfile::TempDir, lock: OwnedMutexGuard<()>) -> Self {
        let keys: &[&'static str] = &[
            "HOME",
            "XDG_STATE_HOME",
            "XDG_CACHE_HOME",
            "CSA_DAEMON_SESSION_ID",
            "CSA_DAEMON_SESSION_DIR",
            "CSA_DAEMON_PROJECT_ROOT",
            // Prevent sa-mode env leak from parent CSA session into test
            // processes (triggers no-op exit gate on fast test executions).
            "CSA_EMIT_CALLER_GUARD_INJECTION",
        ];
        let originals: Vec<_> = keys.iter().map(|k| (*k, std::env::var_os(k))).collect();
        let home_path = tmp.path();
        let state_path = tmp.path().join("state");
        let cache_path = tmp.path().join("cache");
        // SAFETY: test-scoped env mutation protected by TEST_ENV_LOCK.
        unsafe {
            std::env::set_var("HOME", home_path);
            std::env::set_var("XDG_STATE_HOME", state_path.to_str().unwrap());
            std::env::set_var("XDG_CACHE_HOME", cache_path.to_str().unwrap());
            std::env::remove_var("CSA_DAEMON_SESSION_ID");
            std::env::remove_var("CSA_DAEMON_SESSION_DIR");
            std::env::remove_var("CSA_DAEMON_PROJECT_ROOT");
            std::env::remove_var("CSA_EMIT_CALLER_GUARD_INJECTION");
        }
        Self {
            originals,
            _lock: lock,
        }
    }

    /// Snapshot the current value of `key` and restore it when the sandbox drops.
    ///
    /// Call this before mutating any additional env var that is not part of the
    /// sandbox's built-in restore set.
    pub(crate) fn track_env(&mut self, key: &'static str) {
        if self.originals.iter().any(|(tracked, _)| *tracked == key) {
            return;
        }
        self.originals.push((key, std::env::var_os(key)));
    }
}

impl Drop for ScopedSessionSandbox {
    fn drop(&mut self) {
        for (key, val) in &self.originals {
            // SAFETY: restoration of test-scoped env mutation (lock still held).
            unsafe {
                match val {
                    Some(v) => std::env::set_var(key, v),
                    None => std::env::remove_var(key),
                }
            }
        }
        // _lock drops after this, releasing TEST_ENV_LOCK.
    }
}

#[cfg(test)]
mod tests {
    use super::ScopedSessionSandbox;

    #[test]
    fn tracked_env_var_is_restored_on_drop() {
        const KEY: &str = "CSA_TEST_LEAK_PROBE_TRACKED";
        let td = tempfile::tempdir().expect("tempdir");

        unsafe { std::env::remove_var(KEY) };

        {
            let mut sandbox = ScopedSessionSandbox::new_blocking(&td);
            sandbox.track_env(KEY);

            // SAFETY: test-scoped env mutation while ScopedSessionSandbox holds TEST_ENV_LOCK.
            unsafe { std::env::set_var(KEY, "sandbox-value") };
            assert_eq!(std::env::var(KEY).as_deref(), Ok("sandbox-value"));
        }

        assert_eq!(
            std::env::var(KEY),
            Err(std::env::VarError::NotPresent),
            "tracked env var should be removed when it did not exist before sandboxing"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sandbox_lock_can_span_await_without_deadlocking() {
        let td = tempfile::tempdir().expect("tempdir");
        let _sandbox = ScopedSessionSandbox::new(&td).await;

        tokio::task::yield_now().await;
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}
