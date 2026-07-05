use super::*;

#[test]
fn issue_2404_user_daemon_ipc_disabled_by_default() {
    let plan = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_filesystem_capability(FilesystemCapability::Bwrap)
        .build()
        .expect("build should succeed in BestEffort mode");
    assert!(!plan.user_daemon_ipc);
}

#[test]
fn issue_2404_user_daemon_ipc_enabled_via_builder() {
    let plan = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_filesystem_capability(FilesystemCapability::Bwrap)
        .with_user_daemon_ipc()
        .build()
        .expect("build should succeed in BestEffort mode");
    assert!(plan.user_daemon_ipc);
}
