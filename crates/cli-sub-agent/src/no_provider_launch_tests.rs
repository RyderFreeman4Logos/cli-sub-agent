use super::*;

#[test]
fn host_memory_reviewer_guidance_suggests_soft_limit_safe_retry_pair() {
    let memory = NoProviderLaunchMemoryDiagnostic {
        effective_memory_max_mb: Some(10_000),
        soft_limit_percent: Some(90),
        soft_threshold_mb: Some(9_000),
        required_floor_mb: Some(8_192),
        required_memory_max_mb: Some(9_103),
        reserve_mb: Some(9_000),
        available_memory_mb: Some(17_033),
        required_available_mb: Some(19_000),
        projected_spawn_mb: Some(10_000),
        retry_physical_upper_mb: Some(8_033),
        retry_active_session_upper_mb: Some(16_000),
        retry_combined_upper_mb: Some(8_033),
        retry_lower_bound_mb: Some(9_103),
        retry_feasible: Some(true),
        ..Default::default()
    };

    let original_argv = vec![
        "csa".to_string(),
        "review".to_string(),
        "--memory-max-mb".to_string(),
        "10000".to_string(),
        "--diff".to_string(),
    ];
    let guidance = host_memory_guidance_with_argv(
        Some("reviewer_sub_session"),
        "codex",
        &memory,
        &original_argv,
    );
    let joined = guidance.join("\n");

    assert!(joined.contains("physical MemAvailable only"));
    assert!(joined.contains("swap/combined memory is diagnostic"));
    assert!(joined.contains("Retry feasibility: feasible with reserve delta"));
    assert!(joined.contains(
        "Suggested retry command: csa review --memory-max-mb 9103 --diff --min-free-memory-mb 6000"
    ));
    assert!(joined.contains("lower_bound=9103MB > current_upper=8033MB"));
    assert!(joined.contains("lowering reserve opens retry_window=9103..=11033MB"));
    assert!(joined.contains("host_required=15103MB <= physical_available=17033MB"));
    assert!(joined.contains("csa plan run"));
}

#[test]
fn host_memory_reviewer_guidance_preserves_tight_retry_window() {
    let memory = NoProviderLaunchMemoryDiagnostic {
        effective_memory_max_mb: Some(10_000),
        soft_limit_percent: Some(90),
        soft_threshold_mb: Some(9_000),
        required_floor_mb: Some(8_192),
        required_memory_max_mb: Some(9_103),
        reserve_mb: Some(256),
        available_memory_mb: Some(9_296),
        required_available_mb: Some(10_256),
        projected_spawn_mb: Some(10_000),
        retry_physical_upper_mb: Some(9_040),
        retry_active_session_upper_mb: Some(12_000),
        retry_combined_upper_mb: Some(9_040),
        retry_lower_bound_mb: Some(9_103),
        retry_feasible: Some(true),
        ..Default::default()
    };

    let guidance = host_memory_guidance(Some("reviewer_sub_session"), "codex", &memory);
    let joined = guidance.join("\n");

    assert!(joined.contains("--memory-max-mb 9103 --min-free-memory-mb 193"));
    assert!(joined.contains("lower_bound=9103MB > current_upper=9040MB"));
    assert!(joined.contains("lowering reserve opens retry_window=9103..=9103MB"));
    assert!(joined.contains("host_required=9296MB <= physical_available=9296MB"));
}

#[test]
fn soft_limit_writer_guidance_reports_feasible_unified_interval_and_retry_command() {
    let memory = NoProviderLaunchMemoryDiagnostic {
        effective_memory_max_mb: Some(9_103),
        soft_limit_percent: Some(90),
        soft_threshold_mb: Some(8_192),
        required_floor_mb: Some(9_000),
        required_memory_max_mb: Some(10_000),
        reserve_mb: Some(1_024),
        available_memory_mb: Some(11_191),
        projected_spawn_mb: Some(9_103),
        retry_physical_upper_mb: Some(10_167),
        retry_active_session_upper_mb: Some(20_000),
        retry_combined_upper_mb: Some(10_167),
        retry_lower_bound_mb: Some(10_000),
        retry_feasible: Some(true),
        ..Default::default()
    };

    let original_argv = vec![
        "csa".to_string(),
        "run".to_string(),
        "--memory-max-mb".to_string(),
        "9103".to_string(),
        "fix the retry guidance".to_string(),
    ];
    let guidance = soft_limit_admission_guidance_with_argv("codex", &memory, None, &original_argv);
    let joined = guidance.join("\n");

    assert!(joined.contains("soft-limit admission rejected before provider launch"));
    assert!(joined.contains("Retry feasibility: feasible now"));
    assert!(joined.contains(
        "lower_bound=10000MB (role/tool soft-limit floor); current_upper=10167MB \
         (physical/reserve upper=10167MB, active-session upper=20000MB)"
    ));
    assert!(joined.contains(
        "Suggested retry command: csa run --memory-max-mb 10000 'fix the retry guidance'"
    ));
}

