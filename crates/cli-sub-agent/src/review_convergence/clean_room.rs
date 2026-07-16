//! Clean-room workspace and provider-session ports.
//!
//! The production adapters in this module are intentionally driver-injected: this slice can
//! construct and validate detached exact-OID process specifications without invoking `git` or an
//! AI provider. The repository's Rust 1.88 MSRV cannot adopt ADK-Rust 1.0 (Rust 1.94 MSRV), so the
//! provider boundary continues to use CSA's existing catalog-admitted executor.

#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "B5 Slice 3B1 exposes audited clean-room provider authority before orchestration dispatch"
    )
)]

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::Write;
use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use csa_session::convergence::{
    CampaignId, CleanupConfirmation, EpochRecord, WorkspaceLeaseIdentity,
};
use ulid::Ulid;

pub(super) use super::clean_room_provider::admitted_identity;
pub(crate) use super::clean_room_provider::{
    ProviderSessionFactory, ProviderSessionFuture, ProviderSessionOutcome, ProviderSessionRequest,
};
pub(crate) use super::workspace_lease_fs::DetachedWorkspaceLeaseStore;
use super::workspace_lease_fs::{direct_directory_metadata, lease_file_name, read_lease_identity};

#[allow(unused_imports)]
pub(crate) use super::production_clean_room_provider::ProductionCleanRoomProvider;

/// Exact, inspectable subprocess specification. Constructing this value has no side effects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommandSpec {
    program: String,
    args: Vec<String>,
    env: BTreeMap<String, String>,
}

impl CommandSpec {
    fn new(program: &str, args: Vec<String>, env: BTreeMap<String, String>) -> Result<Self> {
        validate_process_component("program", program)?;
        for argument in &args {
            validate_process_component("argument", argument)?;
        }
        for (key, value) in &env {
            validate_process_component("environment key", key)?;
            validate_process_component("environment value", value)?;
        }
        Ok(Self {
            program: program.to_string(),
            args,
            env,
        })
    }

    pub(crate) fn program(&self) -> &str {
        &self.program
    }

    pub(crate) fn args(&self) -> &[String] {
        &self.args
    }

    pub(crate) fn env(&self) -> &BTreeMap<String, String> {
        &self.env
    }
}

/// Side-effect-free create and cleanup specifications for one detached exact-OID workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DetachedWorkspacePlan {
    create: CommandSpec,
    cleanup: CommandSpec,
}

impl DetachedWorkspacePlan {
    fn exact_oid(source_repo: &Path, root: &Path, head_oid: &str) -> Result<Self> {
        let source = absolute_utf8_path("source repository", source_repo)?;
        let root = absolute_utf8_path("clean-room root", root)?;
        validate_process_component("frozen head object ID", head_oid)?;
        let env = BTreeMap::from([
            ("GIT_CONFIG_NOSYSTEM".to_string(), "1".to_string()),
            ("GIT_TERMINAL_PROMPT".to_string(), "0".to_string()),
        ]);
        Ok(Self {
            create: CommandSpec::new(
                "git",
                [
                    "-c",
                    "advice.detachedHead=false",
                    "-C",
                    source,
                    "worktree",
                    "add",
                    "--detach",
                    root,
                    head_oid,
                ]
                .into_iter()
                .map(str::to_string)
                .collect(),
                env.clone(),
            )?,
            cleanup: CommandSpec::new(
                "git",
                ["-C", source, "worktree", "remove", "--force", root]
                    .into_iter()
                    .map(str::to_string)
                    .collect(),
                env,
            )?,
        })
    }

    pub(crate) fn create(&self) -> &CommandSpec {
        &self.create
    }

    pub(crate) fn cleanup(&self) -> &CommandSpec {
        &self.cleanup
    }
}

/// Cleanup capability returned only after a workspace driver materializes a workspace.
pub(crate) trait WorkspaceCleanup {
    fn cleanup(&mut self, timeout: Duration) -> Result<()>;
}

