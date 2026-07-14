use std::fs::{self, File, OpenOptions, Permissions};
use std::io::{Read, Take};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, anyhow, bail};
use fd_lock::RwLock;
use thiserror::Error;

use super::{CampaignId, ConvergenceEvent, ConvergenceLedger, ConvergenceLedgerEntry};
use crate::atomic_state_write::{self, AtomicPublishError};

pub(crate) const MAX_LEDGER_BYTES: u64 = 64 * 1024 * 1024;

/// A project-scoped convergence ledger with locked append-only publication.
#[derive(Debug, Clone)]
pub struct ConvergenceLedgerStore {
    convergence_dir: PathBuf,
    ledger_path: PathBuf,
    lock_path: PathBuf,
}

/// Failure from an append transaction, classified by publication safety.
#[derive(Debug, Error)]
pub enum ConvergenceAppendError {
    /// The target rename did not occur, so correcting the cause and retrying is safe.
    #[error("convergence ledger was not published: {0:#}")]
    NotPublished(#[source] anyhow::Error),
    /// The target rename occurred, but directory durability could not be confirmed.
    #[error(
        "convergence ledger may have been published, but durability is unconfirmed; reload before deciding whether to retry: {0:#}"
    )]
    PublishedButDurabilityUnconfirmed(#[source] anyhow::Error),
}

impl ConvergenceAppendError {
    /// Whether the new entry may already be visible at the ledger path.
    #[must_use]
    pub fn may_have_been_published(&self) -> bool {
        matches!(self, Self::PublishedButDurabilityUnconfirmed(_))
    }

    /// Whether retrying without first reloading can safely avoid a duplicate append.
    #[must_use]
    pub fn retry_is_safe(&self) -> bool {
        !self.may_have_been_published()
    }
}

impl ConvergenceLedgerStore {
    /// Resolve the canonical project-scoped convergence ledger paths.
    ///
    /// This constructor is side-effect free and rejects nonexistent paths and non-directories.
    ///
    /// # Errors
    ///
    /// Returns an error when the project root cannot be canonicalized, is not a directory, or
    /// the canonical project write-state root cannot be determined.
    pub fn for_project(project_root: &Path) -> anyhow::Result<Self> {
        let canonical = fs::canonicalize(project_root).with_context(|| {
            format!(
                "failed to canonicalize convergence project root {}",
                project_root.display()
            )
        })?;
        if !canonical.is_dir() {
            bail!(
                "convergence project root is not a directory: {}",
                canonical.display()
            );
        }
        let project_state_root = crate::manager::get_session_root(&canonical)?;
        Self::for_project_state_root(&project_state_root)
    }

    pub(crate) fn for_project_state_root(project_state_root: &Path) -> anyhow::Result<Self> {
        let convergence_dir = project_state_root.join("convergence");
        Ok(Self {
            ledger_path: convergence_dir.join("ledger.json"),
            lock_path: convergence_dir.join("ledger.lock"),
            convergence_dir,
        })
    }

    /// Load and validate the ledger without creating or repairing any filesystem state.
    ///
    /// Only a missing ledger is treated as an empty history. Every other error fails closed.
    ///
    /// # Errors
    ///
    /// Returns an error for unsafe file types or permissions, oversized content, invalid UTF-8
    /// or JSON, unsupported schemas, unknown fields, and invalid ledger history.
    pub fn load(&self) -> anyhow::Result<ConvergenceLedger> {
        let file = match open_readonly_regular(&self.ledger_path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(ConvergenceLedger::empty());
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "failed to securely open convergence ledger {}",
                        self.ledger_path.display()
                    )
                });
            }
        };
        read_ledger(file, &self.ledger_path)
    }

    /// Append one event under the persistent project lock and durably publish the new ledger.
    ///
    /// # Errors
    ///
    /// Returns a classified error that tells callers whether an immediate retry is safe.
    pub fn append(
        &self,
        campaign_id: CampaignId,
        event: ConvergenceEvent,
    ) -> Result<ConvergenceLedgerEntry, ConvergenceAppendError> {
        self.append_with_publisher(campaign_id, event, |directory, target, bytes| {
            atomic_state_write::publish_bytes(directory, target, bytes)
        })
    }

    fn append_with_publisher<P>(
        &self,
        campaign_id: CampaignId,
        event: ConvergenceEvent,
        publisher: P,
    ) -> Result<ConvergenceLedgerEntry, ConvergenceAppendError>
    where
        P: FnOnce(&Path, &Path, &[u8]) -> Result<(), AtomicPublishError>,
    {
        ensure_convergence_dir(&self.convergence_dir).map_err(not_published)?;
        let lock_file = open_lock_file(&self.lock_path).map_err(not_published)?;
        let mut lock = RwLock::new(lock_file);
        let _guard = lock.write().map_err(|error| {
            not_published(anyhow!(error).context(format!(
                "failed to acquire convergence ledger lock {}",
                self.lock_path.display()
            )))
        })?;

        let mut ledger = self.load().map_err(not_published)?;
        let prefix = ledger.entries().to_vec();
        ledger.append(campaign_id, event).map_err(not_published)?;
        if ledger.entries().get(..prefix.len()) != Some(prefix.as_slice()) {
            return Err(not_published(anyhow!(
                "convergence append changed the existing ledger prefix"
            )));
        }
        let appended = ledger
            .entries()
            .last()
            .cloned()
            .ok_or_else(|| not_published(anyhow!("convergence append produced no entry")))?;

        let mut bytes = serde_json::to_vec_pretty(&ledger)
            .context("failed to serialize convergence ledger")
            .map_err(not_published)?;
        bytes.push(b'\n');
        let roundtrip: ConvergenceLedger = serde_json::from_slice(&bytes)
            .context("serialized convergence ledger failed exact round-trip deserialization")
            .map_err(not_published)?;
        roundtrip
            .validate()
            .context("serialized convergence ledger failed validation")
            .map_err(not_published)?;
        if roundtrip != ledger {
            return Err(not_published(anyhow!(
                "serialized convergence ledger did not round-trip exactly"
            )));
        }
        if roundtrip.entries().get(..prefix.len()) != Some(prefix.as_slice()) {
            return Err(not_published(anyhow!(
                "serialized convergence ledger did not preserve the prior entry prefix"
            )));
        }

        publisher(&self.convergence_dir, &self.ledger_path, &bytes).map_err(map_publish_error)?;
        Ok(appended)
    }

    #[cfg(test)]
    pub(crate) fn append_with_fault(
        &self,
        campaign_id: CampaignId,
        event: ConvergenceEvent,
        fault: crate::atomic_state_write::AtomicWriteFault,
    ) -> Result<ConvergenceLedgerEntry, ConvergenceAppendError> {
        self.append_with_publisher(campaign_id, event, |directory, target, bytes| {
            atomic_state_write::publish_bytes_with_fault(directory, target, bytes, fault)
        })
    }

    #[cfg(test)]
    pub(crate) fn append_with_before_publish<F>(
        &self,
        campaign_id: CampaignId,
        event: ConvergenceEvent,
        probe: F,
    ) -> Result<ConvergenceLedgerEntry, ConvergenceAppendError>
    where
        F: FnOnce(&Path) -> anyhow::Result<()>,
    {
        self.append_with_publisher(campaign_id, event, |directory, target, bytes| {
            probe(&self.lock_path).map_err(AtomicPublishError::BeforePublish)?;
            atomic_state_write::publish_bytes(directory, target, bytes)
        })
    }
}

