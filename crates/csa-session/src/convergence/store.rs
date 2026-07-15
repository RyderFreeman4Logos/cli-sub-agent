use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{Read, Take};
use std::path::{Path, PathBuf};

use anyhow::{Context, anyhow, bail};
use fd_lock::RwLock;
use thiserror::Error;

use super::secure_fs::{self, SecureDirectory};
use super::{CampaignId, ConvergenceEvent, ConvergenceLedger, ConvergenceLedgerEntry};
use crate::atomic_state_write::{self, AtomicPublishError};

pub(crate) const MAX_LEDGER_BYTES: u64 = 64 * 1024 * 1024;

fn ledger_name() -> &'static OsStr {
    OsStr::new("ledger.json")
}

fn lock_name() -> &'static OsStr {
    OsStr::new("ledger.lock")
}

/// A project-scoped convergence ledger with locked append-only publication.
#[derive(Debug, Clone)]
pub struct ConvergenceLedgerStore {
    secure_boundary: PathBuf,
    project_state_root: PathBuf,
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
        "convergence ledger may have been published, but durability is unconfirmed; reload before deciding whether to retry: {source:#}"
    )]
    PublishedButDurabilityUnconfirmed {
        /// The complete entry suffix whose publication must be reconciled by event ID.
        attempted_entries: Box<[ConvergenceLedgerEntry]>,
        /// The publication or post-publication verification failure.
        #[source]
        source: anyhow::Error,
    },
}

impl ConvergenceAppendError {
    /// Whether the new entry may already be visible at the ledger path.
    #[must_use]
    pub fn may_have_been_published(&self) -> bool {
        matches!(self, Self::PublishedButDurabilityUnconfirmed { .. })
    }

    /// Whether retrying without first reloading can safely avoid a duplicate append.
    #[must_use]
    pub fn retry_is_safe(&self) -> bool {
        !self.may_have_been_published()
    }

    /// Return the complete attempted entry suffix when publication may already have occurred.
    #[must_use]
    pub fn attempted_entries(&self) -> &[ConvergenceLedgerEntry] {
        match self {
            Self::NotPublished(_) => &[],
            Self::PublishedButDurabilityUnconfirmed {
                attempted_entries, ..
            } => attempted_entries,
        }
    }

    /// Return the first attempted entry when publication may already have occurred.
    ///
    /// Single-event callers retain this convenience accessor; batch callers must reconcile the
    /// complete suffix from [`Self::attempted_entries`].
    #[must_use]
    pub fn attempted_entry(&self) -> Option<&ConvergenceLedgerEntry> {
        self.attempted_entries().first()
    }
}

