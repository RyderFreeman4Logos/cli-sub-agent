#[test]
fn build_merged_env_injects_node_options_when_heap_limit_configured() {
    let cfg = test_config_with_node_heap_limit(Some(2048));
    let merged = crate::pipeline_env::build_merged_env(MergedEnvRequest {
        extra_env: None,
        config: Some(&cfg),
        global_config: None,
        project_root: None,
        tool_name: "claude-code",
        current_depth: 0,
        pattern_internal: false,
        allow_git_push: false,
    });
    assert_eq!(
        merged.get("NODE_OPTIONS"),
        Some(&"--max-old-space-size=2048".to_string())
    );
    assert_eq!(
        merged.get("CSA_SUPPRESS_NOTIFY"),
        Some(&"1".to_string()),
        "suppress notify should remain enabled by default"
    );
}

#[test]
fn build_merged_env_does_not_inject_node_options_without_heap_limit() {
    let cfg = test_config_with_node_heap_limit(None);
    // Use a lightweight tool (opencode); heavyweight tools default node_heap_limit_mb to Some(2048).
    let merged = crate::pipeline_env::build_merged_env(MergedEnvRequest {
        extra_env: None,
        config: Some(&cfg),
        global_config: None,
        project_root: None,
        tool_name: "opencode",
        current_depth: 0,
        pattern_internal: false,
        allow_git_push: false,
    });

    assert!(
        !merged.contains_key("NODE_OPTIONS"),
        "NODE_OPTIONS should be absent when no node heap limit is configured"
    );
}

#[test]
fn build_merged_env_preserves_current_path_for_tool_runtime_resolution() {
    let cfg = test_config_with_node_heap_limit(None);
    let Some(path) = std::env::var_os("PATH") else {
        return;
    };

    let merged = crate::pipeline_env::build_merged_env(MergedEnvRequest {
        extra_env: None,
        config: Some(&cfg),
        global_config: None,
        project_root: None,
        tool_name: "opencode",
        current_depth: 0,
        pattern_internal: false,
        allow_git_push: false,
    });

    assert_eq!(
        merged.get("PATH"),
        Some(&path.to_string_lossy().into_owned())
    );
}

#[test]
fn build_merged_env_normalizes_readonly_usr_local_rust_state() {
    let _env_lock = crate::test_env_lock::TEST_ENV_LOCK.blocking_lock();
    let temp = tempfile::tempdir().expect("tempdir");
    let home = temp.path().join("home");
    let mise_data = temp.path().join("mise-data");
    let mise_rust = mise_data.join("installs/rust/stable");
    let toolchain_bin = mise_rust
        .join("toolchains")
        .join("1.96.0-x86_64-unknown-linux-gnu")
        .join("bin");
    std::fs::create_dir_all(home.join(".cargo")).expect("create cargo home");
    std::fs::create_dir_all(home.join(".config/mise")).expect("create mise config");
    std::fs::create_dir_all(mise_rust.join("toolchains")).expect("create rust toolchains");
    std::fs::create_dir_all(&toolchain_bin).expect("create toolchain bin");
    std::fs::write(toolchain_bin.join("cargo"), "").expect("write cargo binary marker");
    std::fs::write(mise_rust.join("settings.toml"), "version = \"12\"\n")
        .expect("write rustup settings");
    std::fs::write(
        temp.path().join("rust-toolchain.toml"),
        "[toolchain]\nchannel = \"1.96.0\"\n",
    )
    .expect("write rust toolchain");
    let _home = ScopedEnvVarRestore::set("HOME", home.to_str().expect("home utf8"));
    let _path = ScopedEnvVarRestore::set("PATH", "/usr/local/bin:/bin");
    let _cargo_home = ScopedEnvVarRestore::set(csa_core::env::CARGO_HOME_ENV_KEY, "/usr/local");
    let _rustup_home = ScopedEnvVarRestore::set(csa_core::env::RUSTUP_HOME_ENV_KEY, "/usr/local");
    let _cargo_install_root =
        ScopedEnvVarRestore::set(csa_core::env::CARGO_INSTALL_ROOT_ENV_KEY, "/usr/local");
    let _mise_config =
        ScopedEnvVarRestore::set(csa_core::env::MISE_CONFIG_DIR_ENV_KEY, "/usr/local");
    let _mise_data = ScopedEnvVarRestore::set(
        csa_core::env::MISE_DATA_DIR_ENV_KEY,
        mise_data.to_str().expect("mise data utf8"),
    );
    let cfg = test_config_with_node_heap_limit(None);

    let merged = crate::pipeline_env::build_merged_env(MergedEnvRequest {
        extra_env: None,
        config: Some(&cfg),
        global_config: None,
        project_root: Some(temp.path()),
        tool_name: "codex",
        current_depth: 0,
        pattern_internal: false,
        allow_git_push: false,
    });

    let normalized_cargo_home = merged
        .get(csa_core::env::CARGO_HOME_ENV_KEY)
        .map(std::path::PathBuf::from)
        .expect("CARGO_HOME should be normalized");
    assert!(
        normalized_cargo_home == std::path::Path::new("/usr/local/share/cargo")
            || normalized_cargo_home == temp.path().join(".cargo-local"),
        "CARGO_HOME should use shared cache when available or project-local fallback, got {}",
        normalized_cargo_home.display()
    );
    assert!(
        !csa_core::env::rust_state_path_needs_session_override(&normalized_cargo_home),
        "normalized CARGO_HOME must not point at read-only /usr/local"
    );
    assert_eq!(
        merged
            .get(csa_core::env::CARGO_INSTALL_ROOT_ENV_KEY)
            .map(String::as_str),
        Some(
            temp.path()
                .join("target/cargo-install-root")
                .to_str()
                .expect("cargo install root utf8")
        )
    );
    assert_eq!(
        merged
            .get(csa_core::env::RUSTUP_HOME_ENV_KEY)
            .map(String::as_str),
        Some(mise_rust.to_str().expect("mise rust utf8"))
    );
    assert_eq!(
        merged
            .get(csa_core::env::MISE_CONFIG_DIR_ENV_KEY)
            .map(String::as_str),
        Some(home.join(".config/mise").to_str().expect("mise config utf8"))
    );
    let first_path_entry = std::env::split_paths(
        merged
            .get("PATH")
            .expect("PATH should be present after env merge"),
    )
    .next();
    assert_eq!(
        first_path_entry.as_deref(),
        Some(toolchain_bin.as_path()),
        "real rust toolchain bin should precede rustup shims for bare cargo"
    );
}

