use super::{RunMemorySoftLimitPreflight, validate_run_memory_soft_limit_before_session};
use crate::run_resource_overrides::RunResourceOverrides;
use crate::test_env_lock::ScopedEnvVarRestore;
use crate::test_session_sandbox::ScopedSessionSandbox;
use csa_config::GlobalConfig;

#[test]
fn writer_soft_limit_floor_is_rejected_before_session_creation() {
    let project_dir = tempfile::tempdir().expect("temp project");
    let _sandbox = ScopedSessionSandbox::new_blocking(&project_dir);
    let _tools_available =
        ScopedEnvVarRestore::set(crate::run_helpers::TEST_ASSUME_TOOLS_AVAILABLE_ENV, "1");
    let _resource_capability =
        ScopedEnvVarRestore::set(csa_resource::sandbox::TEST_ASSUME_CGROUP_V2_ENV, "1");
    let mut config = crate::review_cmd::tests::project_config_with_enabled_tools(&["codex"]);
    config.resources.memory_max_mb = Some(9_103);
    config.resources.min_free_memory_mb = 1;
    config.resources.soft_limit_percent = Some(90);
    let global_config = GlobalConfig::default();

    let error = validate_run_memory_soft_limit_before_session(RunMemorySoftLimitPreflight {
        project_root: project_dir.path(),
        project_config: Some(&config),
        global_config: &global_config,
        tool_name: "codex",
        resource_overrides: RunResourceOverrides::from_cli(Some(9_103), None),
        stream_mode: csa_process::StreamMode::BufferOnly,
        idle_timeout_seconds: 120,
        initial_response_timeout_seconds: Some(120),
        build_jobs: Some(1),
        no_fs_sandbox: false,
        allow_user_daemon_ipc: true,
        extra_writable: &[],
        extra_readable: &[],
    })
    .expect_err("writer cap below its role-specific floor must fail before session creation");

    let message = format!("{error:#}");
    assert!(message.contains("memory_soft_limit_admission"), "{message}");
    assert!(message.contains("codex writer soft memory threshold is 8192MB"));
    assert!(message.contains("below required=9000MB"));
    assert!(!message.contains("Invalid session ID"), "{message}");
    assert!(message.contains("writer soft-limit memory retry guidance before session creation"));
    assert!(message.contains("run preflight for writer tool 'codex'"));
    let preflight_dir =
        csa_session::manager::get_session_dir(project_dir.path(), "run-pre-session-preflight")
            .expect("resolve preflight session path");
    assert!(
        !preflight_dir.exists(),
        "sandbox preflight must not create a synthetic session entry before strict inventory"
    );
    let sessions = csa_session::list_sessions(project_dir.path(), None).expect("list sessions");
    assert!(
        sessions.is_empty(),
        "memory floor preflight must not create a writer session"
    );
}