impl ConvergenceLedgerStore {
    /// Resolve the canonical project-scoped convergence ledger paths.
    ///
    /// This constructor is side-effect free and rejects nonexistent paths and non-directories.
    ///
    /// # Errors
    ///
    /// Returns an error when the project root cannot be canonicalized, is not a directory, is not
    /// valid UTF-8, or the canonical project write-state root cannot be determined securely.
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
        canonical.as_os_str().to_str().with_context(|| {
            format!(
                "canonical convergence project root is not valid UTF-8: {}",
                canonical.display()
            )
        })?;
        let state_dir = csa_config::paths::state_dir_write()
            .context("failed to determine convergence state directory")?;
        let secure_boundary = state_dir
            .parent()
            .context("convergence state directory has no XDG state-home parent")?;
        let project_state_root = crate::manager::get_session_root(&canonical)?;
        Self::new(secure_boundary, &project_state_root)
    }

    #[cfg(test)]
    pub(crate) fn for_project_state_root(project_state_root: &Path) -> anyhow::Result<Self> {
        Self::new(project_state_root, project_state_root)
    }

    fn new(secure_boundary: &Path, project_state_root: &Path) -> anyhow::Result<Self> {
        secure_fs::validate_absolute_normalized(secure_boundary, "secure state boundary")?;
        secure_fs::validate_absolute_normalized(project_state_root, "project state root")?;
        if !project_state_root.starts_with(secure_boundary) {
            bail!(
                "project state root {} is outside secure boundary {}",
                project_state_root.display(),
                secure_boundary.display()
            );
        }
        let convergence_dir = project_state_root.join("convergence");
        Ok(Self {
            secure_boundary: secure_boundary.to_path_buf(),
            project_state_root: project_state_root.to_path_buf(),
            ledger_path: convergence_dir.join(ledger_name()),
            lock_path: convergence_dir.join(lock_name()),
        })
    }

    pub(super) fn secure_boundary(&self) -> &Path {
        &self.secure_boundary
    }

    pub(super) fn project_state_root(&self) -> &Path {
        &self.project_state_root
    }

    /// Load and validate the ledger without creating or repairing any filesystem state.
    ///
    /// Only a genuinely missing secure component or ledger is treated as an empty history.
    ///
    /// # Errors
    ///
    /// Returns an error for unsafe components, file types, ownership or permissions, oversized
    /// content, invalid UTF-8 or JSON, unsupported schemas, and invalid ledger history.
    pub fn load(&self) -> anyhow::Result<ConvergenceLedger> {
        let Some(directory) = secure_fs::open_convergence_directory(
            &self.secure_boundary,
            &self.project_state_root,
            false,
        )?
        else {
            return Ok(ConvergenceLedger::empty());
        };
        directory.verify_link()?;
        let ledger = self.load_from_directory(&directory)?;
        directory.verify_link()?;
        Ok(ledger)
    }

    /// Derive consolidated repair authority while holding the campaign read lock.
    ///
    /// # Errors
    /// Returns an error when the ledger or selected campaign is not fully authorized.
    pub fn authorize_consolidated_repairs_locked(
        &self,
        campaign_id: &CampaignId,
    ) -> anyhow::Result<super::ConsolidatedRepairAuthorization> {
        let Some(directory) = secure_fs::open_convergence_directory(
            &self.secure_boundary,
            &self.project_state_root,
            false,
        )?
        else {
            bail!("selected campaign {campaign_id} is missing");
        };
        let lock_file = directory.open_lock(lock_name())?;
        let lock = RwLock::new(lock_file);
        let _guard = lock
            .read()
            .map_err(|error| anyhow!(error).context("acquire convergence campaign read lock"))?;
        directory.verify_link()?;
        let ledger = self.load_from_directory(&directory)?;
        let authorization = super::authorize_consolidated_repairs(&ledger, campaign_id)?;
        directory.verify_link()?;
        Ok(authorization)
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
        self.append_transaction(
            campaign_id,
            event,
            MAX_LEDGER_BYTES,
            |_| Ok(()),
            |directory, path, bytes| {
                atomic_state_write::publish_bytes_in(
                    directory.file(),
                    Some(directory.parent()),
                    ledger_name(),
                    path,
                    bytes,
                )
            },
        )
    }

    /// Append and durably publish a complete event batch under one persistent project lock.
    ///
    /// The ledger remains its prior valid prefix until the one publication rename succeeds.
    ///
    /// # Errors
    ///
    /// Returns a classified error that tells callers whether the complete batch may need
    /// reconciliation after reloading the ledger.
    pub fn append_batch(
        &self,
        campaign_id: CampaignId,
        events: Vec<ConvergenceEvent>,
    ) -> Result<Vec<ConvergenceLedgerEntry>, ConvergenceAppendError> {
        if events.is_empty() {
            return Ok(Vec::new());
        }
        self.append_batch_transaction(
            campaign_id,
            events,
            MAX_LEDGER_BYTES,
            |_| Ok(()),
            |directory, path, bytes| {
                atomic_state_write::publish_bytes_in(
                    directory.file(),
                    Some(directory.parent()),
                    ledger_name(),
                    path,
                    bytes,
                )
            },
        )
    }

    fn append_transaction<B, P>(
        &self,
        campaign_id: CampaignId,
        event: ConvergenceEvent,
        max_bytes: u64,
        before_publish: B,
        publisher: P,
    ) -> Result<ConvergenceLedgerEntry, ConvergenceAppendError>
    where
        B: FnOnce(&SecureDirectory) -> anyhow::Result<()>,
        P: FnOnce(&SecureDirectory, &Path, &[u8]) -> Result<(), AtomicPublishError>,
    {
        let mut appended = self.append_batch_transaction(
            campaign_id,
            vec![event],
            max_bytes,
            before_publish,
            publisher,
        )?;
        appended
            .pop()
            .ok_or_else(|| not_published(anyhow!("convergence append produced no entry")))
    }

    fn append_batch_transaction<B, P>(
        &self,
        campaign_id: CampaignId,
        events: Vec<ConvergenceEvent>,
        max_bytes: u64,
        before_publish: B,
        publisher: P,
    ) -> Result<Vec<ConvergenceLedgerEntry>, ConvergenceAppendError>
    where
        B: FnOnce(&SecureDirectory) -> anyhow::Result<()>,
        P: FnOnce(&SecureDirectory, &Path, &[u8]) -> Result<(), AtomicPublishError>,
    {
        let directory = secure_fs::open_convergence_directory(
            &self.secure_boundary,
            &self.project_state_root,
            true,
        )
        .map_err(not_published)?
        .ok_or_else(|| not_published(anyhow!("secure convergence directory was not opened")))?;
        let lock_file = directory
            .open_lock(lock_name())
            .with_context(|| {
                format!(
                    "failed to securely open convergence lock {}",
                    self.lock_path.display()
                )
            })
            .map_err(not_published)?;
        let mut lock = RwLock::new(lock_file);
        let _guard = lock.write().map_err(|error| {
            not_published(anyhow!(error).context(format!(
                "failed to acquire convergence ledger lock {}",
                self.lock_path.display()
            )))
        })?;

        directory.verify_link().map_err(not_published)?;
        let mut ledger = self
            .load_from_directory(&directory)
            .map_err(not_published)?;
        let prefix = ledger.entries().to_vec();
        let event_count = events.len();
        ledger
            .append_batch(campaign_id, events)
            .map_err(not_published)?;
        if ledger.entries().get(..prefix.len()) != Some(prefix.as_slice()) {
            return Err(not_published(anyhow!(
                "convergence append changed the existing ledger prefix"
            )));
        }
        let appended = ledger.entries()[prefix.len()..].to_vec();
        if appended.len() != event_count {
            return Err(not_published(anyhow!(
                "convergence append batch produced {} entries for {event_count} events",
                appended.len()
            )));
        }
        let bytes = serialize_ledger(&ledger, &prefix, max_bytes).map_err(not_published)?;

        before_publish(&directory).map_err(not_published)?;
        directory.verify_link().map_err(not_published)?;
        publisher(&directory, &self.ledger_path, &bytes)
            .map_err(|error| map_publish_error(error, &appended))?;
        directory
            .verify_link()
            .map_err(|error| uncertain(error, &appended))?;
        Ok(appended)
    }

    fn load_from_directory(
        &self,
        directory: &SecureDirectory,
    ) -> anyhow::Result<ConvergenceLedger> {
        let Some(file) = directory.open_ledger(ledger_name()).with_context(|| {
            format!(
                "failed to securely open convergence ledger {}",
                self.ledger_path.display()
            )
        })?
        else {
            return Ok(ConvergenceLedger::empty());
        };
        read_ledger(file, &self.ledger_path)
    }

    #[cfg(test)]
    pub(crate) fn append_with_fault(
        &self,
        campaign_id: CampaignId,
        event: ConvergenceEvent,
        fault: crate::atomic_state_write::AtomicWriteFault,
    ) -> Result<ConvergenceLedgerEntry, ConvergenceAppendError> {
        self.append_transaction(
            campaign_id,
            event,
            MAX_LEDGER_BYTES,
            |_| Ok(()),
            |directory, path, bytes| {
                atomic_state_write::publish_bytes_in_with_fault(
                    directory.file(),
                    Some(directory.parent()),
                    ledger_name(),
                    path,
                    bytes,
                    fault,
                )
            },
        )
    }

    #[cfg(test)]
    pub(crate) fn append_batch_with_fault(
        &self,
        campaign_id: CampaignId,
        events: Vec<ConvergenceEvent>,
        fault: crate::atomic_state_write::AtomicWriteFault,
    ) -> Result<Vec<ConvergenceLedgerEntry>, ConvergenceAppendError> {
        self.append_batch_transaction(
            campaign_id,
            events,
            MAX_LEDGER_BYTES,
            |_| Ok(()),
            |directory, path, bytes| {
                atomic_state_write::publish_bytes_in_with_fault(
                    directory.file(),
                    Some(directory.parent()),
                    ledger_name(),
                    path,
                    bytes,
                    fault,
                )
            },
        )
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
        self.append_transaction(
            campaign_id,
            event,
            MAX_LEDGER_BYTES,
            |_| probe(&self.lock_path),
            |directory, path, bytes| {
                atomic_state_write::publish_bytes_in(
                    directory.file(),
                    Some(directory.parent()),
                    ledger_name(),
                    path,
                    bytes,
                )
            },
        )
    }

    #[cfg(test)]
    pub(crate) fn append_with_max_for_test(
        &self,
        campaign_id: CampaignId,
        event: ConvergenceEvent,
        max_bytes: u64,
    ) -> Result<ConvergenceLedgerEntry, ConvergenceAppendError> {
        self.append_transaction(
            campaign_id,
            event,
            max_bytes,
            |_| Ok(()),
            |directory, path, bytes| {
                atomic_state_write::publish_bytes_in(
                    directory.file(),
                    Some(directory.parent()),
                    ledger_name(),
                    path,
                    bytes,
                )
            },
        )
    }
}

