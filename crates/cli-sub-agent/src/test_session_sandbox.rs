// NOTE #1858: #[path]-included by tests; no `crate::`, no binary-only methods (dead_code).
/// Redirect session and Rust state/cache I/O into a tempdir so tests work in
/// sandboxed CI environments (avoids read-only host paths leaking into tests).
///
/// Also clears `CSA_DAEMON_*` env vars to prevent daemon session ID leaking
/// into fresh test sessions.
/// `PATH` and `MISE_DATA_DIR` remain untouched so installed toolchains stay
/// executable while their writable state is isolated.
///
/// Internally acquires [`TEST_ENV_LOCK`] to serialise all env-mutating tests
/// across the process. Callers do NOT need to acquire any additional lock.
use std::ffi::OsString;
use tokio::sync::OwnedMutexGuard;

use super::test_env_lock::TEST_ENV_LOCK;

// Keep these keys aligned with `pipeline_env::rust_session_writable_paths`.
// `MISE_DATA_DIR` holds installed toolchains and is intentionally excluded.
const RUST_STATE_HOME_ENV_DIRS: &[(&str, &str)] = &[
    (csa_core::env::CARGO_HOME_ENV_KEY, "rust-state/cargo-home"),
    (csa_core::env::RUSTUP_HOME_ENV_KEY, "rust-state/rustup-home"),
    (
        csa_core::env::CARGO_INSTALL_ROOT_ENV_KEY,
        "rust-state/cargo-install-root",
    ),
    (
        csa_core::env::CARGO_TARGET_DIR_ENV_KEY,
        "rust-state/cargo-target",
    ),
    (
        csa_core::env::MISE_CONFIG_DIR_ENV_KEY,
        "rust-state/mise-config",
    ),
];

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
        let session_keys: &[&'static str] = &[
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
        let rust_state_paths: Vec<_> = RUST_STATE_HOME_ENV_DIRS
            .iter()
            .map(|(key, relative)| (*key, tmp.path().join(relative)))
            .collect();
        for (key, path) in &rust_state_paths {
            std::fs::create_dir_all(path)
                .unwrap_or_else(|error| panic!("create sandboxed {key} directory: {error}"));
            if *key == csa_core::env::CARGO_HOME_ENV_KEY {
                for child in ["git", "registry"] {
                    std::fs::create_dir_all(path.join(child)).unwrap_or_else(|error| {
                        panic!("create sandboxed Cargo {child} cache directory: {error}")
                    });
                }
            }
        }
        let originals: Vec<_> = session_keys
            .iter()
            .copied()
            .chain(rust_state_paths.iter().map(|(key, _)| *key))
            .map(|key| (key, std::env::var_os(key)))
            .collect();
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
            for (key, path) in rust_state_paths {
                std::env::set_var(key, path);
            }
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
    use std::path::PathBuf;
    use std::process::{Command, Output, Stdio};
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    const RUST_STATE_HOME_ENV_KEYS: &[&str] = &[
        csa_core::env::CARGO_HOME_ENV_KEY,
        csa_core::env::RUSTUP_HOME_ENV_KEY,
        csa_core::env::CARGO_INSTALL_ROOT_ENV_KEY,
        csa_core::env::CARGO_TARGET_DIR_ENV_KEY,
        csa_core::env::MISE_CONFIG_DIR_ENV_KEY,
    ];
    const RUST_STATE_CHILD_MARKER: &str = "CSA_TEST_SCOPED_SESSION_RUST_STATE_CHILD";
    const RUST_STATE_TEST_NAME: &str =
        "test_session_sandbox::tests::rust_state_env_is_sandboxed_and_restored";

    fn output_with_timeout(mut command: Command, timeout: Duration) -> Output {
        // Keep this helper local: the file is #[path]-included by integration tests.
        use std::os::unix::process::CommandExt;
        use std::time::Instant;

        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        // SAFETY: only setpgid(0, 0) in the child before exec.
        unsafe {
            command.pre_exec(|| {
                if libc::setpgid(0, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = command.spawn().expect("spawn bounded test command");
        let pid = child.id() as i32;
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(child.wait_with_output());
        });
        match rx.recv_timeout(timeout) {
            Ok(Ok(output)) => output,
            Ok(Err(error)) => panic!("bounded test command failed to wait: {error}"),
            Err(_) => {
                // SAFETY: pid is the child we just spawned into its own group.
                unsafe {
                    let _ = libc::kill(-pid, libc::SIGTERM);
                }
                thread::sleep(Duration::from_millis(50));
                unsafe {
                    let _ = libc::kill(-pid, libc::SIGKILL);
                }
                let deadline = Instant::now() + Duration::from_secs(2);
                loop {
                    match rx.recv_timeout(Duration::from_millis(50)) {
                        Ok(Ok(_)) | Ok(Err(_)) => break,
                        Err(_) if Instant::now() < deadline => continue,
                        Err(_) => break,
                    }
                }
                panic!("bounded test command exceeded {timeout:?}");
            }
        }
    }

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

    #[test]
    fn rust_state_env_is_sandboxed_and_restored() {
        let sandboxed_keys: Vec<_> = super::RUST_STATE_HOME_ENV_DIRS
            .iter()
            .map(|(key, _)| *key)
            .collect();
        assert_eq!(sandboxed_keys, RUST_STATE_HOME_ENV_KEYS);

        if std::env::var_os(RUST_STATE_CHILD_MARKER).is_none() {
            let mut child = Command::new(std::env::current_exe().expect("current test executable"));
            child
                .arg("--exact")
                .arg(RUST_STATE_TEST_NAME)
                .arg("--nocapture")
                .env(RUST_STATE_CHILD_MARKER, "1");
            for (index, key) in RUST_STATE_HOME_ENV_KEYS.iter().enumerate() {
                if index % 2 == 0 {
                    child.env(key, format!("/ambient/rust-state-{index}"));
                } else {
                    child.env_remove(key);
                }
            }

            let output = output_with_timeout(child, Duration::from_secs(120));
            assert!(
                output.status.success(),
                "isolated Rust state child test failed\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }

        let before: Vec<_> = RUST_STATE_HOME_ENV_KEYS
            .iter()
            .map(|key| (*key, std::env::var_os(key)))
            .collect();
        let path_before = std::env::var_os("PATH");
        let mise_data_before = std::env::var_os(csa_core::env::MISE_DATA_DIR_ENV_KEY);
        let td = tempfile::tempdir().expect("tempdir");

        {
            let _sandbox = ScopedSessionSandbox::new_blocking(&td);
            for key in RUST_STATE_HOME_ENV_KEYS {
                let path = PathBuf::from(
                    std::env::var_os(key).unwrap_or_else(|| panic!("{key} should be set")),
                );
                assert!(
                    path.starts_with(td.path()),
                    "{key} should be redirected beneath the sandbox root, got {}",
                    path.display()
                );
                assert!(
                    path.is_dir(),
                    "{key} directory should exist: {}",
                    path.display()
                );
                std::fs::write(path.join("writable-probe"), b"ok")
                    .unwrap_or_else(|error| panic!("{key} should be writable: {error}"));
            }

            let cargo_home = PathBuf::from(
                std::env::var_os(csa_core::env::CARGO_HOME_ENV_KEY)
                    .expect("CARGO_HOME should be set"),
            );
            for child in ["git", "registry"] {
                assert!(
                    cargo_home.join(child).is_dir(),
                    "Cargo {child} cache directory should exist"
                );
            }
            assert_eq!(std::env::var_os("PATH"), path_before);
            assert_eq!(
                std::env::var_os(csa_core::env::MISE_DATA_DIR_ENV_KEY),
                mise_data_before
            );
        }

        for (key, expected) in before {
            assert_eq!(
                std::env::var_os(key),
                expected,
                "{key} should be restored exactly on drop"
            );
        }
        assert_eq!(std::env::var_os("PATH"), path_before);
        assert_eq!(
            std::env::var_os(csa_core::env::MISE_DATA_DIR_ENV_KEY),
            mise_data_before
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
