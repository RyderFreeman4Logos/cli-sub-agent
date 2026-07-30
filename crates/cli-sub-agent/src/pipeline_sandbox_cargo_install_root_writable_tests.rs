use super::*;

#[test]
fn test_rust_env_writable_cargo_install_root_accepts_existing_directory() {
    let project_root = tempfile::tempdir().expect("project root tempdir");
    let cargo_install_root = project_root.path().join("target/cargo-install-root");
    std::fs::create_dir_all(&cargo_install_root).expect("create existing cargo install root");
    let execution_env = HashMap::from([(
        csa_core::env::CARGO_INSTALL_ROOT_ENV_KEY.to_string(),
        cargo_install_root.to_string_lossy().into_owned(),
    )]);

    let plan = add_execution_env_writable_paths(
        csa_resource::isolation_plan::IsolationPlanBuilder::new(
            csa_resource::isolation_plan::EnforcementMode::BestEffort,
        ),
        Some(&execution_env),
        project_root.path(),
    )
    .expect("existing cargo install-root directory should remain usable")
    .build()
    .expect("build isolation plan");

    assert!(
        plan.writable_paths.contains(
            &cargo_install_root
                .canonicalize()
                .expect("canonical install root")
        )
    );
}

#[test]
fn test_rust_env_writable_cargo_install_root_rejects_existing_file_clearly() {
    let project_root = tempfile::tempdir().expect("project root tempdir");
    let cargo_install_root = project_root.path().join("target/cargo-install-root");
    std::fs::create_dir_all(cargo_install_root.parent().expect("install root parent"))
        .expect("create install root parent");
    std::fs::write(&cargo_install_root, "not a directory").expect("create install root file");
    let execution_env = HashMap::from([(
        csa_core::env::CARGO_INSTALL_ROOT_ENV_KEY.to_string(),
        cargo_install_root.to_string_lossy().into_owned(),
    )]);

    let error = add_execution_env_writable_paths(
        csa_resource::isolation_plan::IsolationPlanBuilder::new(
            csa_resource::isolation_plan::EnforcementMode::BestEffort,
        ),
        Some(&execution_env),
        project_root.path(),
    )
    .expect_err("file at cargo install-root must fail closed");

    assert!(
        error.contains("exists as a file"),
        "unexpected error: {error}"
    );
    assert!(error.contains(&cargo_install_root.display().to_string()));
}

#[cfg(unix)]
#[test]
fn test_rust_env_writable_cargo_install_root_reports_broken_parent_symlink() {
    use std::os::unix::fs::symlink;

    let project_root = tempfile::tempdir().expect("project root tempdir");
    let broken_target = project_root.path().join("missing-target");
    let target = project_root.path().join("target");
    symlink(&broken_target, &target).expect("create broken target symlink");
    let cargo_install_root = target.join("cargo-install-root");
    let execution_env = HashMap::from([(
        csa_core::env::CARGO_INSTALL_ROOT_ENV_KEY.to_string(),
        cargo_install_root.to_string_lossy().into_owned(),
    )]);

    let error = add_execution_env_writable_paths(
        csa_resource::isolation_plan::IsolationPlanBuilder::new(
            csa_resource::isolation_plan::EnforcementMode::BestEffort,
        ),
        Some(&execution_env),
        project_root.path(),
    )
    .expect_err("broken target symlink must fail closed");

    assert!(
        error.contains("exists as a symlink"),
        "unexpected error: {error}"
    );
    assert!(error.contains(&target.display().to_string()));
    assert!(!error.contains("File exists (os error 17)"));
}
