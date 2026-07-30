#[test]
fn subagent_initial_wait_hint_requires_the_parent_to_select_its_provider() {
    let _lock = crate::test_env_lock::TEST_ENV_LOCK
        .clone()
        .blocking_lock_owned();
    let state_home = tempfile::tempdir().expect("state tempdir");
    let _state_guard =
        crate::test_env_lock::ScopedEnvVarRestore::set("XDG_STATE_HOME", state_home.path());
    let project = tempfile::tempdir().expect("project tempdir");
    let (_home_guard, _config_guard) = crate::test_env_lock::isolate_user_config(project.path());
    let _ambient_provider =
        crate::test_env_lock::ScopedEnvVarRestore::set("HERMES_MODEL_PROVIDER", "custom");
    let config_path =
        csa_config::ProjectConfig::user_config_path().expect("resolve user config path");
    std::fs::create_dir_all(config_path.parent().expect("config parent"))
        .expect("create config parent");
    std::fs::write(&config_path, "[kv_cache.provider_ttls]\nxai = 17\n")
        .expect("write provider config");
    let startup_env =
        trusted_startup_env_for_daemon_parent(project.path(), "codex/XAI/gpt-5.5/xhigh", true);
    let provider =
        crate::daemon_caller_hints::explicit_wait_provider_from_launch_routing(None, &startup_env)
            .expect("trusted subagent model pin must carry a provider");
    let _spawn_options = DaemonSpawnOptions::for_run(None, None, None, None, false, &[], false)
        .with_wait_hint_provider(Some(provider));

    let command = crate::daemon_caller_hints::resolve_session_wait_command(
        "01KAS6M5XG7V4M4M6YDRS7P8R9",
        project.path(),
    );

    assert!(
        command.command().is_none(),
        "the subagent launch provider is not the caller's KV-cache provider"
    );
    assert!(
        command
            .provider_selection_hint()
            .contains("legal_keys=\"xai\""),
        "the caller must select its own configured provider"
    );
}
