#[cfg(unix)]
static GEMINI_SHARED_NPM_CACHE_ENV_LOCK: std::sync::LazyLock<std::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(()));

#[cfg(unix)]
struct GeminiSharedNpmCacheScopedEnvVar {
    key: &'static str,
    original: Option<String>,
}

#[cfg(unix)]
impl GeminiSharedNpmCacheScopedEnvVar {
    fn set(key: &'static str, value: &str) -> Self {
        let original = std::env::var(key).ok();
        // SAFETY: test-scoped env mutation guarded by GEMINI_SHARED_NPM_CACHE_ENV_LOCK.
        unsafe { std::env::set_var(key, value) };
        Self { key, original }
    }
}

#[cfg(unix)]
impl Drop for GeminiSharedNpmCacheScopedEnvVar {
    fn drop(&mut self) {
        // SAFETY: test-scoped env mutation guarded by GEMINI_SHARED_NPM_CACHE_ENV_LOCK.
        unsafe {
            match self.original.take() {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

#[tokio::test]
async fn test_execute_fails_fast_when_shared_npm_cache_bind_cannot_be_added() {
    let (temp, mut env, model_log_path) = setup_fake_gemini_environment(99);
    let source_home = temp.path().join("source-home");
    std::fs::create_dir_all(source_home.join(".gemini")).expect("create source gemini dir");
    env.insert(
        "HOME".to_string(),
        source_home.to_string_lossy().into_owned(),
    );
    env.insert("XDG_CACHE_HOME".to_string(), "/proc/nonexistent".to_string());

    let transport = AcpTransport::new("gemini-cli", None);
    let session = build_test_meta_session(temp.path().to_str().expect("utf8 temp path"));
    let sandbox = SandboxTransportConfig {
        isolation_plan: IsolationPlan {
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
        },
        tool_name: "gemini-cli".to_string(),
        best_effort: false,
        session_id: "01HTESTGEMININPMLOCKSTEP0000001".to_string(),
    };
    let options = TransportOptions {
        stream_mode: StreamMode::BufferOnly,
        idle_timeout_seconds: 30,
        acp_crash_max_attempts: 2,
        initial_response_timeout: super::ResolvedTimeout(None),
        liveness_dead_seconds: 30,
        stdin_write_timeout_seconds: 30,
        acp_init_timeout_seconds: 30,
        termination_grace_period_seconds: 1,
        output_spool: None,
        output_spool_max_bytes: csa_process::DEFAULT_SPOOL_MAX_BYTES,
        output_spool_keep_rotated: csa_process::DEFAULT_SPOOL_KEEP_ROTATED,
        setting_sources: None,
        sandbox: Some(&sandbox),
    };

    let error = transport
        .execute(
            "shared npm cache bind failure should fail fast",
            None,
            &session,
            Some(&env),
            options,
        )
        .await
        .expect_err("sandbox plan assembly should fail fast");

    let error_text = format!("{error:#}");
    let denied_path = "/proc/nonexistent/cli-sub-agent/npm";
    assert!(
        error_text.contains(denied_path),
        "error should name denied path, got: {error_text}"
    );
    assert!(
        error_text.contains("filesystem_sandbox") || error_text.contains("writable_paths"),
        "error should point at sandbox writable_paths config, got: {error_text}"
    );
    assert!(
        error_text.contains("XDG_CACHE_HOME"),
        "error should mention XDG_CACHE_HOME remediation, got: {error_text}"
    );

    assert!(
        !env.contains_key("npm_config_cache"),
        "caller env must not leak a partially-coupled npm_config_cache override"
    );
    assert!(
        !sandbox.isolation_plan.env_overrides.contains_key("npm_config_cache"),
        "base sandbox config must remain untouched when plan assembly aborts"
    );
    assert!(
        !sandbox
            .isolation_plan
            .writable_paths
            .contains(&PathBuf::from(denied_path)),
        "base sandbox config must not accumulate failed writable binds"
    );
    assert!(
        !model_log_path.exists(),
        "gemini should not launch after sandbox plan failure"
    );
    assert!(
        !model_log_path.with_file_name("attempts.txt").exists(),
        "sandbox plan failure should abort before the fake gemini process runs"
    );
}

#[tokio::test]
async fn test_legacy_execute_fails_fast_when_shared_npm_cache_bind_cannot_be_added() {
    let (temp, mut env, model_log_path) = setup_fake_gemini_environment(99);
    let source_home = temp.path().join("source-home");
    std::fs::create_dir_all(source_home.join(".gemini")).expect("create source gemini dir");
    env.insert(
        "HOME".to_string(),
        source_home.to_string_lossy().into_owned(),
    );
    env.insert("XDG_CACHE_HOME".to_string(), "/proc/nonexistent".to_string());

    let transport = LegacyTransport::new(Executor::GeminiCli {
        model_override: None,
        thinking_budget: None,
    });
    let session = build_test_meta_session(temp.path().to_str().expect("utf8 temp path"));
    let sandbox = SandboxTransportConfig {
        isolation_plan: IsolationPlan {
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
        },
        tool_name: "gemini-cli".to_string(),
        best_effort: false,
        session_id: "01HTESTGEMINILEGACYNPMLOCKSTEP01".to_string(),
    };
    let options = TransportOptions {
        stream_mode: StreamMode::BufferOnly,
        idle_timeout_seconds: 30,
        acp_crash_max_attempts: 2,
        initial_response_timeout: super::ResolvedTimeout(None),
        liveness_dead_seconds: 30,
        stdin_write_timeout_seconds: 30,
        acp_init_timeout_seconds: 30,
        termination_grace_period_seconds: 1,
        output_spool: None,
        output_spool_max_bytes: csa_process::DEFAULT_SPOOL_MAX_BYTES,
        output_spool_keep_rotated: csa_process::DEFAULT_SPOOL_KEEP_ROTATED,
        setting_sources: None,
        sandbox: Some(&sandbox),
    };

    let error = transport
        .execute(
            "legacy shared npm cache bind failure should fail fast",
            None,
            &session,
            Some(&env),
            options,
        )
        .await
        .expect_err("legacy sandbox plan assembly should fail fast");

    let error_text = format!("{error:#}");
    let denied_path = "/proc/nonexistent/cli-sub-agent/npm";
    assert!(
        error_text.contains(denied_path),
        "error should name denied path, got: {error_text}"
    );
    assert!(
        error_text.contains("filesystem_sandbox") || error_text.contains("writable_paths"),
        "error should point at sandbox writable_paths config, got: {error_text}"
    );
    assert!(
        error_text.contains("XDG_CACHE_HOME"),
        "error should mention XDG_CACHE_HOME remediation, got: {error_text}"
    );

    assert!(
        !env.contains_key("npm_config_cache"),
        "caller env must not leak a partially-coupled npm_config_cache override"
    );
    assert!(
        !sandbox.isolation_plan.env_overrides.contains_key("npm_config_cache"),
        "base sandbox config must remain untouched when plan assembly aborts"
    );
    assert!(
        !sandbox
            .isolation_plan
            .writable_paths
            .contains(&PathBuf::from(denied_path)),
        "base sandbox config must not accumulate failed writable binds"
    );
    assert!(
        !model_log_path.exists(),
        "gemini should not launch after sandbox plan failure"
    );
    assert!(
        !model_log_path.with_file_name("attempts.txt").exists(),
        "sandbox plan failure should abort before the fake gemini process runs"
    );
}

#[tokio::test]
async fn test_execute_fails_fast_when_shared_npm_cache_path_violates_writable_allowlist() {
    let (temp, mut env, model_log_path) = setup_fake_gemini_environment(99);
    let source_home = temp.path().join("source-home");
    std::fs::create_dir_all(source_home.join(".gemini")).expect("create source gemini dir");
    env.insert(
        "HOME".to_string(),
        source_home.to_string_lossy().into_owned(),
    );
    env.insert(
        "XDG_CACHE_HOME".to_string(),
        "/var/tmp/csa-outside-allowlist-1047".to_string(),
    );

    let transport = AcpTransport::new("gemini-cli", None);
    let session = build_test_meta_session(temp.path().to_str().expect("utf8 temp path"));
    let sandbox = SandboxTransportConfig {
        isolation_plan: IsolationPlan {
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
        },
        tool_name: "gemini-cli".to_string(),
        best_effort: false,
        session_id: "01HTESTGEMINIALLOWLISTACPNPM0001".to_string(),
    };
    let options = TransportOptions {
        stream_mode: StreamMode::BufferOnly,
        idle_timeout_seconds: 30,
        acp_crash_max_attempts: 2,
        initial_response_timeout: super::ResolvedTimeout(None),
        liveness_dead_seconds: 30,
        stdin_write_timeout_seconds: 30,
        acp_init_timeout_seconds: 30,
        termination_grace_period_seconds: 1,
        output_spool: None,
        output_spool_max_bytes: csa_process::DEFAULT_SPOOL_MAX_BYTES,
        output_spool_keep_rotated: csa_process::DEFAULT_SPOOL_KEEP_ROTATED,
        setting_sources: None,
        sandbox: Some(&sandbox),
    };

    let error = transport
        .execute(
            "shared npm cache allowlist violation should fail fast",
            None,
            &session,
            Some(&env),
            options,
        )
        .await
        .expect_err("sandbox plan assembly should reject outside-allowlist npm cache");

    let error_text = format!("{error:#}");
    let denied_path = "/var/tmp/csa-outside-allowlist-1047/cli-sub-agent/npm";
    assert!(
        error_text.contains(denied_path),
        "error should name denied path, got: {error_text}"
    );
    assert!(
        error_text.contains("XDG_CACHE_HOME"),
        "error should mention XDG_CACHE_HOME remediation, got: {error_text}"
    );
    assert!(
        error_text.contains("[filesystem_sandbox].writable_paths"),
        "error should point at writable_paths allowlist config, got: {error_text}"
    );
    assert!(
        error_text.contains("outside allowed roots"),
        "error should preserve validator policy detail, got: {error_text}"
    );

    assert!(
        !env.contains_key("npm_config_cache"),
        "caller env must not leak a partially-coupled npm_config_cache override"
    );
    assert!(
        !sandbox.isolation_plan.env_overrides.contains_key("npm_config_cache"),
        "base sandbox config must remain untouched when plan assembly aborts"
    );
    assert!(
        !sandbox
            .isolation_plan
            .writable_paths
            .contains(&PathBuf::from(denied_path)),
        "base sandbox config must not accumulate failed writable binds"
    );
    assert!(
        !model_log_path.exists(),
        "gemini should not launch after sandbox plan failure"
    );
    assert!(
        !model_log_path.with_file_name("attempts.txt").exists(),
        "sandbox plan failure should abort before the fake gemini process runs"
    );
}

#[tokio::test]
async fn test_legacy_execute_fails_fast_when_shared_npm_cache_path_violates_writable_allowlist() {
    let (temp, mut env, model_log_path) = setup_fake_gemini_environment(99);
    let source_home = temp.path().join("source-home");
    std::fs::create_dir_all(source_home.join(".gemini")).expect("create source gemini dir");
    env.insert(
        "HOME".to_string(),
        source_home.to_string_lossy().into_owned(),
    );
    env.insert(
        "XDG_CACHE_HOME".to_string(),
        "/var/tmp/csa-outside-allowlist-1047".to_string(),
    );

    let transport = LegacyTransport::new(Executor::GeminiCli {
        model_override: None,
        thinking_budget: None,
    });
    let session = build_test_meta_session(temp.path().to_str().expect("utf8 temp path"));
    let sandbox = SandboxTransportConfig {
        isolation_plan: IsolationPlan {
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
        },
        tool_name: "gemini-cli".to_string(),
        best_effort: false,
        session_id: "01HTESTGEMINIALLOWLISTLEGACY0001".to_string(),
    };
    let options = TransportOptions {
        stream_mode: StreamMode::BufferOnly,
        idle_timeout_seconds: 30,
        acp_crash_max_attempts: 2,
        initial_response_timeout: super::ResolvedTimeout(None),
        liveness_dead_seconds: 30,
        stdin_write_timeout_seconds: 30,
        acp_init_timeout_seconds: 30,
        termination_grace_period_seconds: 1,
        output_spool: None,
        output_spool_max_bytes: csa_process::DEFAULT_SPOOL_MAX_BYTES,
        output_spool_keep_rotated: csa_process::DEFAULT_SPOOL_KEEP_ROTATED,
        setting_sources: None,
        sandbox: Some(&sandbox),
    };

    let error = transport
        .execute(
            "legacy shared npm cache allowlist violation should fail fast",
            None,
            &session,
            Some(&env),
            options,
        )
        .await
        .expect_err("legacy sandbox plan assembly should reject outside-allowlist npm cache");

    let error_text = format!("{error:#}");
    let denied_path = "/var/tmp/csa-outside-allowlist-1047/cli-sub-agent/npm";
    assert!(
        error_text.contains(denied_path),
        "error should name denied path, got: {error_text}"
    );
    assert!(
        error_text.contains("XDG_CACHE_HOME"),
        "error should mention XDG_CACHE_HOME remediation, got: {error_text}"
    );
    assert!(
        error_text.contains("[filesystem_sandbox].writable_paths"),
        "error should point at writable_paths allowlist config, got: {error_text}"
    );
    assert!(
        error_text.contains("outside allowed roots"),
        "error should preserve validator policy detail, got: {error_text}"
    );

    assert!(
        !env.contains_key("npm_config_cache"),
        "caller env must not leak a partially-coupled npm_config_cache override"
    );
    assert!(
        !sandbox.isolation_plan.env_overrides.contains_key("npm_config_cache"),
        "base sandbox config must remain untouched when plan assembly aborts"
    );
    assert!(
        !sandbox
            .isolation_plan
            .writable_paths
            .contains(&PathBuf::from(denied_path)),
        "base sandbox config must not accumulate failed writable binds"
    );
    assert!(
        !model_log_path.exists(),
        "gemini should not launch after sandbox plan failure"
    );
    assert!(
        !model_log_path.with_file_name("attempts.txt").exists(),
        "sandbox plan failure should abort before the fake gemini process runs"
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn test_execute_fails_fast_when_symlinked_shared_npm_cache_resolves_outside_allowlist() {
    let (temp, mut env, model_log_path) = setup_fake_gemini_environment(99);
    let source_home = temp.path().join("source-home");
    std::fs::create_dir_all(source_home.join(".gemini")).expect("create source gemini dir");
    let _env_lock = GEMINI_SHARED_NPM_CACHE_ENV_LOCK.lock().expect("env lock");
    let _home_guard = GeminiSharedNpmCacheScopedEnvVar::set(
        "HOME",
        source_home.to_str().expect("utf8 source home path"),
    );
    env.insert(
        "HOME".to_string(),
        source_home.to_string_lossy().into_owned(),
    );
    let outside_allowlist_root = tempfile::tempdir_in(
        std::env::current_dir().expect("current dir for outside-allowlist tempdir"),
    )
    .expect("create outside-allowlist tempdir");
    let symlink_cache_root = temp.path().join("symlink-cache");
    std::os::unix::fs::symlink(outside_allowlist_root.path(), &symlink_cache_root)
        .expect("symlink cache root");
    env.insert(
        "XDG_CACHE_HOME".to_string(),
        symlink_cache_root.to_string_lossy().into_owned(),
    );

    let transport = AcpTransport::new("gemini-cli", None);
    let session = build_test_meta_session(temp.path().to_str().expect("utf8 temp path"));
    let sandbox = SandboxTransportConfig {
        isolation_plan: IsolationPlan {
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
        },
        tool_name: "gemini-cli".to_string(),
        best_effort: false,
        session_id: "01HTESTGEMINISYMLINKACPNPM0001".to_string(),
    };
    let options = TransportOptions {
        stream_mode: StreamMode::BufferOnly,
        idle_timeout_seconds: 30,
        acp_crash_max_attempts: 2,
        initial_response_timeout: super::ResolvedTimeout(None),
        liveness_dead_seconds: 30,
        stdin_write_timeout_seconds: 30,
        acp_init_timeout_seconds: 30,
        termination_grace_period_seconds: 1,
        output_spool: None,
        output_spool_max_bytes: csa_process::DEFAULT_SPOOL_MAX_BYTES,
        output_spool_keep_rotated: csa_process::DEFAULT_SPOOL_KEEP_ROTATED,
        setting_sources: None,
        sandbox: Some(&sandbox),
    };

    let error = transport
        .execute(
            "symlinked shared npm cache should fail fast",
            None,
            &session,
            Some(&env),
            options,
        )
        .await
        .expect_err("sandbox plan assembly should reject a symlinked outside-allowlist npm cache");

    let error_text = format!("{error:#}");
    let requested_path = symlink_cache_root.join("cli-sub-agent/npm");
    let canonical_path = outside_allowlist_root.path().join("cli-sub-agent/npm");
    assert!(
        error_text.contains(requested_path.to_string_lossy().as_ref()),
        "error should name the original XDG_CACHE_HOME-derived path, got: {error_text}"
    );
    assert!(
        error_text.contains(canonical_path.to_string_lossy().as_ref()),
        "error should name the canonical target path, got: {error_text}"
    );
    assert!(
        error_text.contains("[filesystem_sandbox].writable_paths"),
        "error should point at writable_paths allowlist config, got: {error_text}"
    );
    assert!(
        error_text.contains("non-symlinked location"),
        "error should explain the non-symlinked remediation, got: {error_text}"
    );

    assert!(
        !env.contains_key("npm_config_cache"),
        "caller env must not leak a partially-coupled npm_config_cache override"
    );
    assert!(
        !sandbox.isolation_plan.env_overrides.contains_key("npm_config_cache"),
        "base sandbox config must remain untouched when plan assembly aborts"
    );
    assert!(
        !sandbox
            .isolation_plan
            .writable_paths
            .contains(&canonical_path),
        "base sandbox config must not accumulate failed writable binds"
    );
    assert!(
        !requested_path.exists(),
        "symlinked denied path must not be created during failed plan assembly"
    );
    assert!(
        !canonical_path.exists(),
        "canonical target must not be created during failed plan assembly"
    );
    assert!(
        !model_log_path.exists(),
        "gemini should not launch after sandbox plan failure"
    );
    assert!(
        !model_log_path.with_file_name("attempts.txt").exists(),
        "sandbox plan failure should abort before the fake gemini process runs"
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn test_legacy_execute_fails_fast_when_symlinked_shared_npm_cache_resolves_outside_allowlist()
{
    let (temp, mut env, model_log_path) = setup_fake_gemini_environment(99);
    let source_home = temp.path().join("source-home");
    std::fs::create_dir_all(source_home.join(".gemini")).expect("create source gemini dir");
    let _env_lock = GEMINI_SHARED_NPM_CACHE_ENV_LOCK.lock().expect("env lock");
    let _home_guard = GeminiSharedNpmCacheScopedEnvVar::set(
        "HOME",
        source_home.to_str().expect("utf8 source home path"),
    );
    env.insert(
        "HOME".to_string(),
        source_home.to_string_lossy().into_owned(),
    );
    let outside_allowlist_root = tempfile::tempdir_in(
        std::env::current_dir().expect("current dir for outside-allowlist tempdir"),
    )
    .expect("create outside-allowlist tempdir");
    let symlink_cache_root = temp.path().join("symlink-cache");
    std::os::unix::fs::symlink(outside_allowlist_root.path(), &symlink_cache_root)
        .expect("symlink cache root");
    env.insert(
        "XDG_CACHE_HOME".to_string(),
        symlink_cache_root.to_string_lossy().into_owned(),
    );

    let transport = LegacyTransport::new(Executor::GeminiCli {
        model_override: None,
        thinking_budget: None,
    });
    let session = build_test_meta_session(temp.path().to_str().expect("utf8 temp path"));
    let sandbox = SandboxTransportConfig {
        isolation_plan: IsolationPlan {
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
        },
        tool_name: "gemini-cli".to_string(),
        best_effort: false,
        session_id: "01HTESTGEMINISYMLINKLEGACYNPM01".to_string(),
    };
    let options = TransportOptions {
        stream_mode: StreamMode::BufferOnly,
        idle_timeout_seconds: 30,
        acp_crash_max_attempts: 2,
        initial_response_timeout: super::ResolvedTimeout(None),
        liveness_dead_seconds: 30,
        stdin_write_timeout_seconds: 30,
        acp_init_timeout_seconds: 30,
        termination_grace_period_seconds: 1,
        output_spool: None,
        output_spool_max_bytes: csa_process::DEFAULT_SPOOL_MAX_BYTES,
        output_spool_keep_rotated: csa_process::DEFAULT_SPOOL_KEEP_ROTATED,
        setting_sources: None,
        sandbox: Some(&sandbox),
    };

    let error = transport
        .execute(
            "legacy symlinked shared npm cache should fail fast",
            None,
            &session,
            Some(&env),
            options,
        )
        .await
        .expect_err(
            "legacy sandbox plan assembly should reject a symlinked outside-allowlist npm cache",
        );

    let error_text = format!("{error:#}");
    let requested_path = symlink_cache_root.join("cli-sub-agent/npm");
    let canonical_path = outside_allowlist_root.path().join("cli-sub-agent/npm");
    assert!(
        error_text.contains(requested_path.to_string_lossy().as_ref()),
        "error should name the original XDG_CACHE_HOME-derived path, got: {error_text}"
    );
    assert!(
        error_text.contains(canonical_path.to_string_lossy().as_ref()),
        "error should name the canonical target path, got: {error_text}"
    );
    assert!(
        error_text.contains("[filesystem_sandbox].writable_paths"),
        "error should point at writable_paths allowlist config, got: {error_text}"
    );
    assert!(
        error_text.contains("non-symlinked location"),
        "error should explain the non-symlinked remediation, got: {error_text}"
    );

    assert!(
        !env.contains_key("npm_config_cache"),
        "caller env must not leak a partially-coupled npm_config_cache override"
    );
    assert!(
        !sandbox.isolation_plan.env_overrides.contains_key("npm_config_cache"),
        "base sandbox config must remain untouched when plan assembly aborts"
    );
    assert!(
        !sandbox
            .isolation_plan
            .writable_paths
            .contains(&canonical_path),
        "base sandbox config must not accumulate failed writable binds"
    );
    assert!(
        !requested_path.exists(),
        "symlinked denied path must not be created during failed plan assembly"
    );
    assert!(
        !canonical_path.exists(),
        "canonical target must not be created during failed plan assembly"
    );
    assert!(
        !model_log_path.exists(),
        "gemini should not launch after sandbox plan failure"
    );
    assert!(
        !model_log_path.with_file_name("attempts.txt").exists(),
        "sandbox plan failure should abort before the fake gemini process runs"
    );
}