fn open_readonly_regular(path: &Path) -> std::io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.file_type().is_file() {
        return Err(std::io::Error::other(format!(
            "convergence ledger is not a regular file: {}",
            path.display()
        )));
    }
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "convergence ledger has insecure group/other permissions {:o}: {}",
                metadata.permissions().mode() & 0o777,
                path.display()
            ),
        ));
    }
    Ok(file)
}

fn read_ledger(file: File, path: &Path) -> anyhow::Result<ConvergenceLedger> {
    let metadata = file
        .metadata()
        .with_context(|| format!("failed to inspect convergence ledger {}", path.display()))?;
    if metadata.len() > MAX_LEDGER_BYTES {
        bail!(
            "convergence ledger exceeds maximum size of {MAX_LEDGER_BYTES} bytes: {}",
            path.display()
        );
    }

    let mut bytes = Vec::new();
    let mut bounded: Take<File> = file.take(MAX_LEDGER_BYTES + 1);
    bounded
        .read_to_end(&mut bytes)
        .with_context(|| format!("failed to read convergence ledger {}", path.display()))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_LEDGER_BYTES {
        bail!(
            "convergence ledger grew beyond maximum size of {MAX_LEDGER_BYTES} bytes while reading: {}",
            path.display()
        );
    }
    let text = std::str::from_utf8(&bytes)
        .with_context(|| format!("convergence ledger is not valid UTF-8: {}", path.display()))?;
    let ledger: ConvergenceLedger = serde_json::from_str(text).with_context(|| {
        format!(
            "failed to parse convergence ledger JSON: {}",
            path.display()
        )
    })?;
    ledger
        .validate()
        .with_context(|| format!("invalid convergence ledger history: {}", path.display()))?;
    Ok(ledger)
}

fn ensure_convergence_dir(path: &Path) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .context("convergence directory has no project-state parent")?;
    fs::create_dir_all(parent).with_context(|| {
        format!(
            "failed to create convergence project-state root {}",
            parent.display()
        )
    })?;
    match fs::create_dir(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => {
            return Err(error).with_context(|| {
                format!("failed to create convergence directory {}", path.display())
            });
        }
    }
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("failed to inspect convergence directory {}", path.display()))?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        bail!(
            "convergence path is not a real directory: {}",
            path.display()
        );
    }
    fs::set_permissions(path, Permissions::from_mode(0o700)).with_context(|| {
        format!(
            "failed to tighten convergence directory permissions: {}",
            path.display()
        )
    })?;
    Ok(())
}

fn open_lock_file(path: &Path) -> anyhow::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .with_context(|| format!("failed to open convergence lock {}", path.display()))?;
    let metadata = file
        .metadata()
        .with_context(|| format!("failed to inspect convergence lock {}", path.display()))?;
    if !metadata.file_type().is_file() {
        bail!("convergence lock is not a regular file: {}", path.display());
    }
    file.set_permissions(Permissions::from_mode(0o600))
        .with_context(|| {
            format!(
                "failed to tighten convergence lock mode: {}",
                path.display()
            )
        })?;
    let tightened = file
        .metadata()
        .with_context(|| format!("failed to verify convergence lock mode: {}", path.display()))?;
    if tightened.permissions().mode() & 0o777 != 0o600 {
        bail!("convergence lock mode is not 0600: {}", path.display());
    }
    Ok(file)
}

fn not_published(error: anyhow::Error) -> ConvergenceAppendError {
    ConvergenceAppendError::NotPublished(error)
}

fn map_publish_error(error: AtomicPublishError) -> ConvergenceAppendError {
    match error {
        AtomicPublishError::BeforePublish(error) => ConvergenceAppendError::NotPublished(error),
        AtomicPublishError::PublishedButDurabilityUnconfirmed(error) => {
            ConvergenceAppendError::PublishedButDurabilityUnconfirmed(error)
        }
    }
}
