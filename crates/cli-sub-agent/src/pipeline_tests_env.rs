#[test]
fn build_merged_env_injects_node_options_when_heap_limit_configured() {
    let cfg = test_config_with_node_heap_limit(Some(2048));
    let merged = crate::pipeline_env::build_merged_env(MergedEnvRequest {
        extra_env: None,
        config: Some(&cfg),
        global_config: None,
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
fn build_merged_env_disables_gemini_direct_launch_in_tests() {
    let cfg = test_config_with_node_heap_limit(None);

    let merged = crate::pipeline_env::build_merged_env(MergedEnvRequest {
        extra_env: None,
        config: Some(&cfg),
        global_config: None,
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
