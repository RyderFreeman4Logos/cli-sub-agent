use super::prompt_guard::{PROMPT_GUARD_CALLER_INJECTION_ENV, should_emit_prompt_guard_to_caller};
use crate::test_env_lock::TEST_ENV_LOCK;

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
    let _env_lock = TEST_ENV_LOCK
        .lock()
        .expect("prompt guard env lock poisoned");
    let original_guard = std::env::var(PROMPT_GUARD_CALLER_INJECTION_ENV).ok();
    let original_depth = std::env::var("CSA_DEPTH").ok();
    // SAFETY: test-scoped env mutation, restored immediately.
    unsafe {
        std::env::remove_var(PROMPT_GUARD_CALLER_INJECTION_ENV);
        std::env::remove_var("CSA_DEPTH");
    }
    let enabled = should_emit_prompt_guard_to_caller();
    restore_env_var(PROMPT_GUARD_CALLER_INJECTION_ENV, original_guard);
    restore_env_var("CSA_DEPTH", original_depth);
    assert!(enabled);
}

#[test]
fn prompt_guard_caller_injection_honors_disable_values() {
    let _env_lock = TEST_ENV_LOCK
        .lock()
        .expect("prompt guard env lock poisoned");
    let original_guard = std::env::var(PROMPT_GUARD_CALLER_INJECTION_ENV).ok();
    let original_depth = std::env::var("CSA_DEPTH").ok();
    // SAFETY: test-scoped env mutation, restored immediately.
    unsafe { std::env::remove_var("CSA_DEPTH") };

    for value in ["0", "false", "off", "no", "FALSE"] {
        // SAFETY: test-scoped env mutation, restored immediately.
        unsafe { std::env::set_var(PROMPT_GUARD_CALLER_INJECTION_ENV, value) };
        assert!(
            !should_emit_prompt_guard_to_caller(),
            "expected value '{value}' to disable caller injection"
        );
    }

    restore_env_var(PROMPT_GUARD_CALLER_INJECTION_ENV, original_guard);
    restore_env_var("CSA_DEPTH", original_depth);
}

#[test]
fn prompt_guard_caller_injection_disabled_for_recursive_depth() {
    let _env_lock = TEST_ENV_LOCK
        .lock()
        .expect("prompt guard env lock poisoned");
    let original_guard = std::env::var(PROMPT_GUARD_CALLER_INJECTION_ENV).ok();
    let original_depth = std::env::var("CSA_DEPTH").ok();

    // SAFETY: test-scoped env mutation, restored immediately.
    unsafe {
        std::env::set_var(PROMPT_GUARD_CALLER_INJECTION_ENV, "true");
        std::env::set_var("CSA_DEPTH", "1");
    }

    assert!(
        !should_emit_prompt_guard_to_caller(),
        "recursive depth should suppress caller prompt injection"
    );

    restore_env_var(PROMPT_GUARD_CALLER_INJECTION_ENV, original_guard);
    restore_env_var("CSA_DEPTH", original_depth);
}

#[test]
fn anti_recursion_guard_none_at_depth_zero() {
    let _env_lock = TEST_ENV_LOCK
        .lock()
        .expect("prompt guard env lock poisoned");
    let original_depth = std::env::var("CSA_DEPTH").ok();
    // SAFETY: test-scoped env mutation, restored immediately.
    unsafe { std::env::remove_var("CSA_DEPTH") };
    let guard = super::prompt_guard::anti_recursion_guard();
    restore_env_var("CSA_DEPTH", original_depth);
    assert!(guard.is_none(), "should be None at depth 0");
}

#[test]
fn anti_recursion_guard_present_at_depth_one() {
    let _env_lock = TEST_ENV_LOCK
        .lock()
        .expect("prompt guard env lock poisoned");
    let original_depth = std::env::var("CSA_DEPTH").ok();
    // SAFETY: test-scoped env mutation, restored immediately.
    unsafe { std::env::set_var("CSA_DEPTH", "1") };
    let guard = super::prompt_guard::anti_recursion_guard();
    restore_env_var("CSA_DEPTH", original_depth);
    let guard = guard.expect("should return Some at depth > 0");
    assert!(guard.contains("csa-anti-recursion"));
    assert!(guard.contains("depth=1"));
    assert!(guard.contains("MUST NOT delegate"));
}