/// Materialization receipt from an injected driver.
pub(crate) struct MaterializedWorkspace<C> {
    observed_head: String,
    cleanup: C,
}

impl<C> MaterializedWorkspace<C> {
    pub(crate) fn new(observed_head: String, cleanup: C) -> Self {
        Self {
            observed_head,
            cleanup,
        }
    }
}

/// Injected side-effect boundary. Tests use recording drivers; no shell is spawned by this module.
pub(crate) trait DetachedWorkspaceDriver {
    type Cleanup: WorkspaceCleanup;

    fn materialize(
        &mut self,
        plan: &DetachedWorkspacePlan,
    ) -> Result<MaterializedWorkspace<Self::Cleanup>>;
}

/// Immutable identity and boundaries of one clean-room checkout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CleanRoomWorkspace {
    root: PathBuf,
    bundle_path: PathBuf,
    epoch: EpochRecord,
}

/// Campaign-scoped inputs required to acquire one owned workspace lease.
#[derive(Debug, Clone)]
pub(crate) struct DetachedWorkspaceLeaseContext {
    campaign_id: CampaignId,
    generation: u64,
    store: DetachedWorkspaceLeaseStore,
}

impl DetachedWorkspaceLeaseContext {
    /// Bind an existing lease store to one nonzero completion generation.
    pub(crate) fn new(
        campaign_id: CampaignId,
        generation: u64,
        store: DetachedWorkspaceLeaseStore,
    ) -> Result<Self> {
        if generation == 0 {
            bail!("workspace lease generation must be nonzero");
        }
        Ok(Self {
            campaign_id,
            generation,
            store,
        })
    }

    pub(super) fn acquire(&self, workspace: &CleanRoomWorkspace) -> Result<AcquiredWorkspaceLease> {
        let store_metadata = self.store.validate_current()?;
        let (root, workspace_metadata) =
            direct_directory_metadata("detached workspace root", workspace.root())?;
        if workspace_metadata.dev() != store_metadata.dev() {
            bail!(
                "detached workspace root {} is not on the workspace lease store filesystem",
                root.display()
            );
        }
        let identity = WorkspaceLeaseIdentity::new(
            self.campaign_id.clone(),
            workspace.epoch().clone(),
            self.generation,
            root,
            workspace_metadata.dev(),
            workspace_metadata.ino(),
            Ulid::new().to_string(),
        )?;
        let file_path = self.store.root().join(lease_file_name(&identity));
        let serialized =
            serde_json::to_vec(&identity).context("serialize detached workspace lease identity")?;
        let mut file = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&file_path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                bail!(
                    "detached workspace root {} is already leased",
                    identity.workspace_root().display()
                );
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "atomically acquire detached workspace lease {}",
                        file_path.display()
                    )
                });
            }
        };
        file.write_all(&serialized)
            .context("write detached workspace lease identity")?;
        file.sync_all()
            .context("sync detached workspace lease identity")?;
        Ok(AcquiredWorkspaceLease {
            identity,
            store: self.store.clone(),
            file_path,
        })
    }
}

#[derive(Debug)]
pub(super) struct AcquiredWorkspaceLease {
    identity: WorkspaceLeaseIdentity,
    store: DetachedWorkspaceLeaseStore,
    file_path: PathBuf,
}

impl CleanRoomWorkspace {
    #[cfg(test)]
    pub(super) fn for_lease_test(root: PathBuf, epoch: EpochRecord) -> Self {
        Self {
            bundle_path: root.join("bundle.json"),
            root,
            epoch,
        }
    }
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn bundle_path(&self) -> &Path {
        &self.bundle_path
    }

    pub(crate) fn epoch(&self) -> &EpochRecord {
        &self.epoch
    }
}

/// Cleanup capability for the current checkout. Completion leases the checkout's identity; it
/// must never remove or otherwise mutate the caller's working directory on release.
#[derive(Debug, Default)]
pub(crate) struct CurrentCheckoutCleanup;

