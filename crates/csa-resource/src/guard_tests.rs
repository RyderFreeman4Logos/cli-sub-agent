use super::*;

#[test]
fn test_resource_guard_new_default_limits() {
    let limits = ResourceLimits::default();
    let _guard = ResourceGuard::new(limits);
    assert_eq!(ResourceLimits::default().min_free_memory_mb, 4096);
}

#[test]
fn test_check_availability_succeeds_with_enough_memory() {
    let limits = ResourceLimits {
        min_free_memory_mb: 1,
    };
    let mut guard = ResourceGuard::new(limits);
    let result = guard.check_availability("test_tool");
    // 1 MB reserve: any running system has this.
    assert!(result.is_ok(), "check_availability failed: {result:?}");
}

#[test]
fn test_check_availability_fails_with_impossible_limits() {
    let limits = ResourceLimits {
        min_free_memory_mb: u64::MAX / 2,
    };
    let mut guard = ResourceGuard::new(limits);
    let result = guard.check_availability("any_tool");
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("CSA: low memory"),
        "Expected memory error, got: {err_msg}"
    );
}

#[test]
fn test_check_availability_simple_threshold() {
    let limits = ResourceLimits {
        min_free_memory_mb: 2,
    };
    let mut guard = ResourceGuard::new(limits);
    let result = guard.check_availability("threshold_tool");
    assert!(
        result.is_ok(),
        "2 MB reserve should pass on any system: {result:?}",
    );
}

#[test]
fn test_check_availability_reports_swap_without_requiring_it() {
    let limits = ResourceLimits {
        min_free_memory_mb: 1,
    };
    let mut guard = ResourceGuard::new(limits);

    guard.sys.refresh_memory();
    let phys = guard.sys.available_memory() / 1024 / 1024;
    let swap = guard.sys.free_swap() / 1024 / 1024;

    let result = guard.check_availability("swap_tool");
    assert!(
        result.is_ok(),
        "physical {phys} MB with swap {swap} MB should be >= 1 MB"
    );
}

#[test]
fn test_cgroup_available_memory_bytes_reads_v2_files() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("memory.max"), "1048576\n").expect("write memory.max");
    std::fs::write(dir.path().join("memory.current"), "262144\n").expect("write memory.current");

    assert_eq!(cgroup_available_memory_bytes_at(dir.path()), Some(786432));
}

#[test]
fn test_cgroup_available_memory_bytes_treats_v2_max_as_unlimited() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("memory.max"), "max\n").expect("write memory.max");
    std::fs::write(dir.path().join("memory.current"), "262144\n").expect("write memory.current");

    assert_eq!(cgroup_available_memory_bytes_at(dir.path()), None);
}

#[test]
fn test_cgroup_available_memory_bytes_reads_v1_files() {
    let dir = tempfile::tempdir().expect("tempdir");
    let memory_dir = dir.path().join("memory");
    std::fs::create_dir(&memory_dir).expect("create memory cgroup dir");
    std::fs::write(memory_dir.join("memory.limit_in_bytes"), "2097152\n")
        .expect("write memory.limit_in_bytes");
    std::fs::write(memory_dir.join("memory.usage_in_bytes"), "524288\n")
        .expect("write memory.usage_in_bytes");

    assert_eq!(cgroup_available_memory_bytes_at(dir.path()), Some(1572864));
}

#[test]
fn test_effective_available_memory_uses_lower_cgroup_available() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("memory.max"), "1048576\n").expect("write memory.max");
    std::fs::write(dir.path().join("memory.current"), "262144\n").expect("write memory.current");

    assert_eq!(
        effective_available_memory_bytes_at(4 * 1024 * 1024, dir.path()),
        786432
    );
}

#[test]
fn test_evaluate_hard_block_when_available_below_reserve() {
    let result = evaluate_memory_availability("test_tool", 3000, 1000, 4000, 32_000, 4096, None);
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("CSA: low memory"),
        "Expected hard block, got: {msg}"
    );
    assert!(
        msg.contains("actual_available_mb=3000"),
        "Should show available: {msg}"
    );
    assert!(
        msg.contains("required_mb=4096"),
        "Should show reserve: {msg}"
    );
    assert!(msg.contains("--min-free-memory-mb <MB>"));
}

#[test]
fn test_evaluate_warning_when_available_between_100_and_150_percent() {
    let result = evaluate_memory_availability("test_tool", 5000, 1000, 6000, 32_000, 4096, None);
    assert!(result.is_ok(), "Should warn but not block: {result:?}");
}

