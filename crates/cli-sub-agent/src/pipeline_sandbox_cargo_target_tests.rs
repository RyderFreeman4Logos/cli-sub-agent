use std::collections::HashMap;
use std::path::PathBuf;

use crate::test_env_lock::{ScopedEnvVarRestore, TEST_ENV_LOCK};

use super::*;

fn sandbox_config() -> csa_config::ProjectConfig {
    toml::from_str(
        r#"
[resources]
memory_max_mb = 2048
enforcement_mode = "best-effort"
"#,
    )
    .expect("test TOML should parse")
}

fn resolve_sandbox_options_with_execution_env(
    cfg: &csa_config::ProjectConfig,
    project_root: &std::path::Path,
    execution_env: &HashMap<String, String>,
) -> SandboxResolution {
    resolve_sandbox_options_with_overrides(
        SandboxResolveInput {
            config: Some(cfg),
            tool_name: "codex",
            session_id: "test-session",
            project_root,
            stream_mode: StreamMode::BufferOnly,
            idle_timeout_seconds: 120,
            liveness_dead_seconds: 600,
            initial_response_timeout_seconds: Some(120),
            no_fs_sandbox: false,
            allow_user_daemon_ipc: false,
            readonly_project_root: false,
            extra_writable: &[],
            extra_readable: &[],
            execution_env: Some(execution_env),
        },
        RunResourceOverrides::absent(),
    )
}

#[cfg(unix)]
#[test]
fn cargo_target_env_uses_managed_dir_for_unwritable_workspace_target() {
    use std::os::unix::fs::symlink;

    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let project_root = tempfile::tempdir().expect("tempdir");
    let _state_home = ScopedEnvVarRestore::set("XDG_STATE_HOME", project_root.path().join("state"));
    let cfg = sandbox_config();
    let proc_dir = std::path::Path::new("/proc");
    if !proc_dir.is_dir() {
        return;
    }
    symlink(proc_dir, project_root.path().join("target"))
        .expect("create unwritable target symlink");
    let mut execution_env = HashMap::new();
    let report = crate::pipeline_cargo_target::apply_run_target_dir_guard(
        Some("run"),
        "codex",
        project_root.path(),
        &mut execution_env,
    )
    .expect("policy should resolve");
    let managed_target = PathBuf::from(
        execution_env
            .get(csa_core::env::CARGO_TARGET_DIR_ENV_KEY)
            .expect("managed CARGO_TARGET_DIR"),
    );

    let opts =
        match resolve_sandbox_options_with_execution_env(&cfg, project_root.path(), &execution_env)
        {
            SandboxResolution::Ok(opts) => *opts,
            SandboxResolution::RequiredButUnavailable(msg) => {
                panic!("Expected sandbox resolution to accept managed target: {msg}")
            }
        };
    let sandbox = opts.sandbox.expect("expected sandbox context");
    let managed_target = managed_target
        .canonicalize()
        .unwrap_or_else(|_| managed_target.clone());

    assert_eq!(report.policy_reason, "managed_target_selected");
    assert!(
        sandbox
            .isolation_plan
            .writable_paths
            .contains(&managed_target),
        "managed Cargo target should be writable, got: {:?}",
        sandbox.isolation_plan.writable_paths
    );
}
