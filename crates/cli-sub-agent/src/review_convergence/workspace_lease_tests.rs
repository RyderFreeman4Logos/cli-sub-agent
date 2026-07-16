use std::fs;
use std::os::unix::fs::{MetadataExt, symlink};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Result, anyhow};
use csa_session::convergence::{
    CampaignId, EpochRecord, GitObjectId, Sha256Digest, WorkspaceLeaseIdentity,
};
use ulid::Ulid;

use super::clean_room::{
    CleanRoomWorkspace, CleanupFailureLedger, DetachedWorkspaceLease,
    DetachedWorkspaceLeaseContext, LeaseAcquireFault, WorkspaceCleanup,
};
use super::workspace_lease_fs::DetachedWorkspaceLeaseStore;

#[derive(Default)]
struct Cleanup;
impl WorkspaceCleanup for Cleanup {
    fn cleanup(&mut self, _: Duration) -> Result<()> {
        Ok(())
    }
}

struct FailingCleanup;
impl WorkspaceCleanup for FailingCleanup {
    fn cleanup(&mut self, _: Duration) -> Result<()> {
        Err(anyhow!("simulated SIGTERM cleanup failure"))
    }
}

fn epoch() -> EpochRecord {
    EpochRecord::new(
        GitObjectId::parse(&"a".repeat(40)).unwrap(),
        GitObjectId::parse(&"b".repeat(40)).unwrap(),
        Sha256Digest::compute(b"diff"),
    )
}
fn workspace(root: PathBuf) -> CleanRoomWorkspace {
    CleanRoomWorkspace::for_lease_test(root, epoch())
}
fn context(root: &std::path::Path) -> DetachedWorkspaceLeaseContext {
    DetachedWorkspaceLeaseContext::new(
        CampaignId::generate(),
        1,
        DetachedWorkspaceLeaseStore::open(root).unwrap(),
    )
    .unwrap()
}
fn acquire(
    context: &DetachedWorkspaceLeaseContext,
    workspace: CleanRoomWorkspace,
) -> DetachedWorkspaceLease<Cleanup> {
    acquire_with(context, workspace, Cleanup, CleanupFailureLedger::default())
}

fn acquire_with<C: WorkspaceCleanup>(
    context: &DetachedWorkspaceLeaseContext,
    workspace: CleanRoomWorkspace,
    cleanup: C,
    failure_ledger: CleanupFailureLedger,
) -> DetachedWorkspaceLease<C> {
    DetachedWorkspaceLease::from_acquired(
        workspace.clone(),
        context.acquire(&workspace).unwrap(),
        cleanup,
        Duration::from_secs(1),
        failure_ledger,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LeaseRecoveryState {
    Continue,
    TerminalFailure,
}

fn lease_recovery_state(fault: LeaseAcquireFault) -> LeaseRecoveryState {
    match fault {
        LeaseAcquireFault::BeforeCreate => LeaseRecoveryState::Continue,
        LeaseAcquireFault::AfterCreate
        | LeaseAcquireFault::BeforeFileSync
        | LeaseAcquireFault::AfterFileSync => LeaseRecoveryState::TerminalFailure,
    }
}

#[test]
fn rejects_symlink_and_cross_filesystem_roots() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("target");
    let link = temp.path().join("link");
    fs::create_dir(&target).unwrap();
    symlink(&target, &link).unwrap();
    assert!(
        context(temp.path())
            .acquire(&workspace(link))
            .unwrap_err()
            .to_string()
            .contains("symlink")
    );
    assert_ne!(
        fs::metadata(temp.path()).unwrap().dev(),
        fs::metadata("/proc").unwrap().dev()
    );
    assert!(
        context(temp.path())
            .acquire(&workspace(PathBuf::from("/proc")))
            .unwrap_err()
            .to_string()
            .contains("filesystem")
    );
}

#[test]
fn rejects_concurrent_and_replaced_workspace_roots() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("workspace");
    fs::create_dir(&root).unwrap();
    let lease_context = context(temp.path());
    let lease = acquire(&lease_context, workspace(root.clone()));
    assert!(
        lease_context
            .acquire(&workspace(root.clone()))
            .unwrap_err()
            .to_string()
            .contains("already leased")
    );
    fs::rename(&root, temp.path().join("moved")).unwrap();
    fs::create_dir(&root).unwrap();
    assert!(
        lease
            .validate_current()
            .unwrap_err()
            .to_string()
            .contains("identity changed")
    );
}

