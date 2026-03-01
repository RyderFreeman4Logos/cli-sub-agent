use super::*;
use std::sync::{LazyLock, Mutex};

static HEARTBEAT_ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

fn restore_env_var(key: &str, original: Option<String>) {
    // SAFETY: test-scoped env mutation guarded by a process-wide mutex.
    unsafe {
        match original {
            Some(value) => std::env::set_var(key, value),
            None => std::env::remove_var(key),
        }
    }
}

#[test]
fn test_resolve_heartbeat_interval_default_enabled() {
    let _env_lock = HEARTBEAT_ENV_LOCK
        .lock()
        .expect("heartbeat env lock poisoned");
    let original = std::env::var(HEARTBEAT_INTERVAL_ENV).ok();
    // SAFETY: test-scoped env mutation, restored immediately.
    unsafe { std::env::remove_var(HEARTBEAT_INTERVAL_ENV) };
    let resolved = resolve_heartbeat_interval();
    restore_env_var(HEARTBEAT_INTERVAL_ENV, original);
    assert_eq!(resolved, Some(Duration::from_secs(DEFAULT_HEARTBEAT_SECS)));
}

#[test]
fn test_resolve_heartbeat_interval_disable_with_zero() {
    let _env_lock = HEARTBEAT_ENV_LOCK
        .lock()
        .expect("heartbeat env lock poisoned");
    let original = std::env::var(HEARTBEAT_INTERVAL_ENV).ok();
    // SAFETY: test-scoped env mutation, restored immediately.
    unsafe { std::env::set_var(HEARTBEAT_INTERVAL_ENV, "0") };
    let resolved = resolve_heartbeat_interval();
    restore_env_var(HEARTBEAT_INTERVAL_ENV, original);
    assert_eq!(resolved, None);
}
