#[test]
fn test_apply_gemini_sandbox_runtime_env_overrides_retracts_unbound_shared_npm_cache() {
    let temp = tempdir().expect("tempdir");
    let session_dir = temp.path().join("session");
    let source_home = temp.path().join("source-home");
    std::fs::create_dir_all(source_home.join(".gemini")).expect("create source gemini dir");

    let mut env = HashMap::new();
    env.insert(
        "HOME".to_string(),
        source_home.to_string_lossy().into_owned(),
    );
    env.insert("XDG_CACHE_HOME".to_string(), "/proc/nonexistent".to_string());

    prepare_gemini_acp_runtime(
        &mut env,
        None,
        Some(session_dir.as_path()),
        "01TESTGEMININPMLOCKSTEP0000001",
        &["--acp".to_string()],
    )
    .expect("prepare runtime");

    let runtime_home =
        gemini_runtime_home_from_env(&env).expect("runtime home should be configured");
    let shared_npm_cache = PathBuf::from(
        env.get("npm_config_cache")
            .expect("prepare runtime should set npm_config_cache before sandbox coupling"),
    );
    assert_eq!(
        shared_npm_cache,
        PathBuf::from("/proc/nonexistent").join("cli-sub-agent/npm")
    );

    let mut isolation_plan = IsolationPlan {
        resource: csa_resource::sandbox::ResourceCapability::None,
        filesystem: csa_resource::filesystem_sandbox::FilesystemCapability::Bwrap,
        writable_paths: Vec::new(),
        readable_paths: Vec::new(),
        env_overrides: HashMap::new(),
        degraded_reasons: Vec::new(),
        memory_max_mb: None,
        memory_swap_max_mb: None,
        pids_max: None,
        readonly_project_root: false,
        project_root: None,
        soft_limit_percent: None,
        memory_monitor_interval_seconds: None,
    };

    assert!(ensure_gemini_runtime_home_writable_path(
        &mut isolation_plan,
        Some(runtime_home.as_path())
    ));
    let shared_npm_cache_bound = ensure_gemini_runtime_home_writable_path(
        &mut isolation_plan,
        Some(shared_npm_cache.as_path()),
    );
    if !shared_npm_cache_bound {
        env.remove("npm_config_cache");
    }
    let env_overrides = gemini_sandbox_runtime_env_overrides(&env);
    apply_gemini_sandbox_runtime_env_overrides(&mut isolation_plan, &env_overrides);

    assert!(
        !shared_npm_cache_bound,
        "sensitive /proc path must not be accepted as a writable bind source"
    );
    assert!(
        !isolation_plan.writable_paths.contains(&shared_npm_cache),
        "sandbox must skip the shared npm cache bind when the host path cannot be prepared"
    );
    assert!(
        !env.contains_key("npm_config_cache"),
        "spawn env must retract npm_config_cache when the writable bind cannot be added"
    );
    assert!(
        !isolation_plan.env_overrides.contains_key("npm_config_cache"),
        "sandbox env overrides must stay in lockstep with writable binds (#1047)"
    );
}
