use super::{MergedEnvRequest, ScopedEnvVarRestore, test_config_with_node_heap_limit};

#[test]
fn build_merged_env_normalizes_readonly_cargo_home_without_home() {
    let _env_lock = crate::test_env_lock::TEST_ENV_LOCK.blocking_lock();
    let temp = tempfile::tempdir().expect("tempdir");
    let _home = ScopedEnvVarRestore::unset("HOME");
    let _cargo_home = ScopedEnvVarRestore::set(csa_core::env::CARGO_HOME_ENV_KEY, "/usr/local");
    let _rustup_home = ScopedEnvVarRestore::unset(csa_core::env::RUSTUP_HOME_ENV_KEY);
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

    let cargo_home = merged
        .get(csa_core::env::CARGO_HOME_ENV_KEY)
        .map(std::path::PathBuf::from)
        .expect("CARGO_HOME should be normalized even when HOME is absent");
    assert_ne!(
        cargo_home,
        std::path::Path::new("/usr/local"),
        "CARGO_HOME=/usr/local makes cargo write git db under /usr/local/git"
    );
    assert!(
        !csa_core::env::rust_state_path_needs_session_override(&cargo_home),
        "normalized CARGO_HOME must be writable or outside read-only /usr/local: {}",
        cargo_home.display()
    );
}
