use std::fs;
use std::os::unix::fs::{MetadataExt, symlink};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use csa_session::convergence::{
    CampaignId, EpochRecord, GitObjectId, Sha256Digest, WorkspaceLeaseIdentity,
};
use ulid::Ulid;

use super::clean_room::{
    CleanRoomWorkspace, CleanupFailureLedger, DetachedWorkspaceLease,
    DetachedWorkspaceLeaseContext, WorkspaceCleanup,
};
use super::workspace_lease_fs::DetachedWorkspaceLeaseStore;

#[derive(Default)]
struct Cleanup;
impl WorkspaceCleanup for Cleanup {
    fn cleanup(&mut self, _: Duration) -> Result<()> {
        Ok(())
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
    DetachedWorkspaceLease::from_acquired(
        workspace.clone(),
        context.acquire(&workspace).unwrap(),
        Cleanup,
        Duration::from_secs(1),
        CleanupFailureLedger::default(),
    )
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
    let context = context(temp.path());
    let lease = acquire(&context, workspace(root.clone()));
    assert!(
        context
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