#[test]
fn rejects_changed_nonce_before_cleanup() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("workspace");
    fs::create_dir(&root).unwrap();
    let lease = acquire(&context(temp.path()), workspace(root));
    let id = lease.identity();
    let changed = WorkspaceLeaseIdentity::new(
        id.campaign_id().clone(),
        id.epoch().clone(),
        id.generation(),
        id.workspace_root().to_path_buf(),
        id.device(),
        id.inode(),
        Ulid::new().to_string(),
    )
    .unwrap();
    fs::write(
        lease.lease_file_path(),
        serde_json::to_vec(&changed).unwrap(),
    )
    .unwrap();
    assert!(
        lease
            .validate_current()
            .unwrap_err()
            .to_string()
            .contains("nonce")
    );
}

#[test]
fn lease_create_fault_matrix_never_grants_an_unconfirmed_workspace() {
    for fault in [
        LeaseAcquireFault::BeforeCreate,
        LeaseAcquireFault::AfterCreate,
        LeaseAcquireFault::BeforeFileSync,
        LeaseAcquireFault::AfterFileSync,
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("workspace");
        fs::create_dir(&root).unwrap();
        let context = context(temp.path());
        let room = workspace(root.clone());

        assert!(context.acquire_with_fault(&room, fault).is_err());
        match lease_recovery_state(fault) {
            LeaseRecoveryState::Continue => {
                acquire(&context, room).release().unwrap();
            }
            LeaseRecoveryState::TerminalFailure => {
                assert!(
                    context.acquire(&room).is_err(),
                    "a lease create fault after file creation must not permit guessed reuse"
                );
            }
        }
    }
}

#[tokio::test]
async fn owned_lease_survives_await_and_explicit_release_allows_reacquisition() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("workspace");
    fs::create_dir(&root).unwrap();
    let context = context(temp.path());
    let lease = acquire(&context, workspace(root.clone()));
    tokio::task::yield_now().await;
    assert!(lease.validate_current().is_ok());
    assert!(context.acquire(&workspace(root.clone())).is_err());
    lease.release().unwrap();
    acquire(&context, workspace(root)).release().unwrap();
}

#[test]
fn failed_close_and_future_drop_leave_a_fenced_nonreusable_lease() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("workspace");
    fs::create_dir(&root).unwrap();
    let lease_context = context(temp.path());
    let failure_ledger = CleanupFailureLedger::default();
    let lease = acquire_with(
        &lease_context,
        workspace(root.clone()),
        FailingCleanup,
        failure_ledger.clone(),
    );
    let lease_file = lease.lease_file_path().to_path_buf();
    assert!(lease.close_and_confirm().is_err());
    assert!(lease_file.exists());
    assert!(!failure_ledger.failures().is_empty());
    assert!(lease_context.acquire(&workspace(root)).is_err());

    let drop_root = temp.path().join("dropped-workspace");
    fs::create_dir(&drop_root).unwrap();
    let drop_context = context(temp.path());
    let drop_failures = CleanupFailureLedger::default();
    let dropped = acquire_with(
        &drop_context,
        workspace(drop_root.clone()),
        FailingCleanup,
        drop_failures.clone(),
    );
    let dropped_lease_file = dropped.lease_file_path().to_path_buf();
    drop(dropped);
    assert!(dropped_lease_file.exists());
    assert!(!drop_failures.failures().is_empty());
    assert!(drop_context.acquire(&workspace(drop_root)).is_err());
}
