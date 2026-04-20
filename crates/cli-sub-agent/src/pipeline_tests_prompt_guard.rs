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
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
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
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
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
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
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
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let original_depth = std::env::var("CSA_DEPTH").ok();
    // SAFETY: test-scoped env mutation, restored immediately.
    unsafe { std::env::remove_var("CSA_DEPTH") };
    let guard = super::prompt_guard::anti_recursion_guard(None);
    restore_env_var("CSA_DEPTH", original_depth);
    assert!(guard.is_none(), "should be None at depth 0");
}

#[test]
fn anti_recursion_guard_none_for_legitimate_fractal_depths() {
    // Layer 1 → Layer 2 (depth 1 → 2) and Layer 2 → Layer 3 (depth 2 → 3) are
    // documented fractal-recursion cases; the guard must not discourage them.
    for depth in ["1", "2", "3"] {
        let _env_lock = TEST_ENV_LOCK.blocking_lock();
        let original_depth = std::env::var("CSA_DEPTH").ok();
        // SAFETY: test-scoped env mutation, restored immediately.
        unsafe { std::env::set_var("CSA_DEPTH", depth) };
        let guard = super::prompt_guard::anti_recursion_guard(None);
        restore_env_var("CSA_DEPTH", original_depth);
        assert!(
            guard.is_none(),
            "guard should not fire below default ceiling (depth={depth})"
        );
    }
}

#[test]
fn anti_recursion_guard_warns_near_default_ceiling() {
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let original_depth = std::env::var("CSA_DEPTH").ok();
    // SAFETY: test-scoped env mutation, restored immediately.
    unsafe { std::env::set_var("CSA_DEPTH", "4") };
    let guard = super::prompt_guard::anti_recursion_guard(None);
    restore_env_var("CSA_DEPTH", original_depth);
    let guard = guard.expect("should return Some near recursion ceiling (depth=4)");
    assert!(guard.contains("csa-depth-ceiling"));
    assert!(guard.contains("depth=\"4\""));
    assert!(guard.contains("max=\"5\""));
    assert!(
        !guard.contains("MUST NOT delegate"),
        "ceiling warning must be advisory, not blanket prohibition"
    );
}

#[test]
fn anti_recursion_guard_warns_at_default_ceiling() {
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let original_depth = std::env::var("CSA_DEPTH").ok();
    // SAFETY: test-scoped env mutation, restored immediately.
    unsafe { std::env::set_var("CSA_DEPTH", "5") };
    let guard = super::prompt_guard::anti_recursion_guard(None);
    restore_env_var("CSA_DEPTH", original_depth);
    let guard = guard.expect("should return Some at recursion ceiling");
    assert!(guard.contains("csa-depth-ceiling"));
    assert!(guard.contains("depth=\"5\""));
}

#[test]
fn anti_recursion_guard_honors_custom_max_recursion_depth() {
    use csa_config::config::CURRENT_SCHEMA_VERSION;
    use csa_config::{ProjectConfig, ProjectMeta, ResourcesConfig};
    use std::collections::HashMap;

    fn project_config_with_max_depth(max_depth: u32) -> ProjectConfig {
        ProjectConfig {
            schema_version: CURRENT_SCHEMA_VERSION,
            project: ProjectMeta {
                name: "test-project".to_string(),
                created_at: chrono::Utc::now(),
                max_recursion_depth: max_depth,
            },
            resources: ResourcesConfig {
                min_free_memory_mb: 4096,
                idle_timeout_seconds: 250,
                ..Default::default()
            },
            acp: Default::default(),
            tools: HashMap::new(),
            review: None,
            debate: None,
            tiers: HashMap::new(),
            tier_mapping: HashMap::new(),
            aliases: HashMap::new(),
            tool_aliases: HashMap::new(),
            preferences: None,
            session: Default::default(),
            memory: Default::default(),
            hooks: Default::default(),
            execution: Default::default(),
            vcs: Default::default(),
            filesystem_sandbox: Default::default(),
        }
    }

    // Custom ceiling = 3: guard must fire at depth 2 (one below ceiling), stay
    // silent at depth 1, and carry the project-configured ceiling in the
    // rendered text so LLMs don't see a stale "max=5" number.
    let cfg_low = project_config_with_max_depth(3);

    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let original_depth = std::env::var("CSA_DEPTH").ok();

    // SAFETY: test-scoped env mutation, restored immediately.
    unsafe { std::env::set_var("CSA_DEPTH", "1") };
    let below = super::prompt_guard::anti_recursion_guard(Some(&cfg_low));
    assert!(
        below.is_none(),
        "guard should not fire at depth 1 when ceiling=3 (remaining=2)"
    );

    // SAFETY: test-scoped env mutation, restored immediately.
    unsafe { std::env::set_var("CSA_DEPTH", "2") };
    let near = super::prompt_guard::anti_recursion_guard(Some(&cfg_low));
    restore_env_var("CSA_DEPTH", original_depth);

    let near = near.expect("guard should fire at depth 2 when ceiling=3");
    assert!(near.contains("max=\"3\""));
    assert!(near.contains("depth=\"2\""));
}