#[test]
fn test_evaluate_blocks_when_memavailable_below_reserve_even_with_swap() {
    let result = evaluate_memory_availability("test_tool", 3900, 4096, 7996, 32_000, 4096, None);
    assert!(
        result.is_err(),
        "swap must not satisfy min_free_memory_mb when MemAvailable is low"
    );
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("actual_available_mb=3900"));
    assert!(msg.contains("swap_available_mb=4096"));
    assert!(msg.contains("combined_available_mb=7996"));
}

#[test]
fn test_evaluate_no_warning_when_available_above_150_percent() {
    let result = evaluate_memory_availability("test_tool", 7000, 1000, 8000, 32_000, 4096, None);
    assert!(result.is_ok(), "Should pass without warning: {result:?}");
}

#[test]
fn test_evaluate_exact_boundary_at_reserve() {
    let result = evaluate_memory_availability("test_tool", 4096, 1096, 5192, 32_000, 4096, None);
    assert!(result.is_ok(), "Exact reserve should pass: {result:?}");
}

#[test]
fn test_evaluate_exact_boundary_at_warning_threshold() {
    let result = evaluate_memory_availability("test_tool", 6144, 1144, 7288, 32_000, 4096, None);
    assert!(
        result.is_ok(),
        "Exact warning threshold should pass: {result:?}"
    );
}

#[test]
fn test_evaluate_blocks_when_spawn_projection_exceeds_available_headroom() {
    let admission = SpawnMemoryAdmission {
        projected_spawn_mb: 8192,
        active_session_rss_mb: 2048,
        active_session_projected_mb: 4096,
        active_session_count: 1,
        sampled_session_count: 1,
    };

    let result =
        evaluate_memory_availability("codex", 10_000, 0, 10_000, 32_000, 4096, Some(admission));

    assert!(result.is_err());
    let err = result.unwrap_err();
    let admission_error = err
        .downcast_ref::<MemoryAdmissionError>()
        .expect("memory admission error");
    assert_eq!(admission_error.retry_physical_upper_mb, Some(5904));
    assert_eq!(admission_error.retry_active_session_upper_mb, Some(19_904));
    assert_eq!(admission_error.retry_combined_upper_mb, Some(5904));
    let msg = err.to_string();
    assert!(msg.contains("host memory admission denied"));
    assert!(msg.contains("projected_spawn=8192MB"));
    assert!(msg.contains("infrastructure/session-unavailable"));
    assert!(msg.contains("Host admission uses physical MemAvailable only"));
    assert!(msg.contains("swap and combined memory are reported for diagnostics"));
    assert!(msg.contains("--memory-max-mb <MB>"));
    assert!(msg.contains("--min-free-memory-mb <MB>"));
    assert!(msg.contains("Retry upper bound: memory_max_mb <= 5904MB"));
    assert!(msg.contains("tools.<tool>.memory_max_mb"));
}

#[test]
fn test_evaluate_blocks_when_active_projection_exceeds_host_safe_limit() {
    let admission = SpawnMemoryAdmission {
        projected_spawn_mb: 8192,
        active_session_rss_mb: 16_000,
        active_session_projected_mb: 20_000,
        active_session_count: 3,
        sampled_session_count: 2,
    };

    let result =
        evaluate_memory_availability("codex", 20_000, 0, 20_000, 32_000, 4096, Some(admission));

    assert!(result.is_err());
    let err = result.unwrap_err();
    let admission_error = err
        .downcast_ref::<MemoryAdmissionError>()
        .expect("memory admission error");
    assert_eq!(admission_error.retry_physical_upper_mb, Some(15_904));
    assert_eq!(admission_error.retry_active_session_upper_mb, Some(4000));
    assert_eq!(admission_error.retry_combined_upper_mb, Some(4000));
    let msg = err.to_string();
    assert!(msg.contains("active-session memory admission denied"));
    assert!(msg.contains("projected_active=28192MB"));
    assert!(msg.contains("Retry upper bound: memory_max_mb <= 4000MB"));
    assert!(msg.contains("--memory-max-mb <MB>"));
    assert!(msg.contains("resources.memory_max_mb"));
}

#[test]
fn test_evaluate_allows_safe_spawn_projection() {
    let admission = SpawnMemoryAdmission {
        projected_spawn_mb: 4096,
        active_session_rss_mb: 2048,
        active_session_projected_mb: 4096,
        active_session_count: 1,
        sampled_session_count: 1,
    };

    let result = evaluate_memory_availability(
        "claude-code",
        12_000,
        0,
        12_000,
        32_000,
        4096,
        Some(admission),
    );

    assert!(result.is_ok(), "safe projection should pass: {result:?}");
}