fn serialize_ledger(
    ledger: &ConvergenceLedger,
    prefix: &[ConvergenceLedgerEntry],
    max_bytes: u64,
) -> anyhow::Result<Vec<u8>> {
    let mut bytes =
        serde_json::to_vec_pretty(ledger).context("failed to serialize convergence ledger")?;
    bytes.push(b'\n');
    let serialized_len = u64::try_from(bytes.len())
        .context("serialized convergence ledger length does not fit in u64")?;
    if serialized_len > max_bytes {
        bail!(
            "serialized convergence ledger exceeds maximum size of {max_bytes} bytes ({serialized_len} bytes)"
        );
    }
    let roundtrip: ConvergenceLedger = serde_json::from_slice(&bytes)
        .context("serialized convergence ledger failed exact round-trip deserialization")?;
    roundtrip
        .validate()
        .context("serialized convergence ledger failed validation")?;
    if &roundtrip != ledger {
        bail!("serialized convergence ledger did not round-trip exactly");
    }
    if roundtrip.entries().get(..prefix.len()) != Some(prefix) {
        bail!("serialized convergence ledger did not preserve the prior entry prefix");
    }
    Ok(bytes)
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
    let bytes_read = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    if bytes_read > MAX_LEDGER_BYTES {
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

fn not_published(error: anyhow::Error) -> ConvergenceAppendError {
    ConvergenceAppendError::NotPublished(error)
}

fn uncertain(
    source: anyhow::Error,
    attempted_entries: &[ConvergenceLedgerEntry],
) -> ConvergenceAppendError {
    ConvergenceAppendError::PublishedButDurabilityUnconfirmed {
        attempted_entries: attempted_entries.into(),
        source,
    }
}

fn map_publish_error(
    error: AtomicPublishError,
    attempted_entries: &[ConvergenceLedgerEntry],
) -> ConvergenceAppendError {
    match error {
        AtomicPublishError::BeforePublish(error) => ConvergenceAppendError::NotPublished(error),
        AtomicPublishError::PublishedButDurabilityUnconfirmed(error) => {
            uncertain(error, attempted_entries)
        }
    }
}