#[test]
fn build_merged_env_preserves_explicit_writable_rust_state() {
    let _env_lock = crate::test_env_lock::TEST_ENV_LOCK.blocking_lock();
    let temp = tempfile::tempdir().expect("tempdir");
    let home = temp.path().join("home");
    let cargo_home = temp.path().join("explicit-cargo");
    let rustup_home = temp.path().join("explicit-rustup");
    let cargo_install_root = temp.path().join("explicit-install");
    let mise_config = temp.path().join("explicit-mise-config");
    for dir in [&home, &cargo_home, &rustup_home, &cargo_install_root, &mise_config] {
        std::fs::create_dir_all(dir).expect("create explicit env dir");
    }
    let _home = ScopedEnvVarRestore::set("HOME", home.to_str().expect("home utf8"));
    let _ambient_cargo =
        ScopedEnvVarRestore::set(csa_core::env::CARGO_HOME_ENV_KEY, "/usr/local");
    let _ambient_rustup =
        ScopedEnvVarRestore::set(csa_core::env::RUSTUP_HOME_ENV_KEY, "/usr/local");
    let cfg = test_config_with_node_heap_limit(None);
    let extra_env = HashMap::from([
        (
            csa_core::env::CARGO_HOME_ENV_KEY.to_string(),
            cargo_home.to_string_lossy().into_owned(),
        ),
        (
            csa_core::env::RUSTUP_HOME_ENV_KEY.to_string(),
            rustup_home.to_string_lossy().into_owned(),
        ),
        (
            csa_core::env::CARGO_INSTALL_ROOT_ENV_KEY.to_string(),
            cargo_install_root.to_string_lossy().into_owned(),
        ),
        (
            csa_core::env::MISE_CONFIG_DIR_ENV_KEY.to_string(),
            mise_config.to_string_lossy().into_owned(),
        ),
    ]);

    let merged = crate::pipeline_env::build_merged_env(MergedEnvRequest {
        extra_env: Some(&extra_env),
        config: Some(&cfg),
        global_config: None,
        project_root: Some(temp.path()),
        tool_name: "codex",
        current_depth: 0,
        pattern_internal: false,
        allow_git_push: false,
    });

    assert_eq!(
        merged.get(csa_core::env::CARGO_HOME_ENV_KEY),
        extra_env.get(csa_core::env::CARGO_HOME_ENV_KEY)
    );
    assert_eq!(
        merged.get(csa_core::env::RUSTUP_HOME_ENV_KEY),
        extra_env.get(csa_core::env::RUSTUP_HOME_ENV_KEY)
    );
    assert_eq!(
        merged.get(csa_core::env::CARGO_INSTALL_ROOT_ENV_KEY),
        extra_env.get(csa_core::env::CARGO_INSTALL_ROOT_ENV_KEY)
    );
    assert_eq!(
        merged.get(csa_core::env::MISE_CONFIG_DIR_ENV_KEY),
        extra_env.get(csa_core::env::MISE_CONFIG_DIR_ENV_KEY)
    );
}