#[test]
fn soft_limit_writer_guidance_reports_one_infeasible_interval_without_raise_hint() {
    let memory = NoProviderLaunchMemoryDiagnostic {
        effective_memory_max_mb: Some(8_000),
        soft_limit_percent: Some(90),
        soft_threshold_mb: Some(7_200),
        required_floor_mb: Some(9_000),
        required_memory_max_mb: Some(10_000),
        reserve_mb: Some(1_000),
        available_memory_mb: Some(9_277),
        projected_spawn_mb: Some(10_000),
        retry_physical_upper_mb: Some(8_277),
        retry_active_session_upper_mb: Some(20_000),
        retry_combined_upper_mb: Some(8_277),
        retry_lower_bound_mb: Some(10_000),
        retry_feasible: Some(false),
        ..Default::default()
    };
    let guidance = soft_limit_admission_initial_guidance(
        vec!["Raise --memory-max-mb to at least 10000MB".to_string()],
        &memory,
    );
    let joined = guidance.join("\n");

    assert!(
        joined.contains("No floor-compliant retry envelope exists"),
        "{joined}"
    );
    assert!(!joined.contains("Raise --memory-max-mb"), "{joined}");

    let argv = vec![
        "csa".to_string(),
        "run".to_string(),
        "--memory-max-mb".to_string(),
        "8000".to_string(),
        "fix the retry interval".to_string(),
    ];
    let retry_guidance = soft_limit_admission_guidance_with_argv("codex", &memory, None, &argv);
    let retry_joined = retry_guidance.join("\n");

    assert!(
        retry_joined.contains("Retry feasibility: infeasible"),
        "{retry_joined}"
    );
    assert!(
        retry_joined
            .contains("lower_bound=10000MB (role/tool soft-limit floor); current_upper=8277MB")
    );
    assert!(
        !retry_joined.contains("Suggested retry command:"),
        "{retry_joined}"
    );
}

#[test]
fn soft_limit_diagnostic_reports_live_retry_interval_without_provider_side_effects() {
    use csa_resource::{FilesystemCapability, IsolationPlan, ResourceCapability};

    let project_dir = tempfile::tempdir().expect("temporary project");
    let _sandbox = crate::test_session_sandbox::ScopedSessionSandbox::new_blocking(&project_dir);
    let session = csa_session::create_session(
        project_dir.path(),
        Some("soft-limit feasibility diagnostic"),
        None,
        Some("codex"),
    )
    .expect("create session");
    let plan = IsolationPlan {
        resource: ResourceCapability::CgroupV2,
        filesystem: FilesystemCapability::Bwrap,
        writable_paths: Vec::new(),
        readable_paths: Vec::new(),
        env_overrides: std::collections::HashMap::new(),
        degraded_reasons: Vec::new(),
        memory_max_mb: Some(9_103),
        memory_swap_max_mb: None,
        pids_max: None,
        readonly_project_root: true,
        project_root: None,
        soft_limit_percent: Some(90),
        memory_monitor_interval_seconds: None,
        user_daemon_ipc: false,
    };
    let error = anyhow::Error::new(
        crate::resource_admission_soft_limit::ensure_memory_soft_limit_admission(
            Some("run"),
            "codex",
            Some(&plan),
        )
        .expect_err("writer soft limit should be denied"),
    );

    let diagnostic = diagnostic_from_error(
        NoProviderLaunchContext {
            session: &session,
            tool_name: "codex",
            task_type: Some("run"),
            config: None,
            resource_overrides: RunResourceOverrides::absent(),
        },
        &error,
    )
    .expect("soft-limit admission should retain a no-provider diagnostic");
    let memory = &diagnostic.memory;
    let available_mb = memory
        .available_memory_mb
        .expect("live physical availability is captured");
    let reserve_mb = memory.reserve_mb.expect("configured reserve is captured");
    let physical_upper_mb = memory
        .retry_physical_upper_mb
        .expect("physical/reserve upper bound is captured");
    let combined_upper_mb = memory
        .retry_combined_upper_mb
        .expect("unified retry upper bound is captured");

    assert!(diagnostic.no_provider_launch);
    assert!(!diagnostic.provider_side_effects);
    assert_eq!(
        diagnostic.denial_class,
        crate::resource_admission_soft_limit::MEMORY_SOFT_LIMIT_ADMISSION_REASON
    );
    assert_eq!(memory.retry_lower_bound_mb, Some(10_000));
    assert_eq!(physical_upper_mb, available_mb.saturating_sub(reserve_mb));
    assert_eq!(
        combined_upper_mb,
        physical_upper_mb.min(memory.retry_active_session_upper_mb.unwrap_or(u64::MAX))
    );
    let guidance = diagnostic.guidance.join("\n");
    assert!(guidance.contains("soft-limit admission rejected before provider launch"));
    assert!(guidance.contains("lower_bound=10000MB (role/tool soft-limit floor)"));
    assert!(guidance.contains("physical/reserve upper="));
    assert!(guidance.contains("Retry feasibility:"));
}

