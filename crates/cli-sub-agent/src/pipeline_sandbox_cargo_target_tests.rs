use std::collections::HashMap;

use crate::test_env_lock::{ScopedEnvVarRestore, TEST_ENV_LOCK};

#[cfg(unix)]
#[test]
fn cargo_target_preflight_fails_before_sandbox_resolves_an_alternate_target() {
    use std::os::unix::fs::symlink;

    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let project_root = tempfile::tempdir().expect("tempdir");
    let _state_home = ScopedEnvVarRestore::set("XDG_STATE_HOME", project_root.path().join("state"));
    let proc_dir = std::path::Path::new("/proc");
    if !proc_dir.is_dir() {
        return;
    }
    symlink(proc_dir, project_root.path().join("target"))
        .expect("create unwritable target symlink");
    let mut execution_env = HashMap::new();

    let error = crate::pipeline_cargo_target::apply_run_target_dir_guard(
        Some("run"),
        "codex",
        project_root.path(),
        &mut execution_env,
    )
    .expect_err("unwritable target must fail before sandbox resolution");

    assert!(error.contains("Cargo target preflight blocked before provider execution"));
    assert!(
        !execution_env.contains_key(csa_core::env::CARGO_TARGET_DIR_ENV_KEY),
        "preflight must not inject a sandbox-specific alternate target"
    );
}

#[cfg(unix)]
#[test]
fn cargo_target_preflight_rejects_host_writable_external_symlink_missing_from_bwrap_plan() {
    use std::os::unix::fs::symlink;

    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let project_root = tempfile::tempdir().expect("tempdir");
    let external_root = tempfile::tempdir().expect("external tempdir");
    let external_target = external_root.path().join("external-target");
    std::fs::create_dir(&external_target).expect("create host-writable external target");
    symlink(&external_target, project_root.path().join("target"))
        .expect("create external target symlink");
    let mut execution_env = HashMap::new();
    let report = crate::pipeline_cargo_target::apply_run_target_dir_guard(
        Some("run"),
        "codex",
        project_root.path(),
        &mut execution_env,
    )
    .expect("host-writable target should pass the host probe");
    let plan = csa_resource::isolation_plan::IsolationPlanBuilder::new(
        csa_resource::isolation_plan::EnforcementMode::BestEffort,
    )
    .with_filesystem_capability(csa_resource::FilesystemCapability::Bwrap)
    .with_writable_path(project_root.path().to_path_buf())
    .build()
    .expect("build bwrap plan");

    let error = crate::pipeline_cargo_target::ensure_cargo_target_sandbox_writable(
        &report,
        project_root.path(),
        Some(&plan),
    )
    .expect_err("external target must be admitted by the final sandbox plan");

    assert!(error.contains("Cargo target preflight blocked before provider execution"));
    assert!(error.contains(&format!("resolves to '{}'", external_target.display())));
    assert!(error.contains("filesystem_sandbox.extra_writable"));
}

#[cfg(unix)]
#[test]
fn cargo_target_preflight_accepts_external_symlink_granted_by_bwrap_plan() {
    use std::os::unix::fs::symlink;

    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let project_root = tempfile::tempdir().expect("tempdir");
    let external_root = tempfile::tempdir().expect("external tempdir");
    let external_target = external_root.path().join("external-target");
    std::fs::create_dir(&external_target).expect("create host-writable external target");
    symlink(&external_target, project_root.path().join("target"))
        .expect("create external target symlink");
    let mut execution_env = HashMap::new();
    let report = crate::pipeline_cargo_target::apply_run_target_dir_guard(
        Some("run"),
        "codex",
        project_root.path(),
        &mut execution_env,
    )
    .expect("host-writable target should pass the host probe");
    let plan = csa_resource::isolation_plan::IsolationPlanBuilder::new(
        csa_resource::isolation_plan::EnforcementMode::BestEffort,
    )
    .with_filesystem_capability(csa_resource::FilesystemCapability::Bwrap)
    .with_writable_path(project_root.path().to_path_buf())
    .with_writable_path(external_target.clone())
    .build()
    .expect("build bwrap plan");

    crate::pipeline_cargo_target::ensure_cargo_target_sandbox_writable(
        &report,
        project_root.path(),
        Some(&plan),
    )
    .expect("explicitly granted external target should be admitted");
}

#[cfg(unix)]
#[test]
fn readonly_bwrap_project_root_is_not_a_writable_cargo_target_grant() {
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let project_root = tempfile::tempdir().expect("project tempdir");
    let target = project_root.path().join("target");
    std::fs::create_dir(&target).expect("create workspace target");
    let mut execution_env = HashMap::new();
    let report = crate::pipeline_cargo_target::apply_run_target_dir_guard(
        Some("run"),
        "codex",
        project_root.path(),
        &mut execution_env,
    )
    .expect("host-writable workspace target should pass first preflight");

    let readonly_root_plan = csa_resource::isolation_plan::IsolationPlanBuilder::new(
        csa_resource::isolation_plan::EnforcementMode::BestEffort,
    )
    .with_filesystem_capability(csa_resource::FilesystemCapability::Bwrap)
    .with_writable_path(project_root.path().to_path_buf())
    .with_readonly_project_root(true)
    .build()
    .expect("build readonly bwrap plan");
    let error = crate::pipeline_cargo_target::ensure_cargo_target_sandbox_writable(
        &report,
        project_root.path(),
        Some(&readonly_root_plan),
    )
    .expect_err("readonly project root must not grant its Cargo target");
    assert!(error.contains("workspace_target_not_granted_by_sandbox"));

    let explicitly_granted_target_plan = csa_resource::isolation_plan::IsolationPlanBuilder::new(
        csa_resource::isolation_plan::EnforcementMode::BestEffort,
    )
    .with_filesystem_capability(csa_resource::FilesystemCapability::Bwrap)
    .with_writable_path(project_root.path().to_path_buf())
    .with_writable_path(target)
    .with_readonly_project_root(true)
    .build()
    .expect("build explicit target bwrap plan");
    crate::pipeline_cargo_target::ensure_cargo_target_sandbox_writable(
        &report,
        project_root.path(),
        Some(&explicitly_granted_target_plan),
    )
    .expect("specific writable target grant must remain valid under readonly root");
}