#[test]
fn build_merged_env_disables_gemini_direct_launch_in_tests() {
    let cfg = test_config_with_node_heap_limit(None);

    let merged = crate::pipeline_env::build_merged_env(MergedEnvRequest {
        extra_env: None,
        config: Some(&cfg),
        global_config: None,
        project_root: None,
        tool_name: "gemini-cli",
        current_depth: 0,
        pattern_internal: false,
        allow_git_push: false,
    });

    assert_eq!(
        merged.get("CSA_TEST_DISABLE_GEMINI_DIRECT_LAUNCH"),
        Some(&"1".to_string())
    );
}

#[test]
fn build_merged_env_injects_openai_compat_http_config_without_model_env() {
    let mut cfg = test_config_with_node_heap_limit(None);
    cfg.tools.insert(
        "openai-compat".to_string(),
        csa_config::ToolConfig {
            base_url: Some("http://localhost:8317".to_string()),
            api_key: Some("test-key".to_string()),
            default_model: Some("local-model".to_string()),
            ..Default::default()
        },
    );

    let merged = crate::pipeline_env::build_merged_env(MergedEnvRequest {
        extra_env: None,
        config: Some(&cfg),
        global_config: None,
        project_root: None,
        tool_name: "openai-compat",
        current_depth: 0,
        pattern_internal: false,
        allow_git_push: false,
    });

    assert_eq!(
        merged.get("OPENAI_COMPAT_BASE_URL").map(String::as_str),
        Some("http://localhost:8317")
    );
    assert_eq!(
        merged.get("OPENAI_COMPAT_API_KEY").map(String::as_str),
        Some("test-key")
    );
    assert!(
        !merged.contains_key("OPENAI_COMPAT_MODEL"),
        "project default_model must stay on executor model_override path so explicit model specs win"
    );
}

#[test]
fn build_merged_env_appends_node_options_when_existing_value_present() {
    let cfg = test_config_with_node_heap_limit(Some(2048));
    let mut extra_env = HashMap::new();
    extra_env.insert("NODE_OPTIONS".to_string(), "--trace-warnings".to_string());

    let merged = crate::pipeline_env::build_merged_env(MergedEnvRequest {
        extra_env: Some(&extra_env),
        config: Some(&cfg),
        global_config: None,
        project_root: None,
        tool_name: "claude-code",
        current_depth: 0,
        pattern_internal: false,
        allow_git_push: false,
    });

    assert_eq!(
        merged.get("NODE_OPTIONS"),
        Some(&"--trace-warnings --max-old-space-size=2048".to_string())
    );
}

#[test]
fn build_merged_env_scrubs_stale_contract_then_sets_fresh_invocation_env() {
    let cfg = test_config_with_node_heap_limit(None);
    let extra_env = HashMap::from([
        (
            csa_core::env::CSA_SESSION_ID_ENV_KEY.to_string(),
            "stale-session".to_string(),
        ),
        (
            csa_core::env::CSA_DEPTH_ENV_KEY.to_string(),
            "99".to_string(),
        ),
        (
            csa_core::env::CSA_PROJECT_ROOT_ENV_KEY.to_string(),
            "/stale/root".to_string(),
        ),
        (
            csa_core::env::CSA_INTERNAL_INVOCATION_ENV_KEY.to_string(),
            "0".to_string(),
        ),
        (
            csa_core::env::CSA_MODEL_SPEC_ENV_KEY.to_string(),
            "codex/openai/gpt-5.5/xhigh".to_string(),
        ),
        (
            csa_core::env::CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY.to_string(),
            "1".to_string(),
        ),
        ("KEEP_ME".to_string(), "value".to_string()),
    ]);

    let merged = crate::pipeline_env::build_merged_env(MergedEnvRequest {
        extra_env: Some(&extra_env),
        config: Some(&cfg),
        global_config: None,
        project_root: None,
        tool_name: "opencode",
        current_depth: 4,
        pattern_internal: false,
        allow_git_push: false,
    });

    assert_eq!(
        merged
            .get(csa_core::env::CSA_DEPTH_ENV_KEY)
            .map(String::as_str),
        Some("5"),
        "fresh CSA_DEPTH must be current_depth + 1"
    );
    assert_eq!(
        merged
            .get(csa_core::env::CSA_INTERNAL_INVOCATION_ENV_KEY)
            .map(String::as_str),
        Some("1"),
        "fresh CSA_INTERNAL_INVOCATION must be re-applied after scrub"
    );
    assert_eq!(merged.get("KEEP_ME").map(String::as_str), Some("value"));
    for key in [
        csa_core::env::CSA_SESSION_ID_ENV_KEY,
        csa_core::env::CSA_PROJECT_ROOT_ENV_KEY,
        csa_core::env::CSA_MODEL_SPEC_ENV_KEY,
        csa_core::env::CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY,
    ] {
        assert!(
            !merged.contains_key(key),
            "stale subtree-contract key {key} must not survive build_merged_env"
        );
    }
}
