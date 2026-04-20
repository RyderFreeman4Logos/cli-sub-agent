/// Redirect session I/O into a tempdir so tests work in sandboxed CI environments
/// (avoids "Read-only file system" on the real XDG_STATE_HOME).
///
/// Also clears `CSA_DAEMON_*` env vars to prevent daemon session ID leaking
/// into fresh test sessions.
///
/// Internally acquires [`TEST_ENV_LOCK`] to serialise all env-mutating tests
/// across the process. Callers do NOT need to acquire any additional lock.
use std::ffi::OsString;
use std::sync::MutexGuard;

use crate::test_env_lock::TEST_ENV_LOCK;

/// RAII guard that sandboxes session env vars and restores them on drop.
///
/// Holds [`TEST_ENV_LOCK`] for its entire lifetime so concurrent tests cannot
/// observe partially-mutated environment state.
pub(crate) struct ScopedSessionSandbox {
    originals: Vec<(&'static str, Option<OsString>)>,
    // Guard is held alive until drop (ordering: restored env *then* lock released).
    _lock: MutexGuard<'static, ()>,
}

impl ScopedSessionSandbox {
    pub(crate) fn new(tmp: &tempfile::TempDir) -> Self {
        let lock = TEST_ENV_LOCK.lock().expect("TEST_ENV_LOCK poisoned");

        let keys: &[&'static str] = &[
            "XDG_STATE_HOME",
            "CSA_DAEMON_SESSION_ID",
            "CSA_DAEMON_SESSION_DIR",
            "CSA_DAEMON_PROJECT_ROOT",
        ];
        let originals: Vec<_> = keys.iter().map(|k| (*k, std::env::var_os(k))).collect();
        let state_path = tmp.path().join("state");
        // SAFETY: test-scoped env mutation protected by TEST_ENV_LOCK.
        unsafe {
            std::env::set_var("XDG_STATE_HOME", state_path.to_str().unwrap());
            std::env::remove_var("CSA_DAEMON_SESSION_ID");
            std::env::remove_var("CSA_DAEMON_SESSION_DIR");
            std::env::remove_var("CSA_DAEMON_PROJECT_ROOT");
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
            let mut sandbox = ScopedSessionSandbox::new(&td);
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
}
