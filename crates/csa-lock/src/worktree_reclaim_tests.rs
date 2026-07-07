use super::*;
use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::process::{Child, Command};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;
use tempfile::tempdir;

fn env_test_lock() -> MutexGuard<'static, ()> {
    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

struct EnvVarGuard {
    key: &'static str,
    original: Option<OsString>,
}

impl EnvVarGuard {
    fn set_os(key: &'static str, value: &Path) -> Self {
        let original = std::env::var_os(key);
        // SAFETY: test-scoped env mutation is serialized by `env_test_lock`.
        unsafe { std::env::set_var(key, value) };
        Self { key, original }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match self.original.take() {
            // SAFETY: test-scoped env restoration is serialized by `env_test_lock`.
            Some(value) => unsafe { std::env::set_var(self.key, value) },
            // SAFETY: test-scoped env restoration is serialized by `env_test_lock`.
            None => unsafe { std::env::remove_var(self.key) },
        }
    }
}

#[test]
#[cfg(target_os = "linux")]
fn worktree_write_lock_reclaims_terminal_session_after_holder_crash() {
    let _env_lock = env_test_lock();
    let state_home = tempdir().expect("state-home tempdir");
    let _state_guard = EnvVarGuard::set_os("XDG_STATE_HOME", state_home.path());

    let worktree_root = tempdir().expect("worktree tempdir");
    let ready_path = worktree_root.path().join("holder-ready");
    let mut holder = spawn_terminal_reclaim_holder(
        state_home.path(),
        worktree_root.path(),
        "01TERMINAL",
        &ready_path,
        true,
    );
    wait_for_ready_marker(&ready_path);
    holder.wait_for_exit();

    let lock = acquire_worktree_write_lock(
        worktree_root.path(),
        "01NEXT",
        &[],
        |_| false,
        |holder_session_id| holder_session_id == "01TERMINAL",
        |_| false,
    )
    .expect("terminal holder session with free flock should be reclaimed");

    assert!(!lock.is_lineage_reentry());
}

#[test]
fn worktree_write_lock_keeps_live_nonterminal_holder_blocked() {
    let _env_lock = env_test_lock();
    let state_home = tempdir().expect("state-home tempdir");
    let _state_guard = EnvVarGuard::set_os("XDG_STATE_HOME", state_home.path());

    let worktree_root = tempdir().expect("worktree tempdir");
    let ready_path = worktree_root.path().join("holder-ready");
    let mut holder = spawn_terminal_reclaim_holder(
        state_home.path(),
        worktree_root.path(),
        "01ACTIVE",
        &ready_path,
        false,
    );
    wait_for_ready_marker(&ready_path);

    let err = acquire_worktree_write_lock(
        worktree_root.path(),
        "01NEXT",
        &[],
        |_| false,
        |_| false,
        |_| false,
    )
    .expect_err("live nonterminal holder must still block")
    .to_string();

    assert!(err.contains("concurrent write session blocked"));
    assert!(err.contains("01ACTIVE"));
    assert!(holder.is_running());
}

#[test]
#[cfg(target_os = "linux")]
fn worktree_write_lock_sigterms_alive_stale_result_holder() {
    let _env_lock = env_test_lock();
    let state_home = tempdir().expect("state-home tempdir");
    let _state_guard = EnvVarGuard::set_os("XDG_STATE_HOME", state_home.path());

    let worktree_root = tempdir().expect("worktree tempdir");
    let ready_path = worktree_root.path().join("holder-ready");
    let mut holder = spawn_terminal_reclaim_holder(
        state_home.path(),
        worktree_root.path(),
        "01STALE",
        &ready_path,
        false,
    );
    wait_for_ready_marker(&ready_path);

    let lock = acquire_worktree_write_lock(
        worktree_root.path(),
        "01NEXT",
        &[],
        |_| false,
        |_| false,
        |holder_session_id| holder_session_id == "01STALE",
    )
    .expect("alive stale holder should be terminated and reclaimed");

    assert!(!lock.is_lineage_reentry());
    holder.wait_for_exit();
}

#[test]
fn terminal_reclaim_holder_child_entrypoint() {
    let Some(worktree_root) = std::env::var_os("CSA_LOCK_TERMINAL_RECLAIM_WORKTREE") else {
        return;
    };
    let Some(ready_path) = std::env::var_os("CSA_LOCK_TERMINAL_RECLAIM_READY") else {
        return;
    };
    let holder_session_id =
        std::env::var("CSA_LOCK_TERMINAL_RECLAIM_HOLDER").expect("holder session id env");

    let _lock = acquire_worktree_write_lock(
        Path::new(&worktree_root),
        &holder_session_id,
        &[],
        |_| false,
        |_| false,
        |_| false,
    )
    .expect("child holder should acquire worktree write lock");
    fs::write(ready_path, b"ready").expect("write ready marker");

    if std::env::var_os("CSA_LOCK_TERMINAL_RECLAIM_EXIT").is_some() {
        std::process::exit(0);
    }

    loop {
        std::thread::sleep(Duration::from_secs(60));
    }
}

fn spawn_terminal_reclaim_holder(
    state_home: &Path,
    worktree_root: &Path,
    holder_session_id: &str,
    ready_path: &Path,
    exit_after_ready: bool,
) -> ChildGuard {
    let mut cmd = Command::new(std::env::current_exe().expect("current test binary"));
    cmd.arg("terminal_reclaim_holder_child_entrypoint")
        .arg("--nocapture")
        .env("XDG_STATE_HOME", state_home)
        .env("CSA_LOCK_TERMINAL_RECLAIM_WORKTREE", worktree_root)
        .env("CSA_LOCK_TERMINAL_RECLAIM_HOLDER", holder_session_id)
        .env("CSA_LOCK_TERMINAL_RECLAIM_READY", ready_path);
    if exit_after_ready {
        cmd.env("CSA_LOCK_TERMINAL_RECLAIM_EXIT", "1");
    }
    let child = cmd.spawn().expect("spawn holder child process");
    ChildGuard { child }
}

fn wait_for_ready_marker(ready_path: &Path) {
    for _ in 0..100 {
        if ready_path.exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("holder child did not report ready");
}

struct ChildGuard {
    child: Child,
}

impl ChildGuard {
    fn is_running(&mut self) -> bool {
        self.child.try_wait().expect("check child status").is_none()
    }

    fn wait_for_exit(&mut self) {
        for _ in 0..100 {
            if self.child.try_wait().expect("check child status").is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("holder child did not exit after stale lock reclaim");
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}