#[test]
fn soft_limit_guidance_reports_unavailable_strict_inventory_without_zero_memory_claims() {
    use csa_resource::{FilesystemCapability, IsolationPlan, ResourceCapability};

    let project_dir = tempfile::tempdir().expect("temporary project");
    let _sandbox = crate::test_session_sandbox::ScopedSessionSandbox::new_blocking(&project_dir);
    let project_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let session = MetaSessionState {
        meta_session_id: csa_session::new_session_id(),
        project_path: project_root.display().to_string(),
        ..Default::default()
    };
    csa_session::validate_session_id(&session.meta_session_id).expect("valid session ULID");
    let session_dir = csa_session::get_session_root(project_root)
        .expect("session root")
        .join("sessions")
        .join(&session.meta_session_id);
    std::fs::create_dir_all(&session_dir).expect("session dir");
    let state_path = session_dir.join("state.toml");
    let complete_state = toml::to_string_pretty(&session).expect("serialize complete state");
    std::fs::write(&state_path, complete_state).expect("write complete session state");
    let corrupt_state = b"not valid toml = [";
    std::fs::write(&state_path, corrupt_state).expect("corrupt complete session state");

    let plan = IsolationPlan {
        resource: ResourceCapability::CgroupV2,
        filesystem: FilesystemCapability::Bwrap,
        writable_paths: Vec::new(),
        readable_paths: Vec::new(),
        env_overrides: std::collections::HashMap::new(),
        degraded_reasons: Vec::new(),
        memory_max_mb: Some(9_103),
        memory_swap_max_mb: None,
        pids_max: None,
        readonly_project_root: true,
        project_root: None,
        soft_limit_percent: Some(90),
        memory_monitor_interval_seconds: None,
        user_daemon_ipc: false,
    };
    let error = anyhow::Error::new(
        crate::resource_admission_soft_limit::ensure_memory_soft_limit_admission(
            Some("run"),
            "codex",
            Some(&plan),
        )
        .expect_err("writer soft limit should be denied"),
    );
    let argv = vec!["csa".to_string(), "run".to_string()];
    let preflight_guidance = soft_limit_admission_guidance_from_error_with_argv(
        project_root,
        None,
        "codex",
        None,
        RunResourceOverrides::absent(),
        &error,
        &argv,
    )
    .expect("soft-limit preflight guidance");
    let diagnostic = diagnostic_from_error(
        NoProviderLaunchContext {
            session: &session,
            tool_name: "codex",
            task_type: Some("run"),
            config: None,
            resource_overrides: RunResourceOverrides::absent(),
        },
        &error,
    )
    .expect("soft-limit no-provider diagnostic");

    assert!(diagnostic.no_provider_launch);
    assert!(!diagnostic.provider_side_effects);
    assert_eq!(diagnostic.memory.available_memory_mb, None);
    assert_eq!(diagnostic.memory.retry_combined_upper_mb, None);
    assert_eq!(diagnostic.memory.retry_feasible, None);
    for guidance in [preflight_guidance, diagnostic.guidance] {
        let joined = guidance.join("\n");
        assert!(joined.contains("active-session inventory is unavailable"));
        assert!(joined.contains("Failed to list active CSA sessions for host-memory admission"));
        assert!(joined.contains("Failed to parse state file"));
        assert!(!joined.contains("physical MemAvailable 0MB"));
        assert!(!joined.contains("current_upper=0MB"));
        assert!(!joined.contains("Suggested retry command"));
        assert!(!joined.contains("Do not retry with another memory_max_mb"));
    }
    assert_eq!(
        std::fs::read(&state_path).expect("read corrupt state"),
        corrupt_state
    );
    assert!(!state_path.with_file_name("state.toml.corrupt").exists());
}

#[test]
fn host_memory_reviewer_guidance_reports_active_pressure_infeasible() {
    let memory = NoProviderLaunchMemoryDiagnostic {
        effective_memory_max_mb: Some(10_000),
        soft_limit_percent: Some(90),
        soft_threshold_mb: Some(9_000),
        required_floor_mb: Some(8_192),
        required_memory_max_mb: Some(9_103),
        reserve_mb: Some(1_000),
        available_memory_mb: Some(20_000),
        required_available_mb: Some(11_000),
        projected_spawn_mb: Some(10_000),
        retry_physical_upper_mb: Some(19_000),
        retry_active_session_upper_mb: Some(8_000),
        retry_combined_upper_mb: Some(8_000),
        retry_lower_bound_mb: Some(9_103),
        retry_feasible: Some(false),
        ..Default::default()
    };

    let guidance = host_memory_guidance(Some("reviewer_sub_session"), "codex", &memory);
    let joined = guidance.join("\n");

    assert!(joined.contains("Retry feasibility: infeasible"));
    assert!(joined.contains("active-session upper 8000MB is below lower_bound=9103MB"));
    assert!(joined.contains("Do not retry with another memory_max_mb"));
}
