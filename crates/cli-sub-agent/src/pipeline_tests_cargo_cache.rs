use super::*;

#[test]
fn build_merged_env_materializes_cargo_home_cache_subdirs_for_sandbox() {
    let _env_lock = crate::test_env_lock::TEST_ENV_LOCK.blocking_lock();
    let temp = tempfile::tempdir().expect("tempdir");
    let home = temp.path().join("home");
    let cargo_home = temp.path().join("ambient-cargo-home");
    for dir in [&home, &cargo_home] {
        std::fs::create_dir_all(dir).expect("create Cargo dir");
    }
    let _home = ScopedEnvVarRestore::set("HOME", home.to_str().expect("home utf8"));
    let _cargo_home = ScopedEnvVarRestore::set(
        csa_core::env::CARGO_HOME_ENV_KEY,
        cargo_home.to_str().expect("cargo home utf8"),
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

    let writable_paths = crate::pipeline_env::rust_session_writable_paths(&merged);
    for path in [
        &cargo_home,
        &cargo_home.join("git"),
        &cargo_home.join("registry"),
    ] {
        assert!(
            writable_paths.contains(path),
            "{} should be granted writable sandbox access, got {:?}",
            path.display(),
            writable_paths
        );
    }
}