impl WorkspaceCleanup for CurrentCheckoutCleanup {
    fn cleanup(&mut self, _timeout: Duration) -> Result<()> {
        Ok(())
    }
}

/// Acquire an owned lease for the current repository checkout without materializing a worktree.
///
/// The lease records the actual current-directory device, inode, nonce, campaign, and frozen
/// epoch. A direct `.git` directory is deliberately required as the same-filesystem durable
/// lease store; linked worktrees are rejected rather than silently creating another checkout.
pub(crate) fn acquire_current_checkout_lease(
    project_root: &Path,
    bundle_path: &Path,
    campaign_id: CampaignId,
    generation: u64,
    epoch: EpochRecord,
    cleanup_timeout: Duration,
    failure_ledger: CleanupFailureLedger,
) -> Result<DetachedWorkspaceLease<CurrentCheckoutCleanup>> {
    let (root, _) = direct_directory_metadata("current repository checkout", project_root)?;
    let bundle_path = bundle_path.canonicalize().with_context(|| {
        format!(
            "canonicalize provider evidence bundle {}",
            bundle_path.display()
        )
    })?;
    if !bundle_path.is_absolute() || !bundle_path.is_file() {
        bail!("provider evidence bundle must be an absolute regular file");
    }
    let output = Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["rev-parse", "--verify", "HEAD^{commit}", "--end-of-options"])
        .output()
        .context("resolve current checkout HEAD with direct git argv")?;
    if !output.status.success() {
        bail!(
            "current checkout HEAD validation failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let observed_head = String::from_utf8(output.stdout)
        .context("current checkout HEAD was not UTF-8")?
        .trim()
        .to_string();
    if observed_head != epoch.head_oid().as_str() {
        bail!("current checkout HEAD differs from the clustered completion epoch");
    }
    let store = DetachedWorkspaceLeaseStore::open(&root.join(".git"))?;
    let context = DetachedWorkspaceLeaseContext::new(campaign_id, generation, store)?;
    let workspace = CleanRoomWorkspace {
        root,
        bundle_path,
        epoch,
    };
    let acquired = context.acquire(&workspace)?;
    Ok(DetachedWorkspaceLease::from_acquired(
        workspace,
        acquired,
        CurrentCheckoutCleanup,
        cleanup_timeout,
        failure_ledger,
    ))
}

/// Shared audit channel for failures that can only be observed from `Drop`.
#[derive(Debug, Clone, Default)]
pub(crate) struct CleanupFailureLedger {
    failures: Arc<Mutex<Vec<String>>>,
}

impl CleanupFailureLedger {
    fn record(&self, error: &anyhow::Error) {
        self.failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(format!("{error:#}"));
    }

    pub(crate) fn failures(&self) -> Vec<String> {
        self.failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

/// Owned RAII lease for one detached workspace execution.
///
/// The lease remains owned by a completion port across asynchronous calls. Every explicit use
/// validates the direct directory's device, inode, and persisted nonce before work continues.
pub(crate) struct DetachedWorkspaceLease<C: WorkspaceCleanup> {
    workspace: CleanRoomWorkspace,
    identity: WorkspaceLeaseIdentity,
    store: DetachedWorkspaceLeaseStore,
    file_path: PathBuf,
    cleanup: Option<C>,
    cleanup_timeout: Duration,
    failure_ledger: CleanupFailureLedger,
}

impl<C: WorkspaceCleanup> fmt::Debug for DetachedWorkspaceLease<C> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DetachedWorkspaceLease")
            .field("workspace", &self.workspace)
            .field("identity", &self.identity)
            .field("cleanup_armed", &self.cleanup.is_some())
            .field("cleanup_timeout", &self.cleanup_timeout)
            .finish_non_exhaustive()
    }
}

impl<C: WorkspaceCleanup> DetachedWorkspaceLease<C> {
    pub(super) fn from_acquired(
        workspace: CleanRoomWorkspace,
        acquired: AcquiredWorkspaceLease,
        cleanup: C,
        cleanup_timeout: Duration,
        failure_ledger: CleanupFailureLedger,
    ) -> Self {
        Self {
            workspace,
            identity: acquired.identity,
            store: acquired.store,
            file_path: acquired.file_path,
            cleanup: Some(cleanup),
            cleanup_timeout,
            failure_ledger,
        }
    }

    pub(crate) fn workspace(&self) -> &CleanRoomWorkspace {
        &self.workspace
    }

    /// Return the immutable identity recorded for this workspace lease.
    pub(crate) fn identity(&self) -> &WorkspaceLeaseIdentity {
        &self.identity
    }

    #[cfg(test)]
    pub(super) fn lease_file_path(&self) -> &Path {
        &self.file_path
    }

    /// Reject a replaced directory, changed lease store, cross-mount workspace, or nonce change.
    pub(crate) fn validate_current(&self) -> Result<()> {
        let store_metadata = self.store.validate_current()?;
        let (root, workspace_metadata) =
            direct_directory_metadata("detached workspace root", self.workspace.root())?;
        if root != self.identity.workspace_root()
            || workspace_metadata.dev() != self.identity.device()
            || workspace_metadata.ino() != self.identity.inode()
        {
            bail!("detached workspace identity changed after lease acquisition");
        }
        if workspace_metadata.dev() != store_metadata.dev() {
            bail!("detached workspace moved to a different filesystem after lease acquisition");
        }
        let persisted = read_lease_identity(&self.file_path)?;
        if persisted != self.identity {
            bail!("detached workspace lease nonce or identity changed after acquisition");
        }
        Ok(())
    }

    /// Perform cleanup and lease release now so callers can surface every failure synchronously.
    pub(crate) fn release(mut self) -> Result<()> {
        let Some(mut cleanup) = self.cleanup.take() else {
            return Ok(());
        };
        let result = self.release_with(&mut cleanup);
        if let Err(error) = &result {
            self.failure_ledger.record(error);
        }
        result
    }

    /// Compatibility spelling for callers that use close as their explicit release boundary.
    pub(crate) fn close(self) -> Result<()> {
        self.release()
    }

    /// Release the workspace and return the only cleanup receipt suitable for terminal evidence.
    ///
    /// No receipt is returned when identity validation, process cleanup, or lease-file removal
    /// fails, so a completion port cannot publish a terminal pair from uncertain cleanup.
    pub(crate) fn close_and_confirm(self) -> Result<CleanupConfirmation> {
        let identity = self.identity.clone();
        self.release()?;
        Ok(CleanupConfirmation::after_successful_cleanup(&identity))
    }

    fn release_with(&self, cleanup: &mut C) -> Result<()> {
        self.validate_current()
            .context("validate detached workspace lease before cleanup")?;
        cleanup
            .cleanup(self.cleanup_timeout)
            .context("clean-room workspace cleanup failed")?;
        fs::remove_file(&self.file_path).with_context(|| {
            format!(
                "release detached workspace lease {}",
                self.file_path.display()
            )
        })
    }
}

impl<C: WorkspaceCleanup> Drop for DetachedWorkspaceLease<C> {
    fn drop(&mut self) {
        let Some(mut cleanup) = self.cleanup.take() else {
            return;
        };
        if let Err(error) = self
            .release_with(&mut cleanup)
            .context("detached workspace lease cleanup fallback failed during drop")
        {
            self.failure_ledger.record(&error);
        }
    }
}

pub(crate) trait CleanRoomWorkspaceFactory {
    type Cleanup: WorkspaceCleanup;

    fn create(
        &mut self,
        source_repo: &Path,
        root: &Path,
        bundle_path: &Path,
        epoch: EpochRecord,
        lease_context: &DetachedWorkspaceLeaseContext,
    ) -> Result<DetachedWorkspaceLease<Self::Cleanup>>;
}

/// Production plan adapter. The injected driver owns any eventual process execution.
pub(crate) struct ExactOidWorkspaceFactory<D> {
    driver: D,
    cleanup_timeout: Duration,
    failure_ledger: CleanupFailureLedger,
}

impl<D> ExactOidWorkspaceFactory<D> {
    pub(crate) fn new(
        driver: D,
        cleanup_timeout: Duration,
        failure_ledger: CleanupFailureLedger,
    ) -> Self {
        Self {
            driver,
            cleanup_timeout,
            failure_ledger,
        }
    }
}

impl<D: DetachedWorkspaceDriver> CleanRoomWorkspaceFactory for ExactOidWorkspaceFactory<D> {
    type Cleanup = D::Cleanup;

    fn create(
        &mut self,
        source_repo: &Path,
        root: &Path,
        bundle_path: &Path,
        epoch: EpochRecord,
        lease_context: &DetachedWorkspaceLeaseContext,
    ) -> Result<DetachedWorkspaceLease<Self::Cleanup>> {
        absolute_utf8_path("source repository", source_repo)?;
        absolute_utf8_path("clean-room root", root)?;
        absolute_utf8_path("provider evidence bundle", bundle_path)?;
        epoch
            .validate()
            .context("validate frozen clean-room epoch")?;
        let expected_head = epoch.head_oid().as_str().to_string();
        let plan = DetachedWorkspacePlan::exact_oid(source_repo, root, &expected_head)?;
        let mut materialized = self
            .driver
            .materialize(&plan)
            .context("materialize detached exact-OID clean-room workspace")?;
        let workspace = CleanRoomWorkspace {
            root: root.to_path_buf(),
            bundle_path: bundle_path.to_path_buf(),
            epoch,
        };
        if materialized.observed_head != expected_head {
            let mismatch = anyhow!(
                "clean-room workspace did not materialize the exact frozen head: expected {expected_head}, observed {}",
                materialized.observed_head
            );
            return match materialized.cleanup.cleanup(self.cleanup_timeout) {
                Ok(()) => Err(mismatch),
                Err(cleanup_error) => Err(mismatch.context(format!(
                    "cleanup after exact-OID mismatch also failed: {cleanup_error:#}"
                ))),
            };
        }
        let acquired = match lease_context.acquire(&workspace) {
            Ok(acquired) => acquired,
            Err(acquisition_error) => {
                let cleanup_result = materialized.cleanup.cleanup(self.cleanup_timeout);
                if let Err(cleanup_error) = &cleanup_result {
                    self.failure_ledger.record(cleanup_error);
                }
                return match cleanup_result {
                    Ok(()) => Err(acquisition_error.context("acquire detached workspace lease")),
                    Err(cleanup_error) => Err(acquisition_error.context(format!(
                        "acquire detached workspace lease; cleanup after acquisition failure also failed: {cleanup_error:#}"
                    ))),
                };
            }
        };
        Ok(DetachedWorkspaceLease::from_acquired(
            workspace,
            acquired,
            materialized.cleanup,
            self.cleanup_timeout,
            self.failure_ledger.clone(),
        ))
    }
}

fn absolute_utf8_path<'a>(label: &str, path: &'a Path) -> Result<&'a str> {
    if !path.is_absolute() {
        bail!("{label} must be an absolute path: {}", path.display());
    }
    let value = path
        .to_str()
        .with_context(|| format!("{label} must be valid UTF-8: {}", path.display()))?;
    validate_process_component(label, value)?;
    Ok(value)
}

fn validate_process_component(label: &str, value: &str) -> Result<()> {
    if value.is_empty() {
        bail!("{label} must not be empty");
    }
    if value.contains('\0') {
        bail!("{label} must not contain NUL");
    }
    Ok(())
}
