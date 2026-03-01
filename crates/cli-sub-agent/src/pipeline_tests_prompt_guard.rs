use super::prompt_guard::{PROMPT_GUARD_CALLER_INJECTION_ENV, should_emit_prompt_guard_to_caller};
use std::sync::{LazyLock, Mutex};

static PROMPT_GUARD_ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

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
fn prompt_guard_caller_injection_defaults_to_enabled() {
    let _env_lock = PROMPT_GUARD_ENV_LOCK
        .lock()
        .expect("prompt guard env lock poisoned");
    let original = std::env::var(PROMPT_GUARD_CALLER_INJECTION_ENV).ok();
    // SAFETY: test-scoped env mutation, restored immediately.
    unsafe { std::env::remove_var(PROMPT_GUARD_CALLER_INJECTION_ENV) };
    let enabled = should_emit_prompt_guard_to_caller();
    restore_env_var(PROMPT_GUARD_CALLER_INJECTION_ENV, original);
    assert!(enabled);
}

#[test]
fn prompt_guard_caller_injection_honors_disable_values() {
    let _env_lock = PROMPT_GUARD_ENV_LOCK
        .lock()
        .expect("prompt guard env lock poisoned");
    let original = std::env::var(PROMPT_GUARD_CALLER_INJECTION_ENV).ok();
    for value in ["0", "false", "off", "no", "FALSE"] {
        // SAFETY: test-scoped env mutation, restored immediately.
        unsafe { std::env::set_var(PROMPT_GUARD_CALLER_INJECTION_ENV, value) };
        assert!(
            !should_emit_prompt_guard_to_caller(),
            "expected value '{value}' to disable caller injection"
        );
    }
    restore_env_var(PROMPT_GUARD_CALLER_INJECTION_ENV, original);
}
