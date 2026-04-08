use super::*;

fn test_guard(tool_name: &str, session_id: &str) -> CgroupScopeGuard {
    CgroupScopeGuard::new(
        tool_name,
        session_id,
        &SandboxConfig {
            memory_max_mb: 1024,
            memory_swap_max_mb: Some(256),
            pids_max: Some(32),
        },
    )
}

#[test]
fn test_scope_unit_name_basic() {
    let name = scope_unit_name("claude-code", "01JABCDEF");
    assert_eq!(name, "csa-claude-code-01JABCDEF.scope");
}

#[test]
fn test_scope_unit_name_truncation() {
    let long_id = "A".repeat(300);
    let name = scope_unit_name("x", &long_id);
    assert!(
        name.len() <= MAX_SCOPE_NAME_LEN,
        "scope name {} exceeds limit {}",
        name.len(),
        MAX_SCOPE_NAME_LEN,
    );
    assert!(name.starts_with("csa-x-"));
    assert!(name.ends_with(".scope"));
}

#[test]
fn test_create_scope_command_full() {
    let cfg = SandboxConfig {
        memory_max_mb: 4096,
        memory_swap_max_mb: Some(0),
        pids_max: Some(512),
    };
    let cmd = create_scope_command("codex", "01JTEST", &cfg);
    let args: Vec<_> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();

    assert_eq!(cmd.get_program().to_string_lossy(), "systemd-run");
    assert!(args.contains(&"--user".to_string()));
    assert!(args.contains(&"--scope".to_string()));
    assert!(args.contains(&"csa-codex-01JTEST.scope".to_string()));
    assert!(args.contains(&"MemoryMax=4096M".to_string()));
    assert!(args.contains(&"MemorySwapMax=0M".to_string()));
    assert!(args.contains(&"TasksMax=512".to_string()));
    assert!(args.contains(&"--".to_string()));
}

#[test]
fn test_create_scope_command_minimal() {
    let cfg = SandboxConfig {
        memory_max_mb: 1024,
        memory_swap_max_mb: None,
        pids_max: None,
    };
    let cmd = create_scope_command("gemini-cli", "01JXY", &cfg);
    let args: Vec<_> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();

    assert!(args.contains(&"MemoryMax=1024M".to_string()));
    assert!(!args.iter().any(|a| a.contains("MemorySwapMax")));
    assert!(!args.iter().any(|a| a.contains("TasksMax")));
}

#[test]
fn test_create_scope_command_separator_at_end() {
    let cfg = SandboxConfig {
        memory_max_mb: 512,
        memory_swap_max_mb: None,
        pids_max: None,
    };
    let cmd = create_scope_command("t", "s", &cfg);
    let args: Vec<_> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();

    assert_eq!(args.last().unwrap(), "--");
}

#[test]
fn test_create_scope_command_with_env_keeps_secrets_off_command_line() {
    let cfg = SandboxConfig {
        memory_max_mb: 512,
        memory_swap_max_mb: None,
        pids_max: None,
    };
    let env = HashMap::from([
        ("CSA_SUPPRESS_NOTIFY".to_string(), "1".to_string()),
        ("GEMINI_API_KEY".to_string(), "fallback-key".to_string()),
    ]);

    let cmd = create_scope_command_with_env("gemini-cli", "01JENV", &cfg, &env);
    let args: Vec<_> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();

    assert!(
        !args.iter().any(|arg| arg == "-E"),
        "systemd-run scope command must not expose env via -E: {args:?}"
    );
    assert!(
        !args.iter().any(|arg| arg.contains("GEMINI_API_KEY")),
        "secret env values must stay out of the systemd-run argv: {args:?}"
    );
    assert!(
        !args.iter().any(|arg| arg.contains("fallback-key")),
        "secret env contents must stay out of the systemd-run argv: {args:?}"
    );
}

#[test]
fn test_cgroup_scope_guard_name() {
    let guard = test_guard("claude-code", "01JGUARD");
    assert_eq!(guard.scope_name(), "csa-claude-code-01JGUARD.scope");
}

#[test]
fn test_check_oom_killed_returns_false_for_nonexistent_scope() {
    let guard = test_guard("test", "01JNONEXISTENT");
    assert!(!guard.check_oom_killed());
}

#[test]
fn test_memory_peak_returns_none_for_nonexistent_scope() {
    let guard = test_guard("test", "01JNONEXISTENT");
    assert!(guard.memory_peak_mb().is_none());
}

#[test]
fn test_memory_max_returns_none_for_nonexistent_scope() {
    let guard = test_guard("test", "01JNONEXISTENT");
    assert!(guard.memory_max_mb().is_none());
}

#[test]
fn test_oom_diagnosis_returns_none_when_no_oom() {
    let guard = test_guard("test", "01JNONEXISTENT");
    assert!(guard.oom_diagnosis().is_none());
}

#[test]
fn test_check_oom_killed_with_sigkill_fallback_for_missing_scope() {
    let guard = test_guard("test", "01JNONEXISTENT");
    assert!(guard.check_oom_killed_with_signal(Some(SIGKILL)));
}

#[test]
fn test_oom_diagnosis_format_includes_peak_and_swap_limit() {
    let guard = test_guard("test", "01JFORMAT");
    let diagnosis = guard.format_oom_diagnosis(
        Some(1536_u64 * 1024 * 1024),
        Some(2048_u64 * 1024 * 1024),
        Some(512_u64 * 1024 * 1024),
        false,
    );

    assert!(diagnosis.contains("OOM-killed"));
    assert!(diagnosis.contains("peak: 1536MB"));
    assert!(diagnosis.contains("limit: 2048MB"));
    assert!(diagnosis.contains("swap: 512MB"));
}

#[test]
fn test_oom_diagnosis_with_sigkill_fallback_uses_configured_limits() {
    let guard = test_guard("test", "01JNONEXISTENT");
    let diagnosis = guard
        .oom_diagnosis_with_signal(Some(SIGKILL))
        .expect("SIGKILL fallback should infer OOM");

    assert!(diagnosis.contains("likely OOM-killed after scope cleanup"));
    assert!(diagnosis.contains("limit: 1024MB"));
    assert!(diagnosis.contains("swap: 256MB"));
}

#[test]
fn test_oom_diagnosis_zero_swap_includes_hint() {
    let guard = CgroupScopeGuard::new(
        "test",
        "01JZEROSWAP",
        &SandboxConfig {
            memory_max_mb: 1024,
            memory_swap_max_mb: Some(0),
            pids_max: Some(32),
        },
    );
    let diagnosis = guard
        .oom_diagnosis_with_signal(Some(SIGKILL))
        .expect("zero-swap SIGKILL fallback should infer OOM");

    assert!(diagnosis.contains("swap: 0MB"));
    assert!(diagnosis.contains("Swap is disabled"));
}
