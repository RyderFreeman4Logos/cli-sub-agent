//! Durable storage operations for the completion action journal.

use std::ffi::OsStr;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, anyhow, bail};
use fd_lock::RwLock;
use thiserror::Error;

use super::secure_fs::{self, SecureDirectory};
use super::{
    CampaignId, CompletionActionClaim, CompletionActionId, CompletionActionJournal,
    CompletionActionJournalError, CompletionActionJournalRead, ConvergenceLedgerStore, EpochId,
    Sha256Digest,
};
use crate::atomic_state_write::{self, AtomicPublishError};

const MAX_COMPLETION_ACTION_JOURNAL_BYTES: u64 = 8 * 1024 * 1024;

fn action_journal_name() -> &'static OsStr {
    OsStr::new("completion-actions.json")
}

/// Failure while durably recording a completion action journal transition.
#[derive(Debug, Error)]
pub enum CompletionActionJournalStoreError {
    /// The requested transition was not published, so no external action may start from it.
    #[error("completion action journal was not published: {0:#}")]
    NotPublished(#[source] anyhow::Error),
    /// The target rename may have happened, so the caller must reload and fail closed.
    #[error(
        "completion action journal may have been published, but durability is unconfirmed; reload before attempting recovery: {0:#}"
    )]
    PublishedButDurabilityUnconfirmed(#[source] anyhow::Error),
}

impl CompletionActionJournalStoreError {
    /// Whether the latest transition may already be visible on disk.
    #[must_use]
    pub fn may_have_been_published(&self) -> bool {
        matches!(self, Self::PublishedButDurabilityUnconfirmed(_))
    }
}

impl ConvergenceLedgerStore {
    /// Read the completion action journal without creating, repairing, or resuming it.
    ///
    /// A v1 journal is returned as an explicitly read-only value. Unknown and mixed schemas are
    /// errors, never an implicit resume.
    pub fn load_completion_action_journal(&self) -> anyhow::Result<CompletionActionJournalRead> {
        let Some(directory) = secure_fs::open_convergence_directory(
            self.secure_boundary(),
            self.project_state_root(),
            false,
        )?
        else {
            return Ok(CompletionActionJournalRead::Missing);
        };
        directory.verify_link()?;
        let journal = self.load_completion_action_journal_from_directory(&directory)?;
        directory.verify_link()?;
        Ok(journal)
    }

    /// Initialize the only schema this binary can write for an exact campaign, epoch, and policy.
    ///
    /// # Errors
    ///
    /// Returns an error when any journal already exists, including a legacy v1 journal.
    pub fn initialize_completion_action_journal(
        &self,
        campaign_id: CampaignId,
        epoch_id: EpochId,
        policy_digest: Sha256Digest,
    ) -> Result<CompletionActionJournal, CompletionActionJournalStoreError> {
        let directory = self.open_completion_action_journal_directory()?;
        let mut lock = self.open_completion_action_journal_lock(&directory)?;
        let _guard = lock.write().map_err(|error| {
            action_not_published(anyhow!(error).context("acquire completion action journal lock"))
        })?;
        directory.verify_link().map_err(action_not_published)?;
        match self
            .load_completion_action_journal_from_directory(&directory)
            .map_err(action_not_published)?
        {
            CompletionActionJournalRead::Missing => {}
            CompletionActionJournalRead::LegacyV1(_) => {
                return Err(action_not_published(anyhow!(
                    CompletionActionJournalError::LegacyReadOnly
                )));
            }
            CompletionActionJournalRead::Current(_) => {
                return Err(action_not_published(anyhow!(
                    "completion action journal already exists"
                )));
            }
        }
        let journal = CompletionActionJournal::new(campaign_id, epoch_id, policy_digest);
        self.publish_completion_action_journal(&directory, &journal)?;
        directory.verify_link().map_err(action_uncertain)?;
        Ok(journal)
    }

    /// Atomically claim the next external action using `expected_generation` as a CAS fence.
    pub fn claim_completion_action(
        &self,
        expected_generation: u64,
        action_id: CompletionActionId,
    ) -> Result<CompletionActionClaim, CompletionActionJournalStoreError> {
        self.update_completion_action_journal(|journal| {
            journal.claim_next(expected_generation, action_id)
        })
    }

    /// Atomically mark a started claim uncertain and issue a newer fenced replacement claim.
    pub fn recover_completion_action(
        &self,
        previous: &CompletionActionClaim,
        next_action_id: CompletionActionId,
    ) -> Result<CompletionActionClaim, CompletionActionJournalStoreError> {
        self.update_completion_action_journal(|journal| {
            journal.recover_and_claim(previous, next_action_id)
        })
    }

    /// Persist an uncertain external action state without allowing a silent completion.
    pub fn mark_completion_action_uncertain(
        &self,
        claim: &CompletionActionClaim,
    ) -> Result<(), CompletionActionJournalStoreError> {
        self.update_completion_action_journal(|journal| journal.mark_uncertain(claim))
    }

    /// Verify under the shared lock that no completion action blocks terminal attestation.
    ///
    /// The terminal publication path repeats this check under its write transaction; this public
    /// read check is for callers that need to reject an incomplete recovery before assembling
    /// attestation evidence.
    pub fn verify_completion_action_journal_attestable(
        &self,
    ) -> Result<(), CompletionActionJournalStoreError> {
        let Some(directory) = secure_fs::open_convergence_directory(
            self.secure_boundary(),
            self.project_state_root(),
            false,
        )
        .map_err(action_not_published)?
        else {
            return Ok(());
        };
        let lock_file = directory
            .open_lock(super::store::lock_name())
            .map_err(action_not_published)?;
        let lock = RwLock::new(lock_file);
        let _guard = lock.read().map_err(|error| {
            action_not_published(anyhow!(error).context("acquire completion action journal lock"))
        })?;
        directory.verify_link().map_err(action_not_published)?;
        self.require_completion_action_journal_attestable(&directory)
            .map_err(action_not_published)?;
        directory.verify_link().map_err(action_not_published)
    }

    /// Finish an external action only when its claim remains the current generation holder.
    pub fn finish_completion_action(
        &self,
        claim: &CompletionActionClaim,
    ) -> Result<(), CompletionActionJournalStoreError> {
        self.update_completion_action_journal(|journal| journal.finish(claim))
    }

    pub(super) fn require_completion_action_journal_attestable(
        &self,
        directory: &SecureDirectory,
    ) -> anyhow::Result<()> {
        match self.load_completion_action_journal_from_directory(directory)? {
            CompletionActionJournalRead::Missing => Ok(()),
            CompletionActionJournalRead::LegacyV1(_) => {
                Err(anyhow!(CompletionActionJournalError::LegacyReadOnly))
            }
            CompletionActionJournalRead::Current(journal) if journal.permits_attestation() => {
                Ok(())
            }
            CompletionActionJournalRead::Current(_) => Err(anyhow!(
                CompletionActionJournalError::IncompleteForAttestation
            )),
        }
    }

    fn load_completion_action_journal_from_directory(
        &self,
        directory: &SecureDirectory,
    ) -> anyhow::Result<CompletionActionJournalRead> {
        let path = completion_action_journal_path(self);
        let Some(file) = directory
            .open_private_file(action_journal_name())
            .with_context(|| {
                format!(
                    "failed to securely open completion action journal {}",
                    path.display()
                )
            })?
        else {
            return Ok(CompletionActionJournalRead::Missing);
        };
        read_completion_action_journal(file, &path)
    }

    fn open_completion_action_journal_directory(
        &self,
    ) -> Result<SecureDirectory, CompletionActionJournalStoreError> {
        secure_fs::open_convergence_directory(
            self.secure_boundary(),
            self.project_state_root(),
            true,
        )
        .map_err(action_not_published)?
        .ok_or_else(|| action_not_published(anyhow!("secure convergence directory was not opened")))
    }

    fn open_completion_action_journal_lock(
        &self,
        directory: &SecureDirectory,
    ) -> Result<RwLock<File>, CompletionActionJournalStoreError> {
        let lock_file = directory
            .open_lock(super::store::lock_name())
            .with_context(|| {
                format!(
                    "failed to securely open completion action journal lock {}",
                    completion_action_journal_lock_path(self).display()
                )
            });
        lock_file.map(RwLock::new).map_err(action_not_published)
    }

    fn update_completion_action_journal<T>(
        &self,
        update: impl FnOnce(&mut CompletionActionJournal) -> Result<T, CompletionActionJournalError>,
    ) -> Result<T, CompletionActionJournalStoreError> {
        let directory = self.open_completion_action_journal_directory()?;
        let mut lock = self.open_completion_action_journal_lock(&directory)?;
        let _guard = lock.write().map_err(|error| {
            action_not_published(anyhow!(error).context("acquire completion action journal lock"))
        })?;
        directory.verify_link().map_err(action_not_published)?;
        let mut journal = match self
            .load_completion_action_journal_from_directory(&directory)
            .map_err(action_not_published)?
        {
            CompletionActionJournalRead::Missing => {
                return Err(action_not_published(anyhow!(
                    "completion action journal is missing"
                )));
            }
            CompletionActionJournalRead::LegacyV1(_) => {
                return Err(action_not_published(anyhow!(
                    CompletionActionJournalError::LegacyReadOnly
                )));
            }
            CompletionActionJournalRead::Current(journal) => journal,
        };
        let result = update(&mut journal).map_err(|error| action_not_published(anyhow!(error)))?;
        self.publish_completion_action_journal(&directory, &journal)?;
        directory.verify_link().map_err(action_uncertain)?;
        Ok(result)
    }

    fn publish_completion_action_journal(
        &self,
        directory: &SecureDirectory,
        journal: &CompletionActionJournal,
    ) -> Result<(), CompletionActionJournalStoreError> {
        let bytes = serialize_completion_action_journal(journal).map_err(action_not_published)?;
        atomic_state_write::publish_bytes_in(
            directory.file(),
            Some(directory.parent()),
            action_journal_name(),
            &completion_action_journal_path(self),
            &bytes,
        )
        .map_err(map_action_publish_error)
    }
}

fn completion_action_journal_path(store: &ConvergenceLedgerStore) -> std::path::PathBuf {
    store
        .project_state_root()
        .join("convergence")
        .join(action_journal_name())
}

fn completion_action_journal_lock_path(store: &ConvergenceLedgerStore) -> std::path::PathBuf {
    store
        .project_state_root()
        .join("convergence")
        .join(super::store::lock_name())
}

fn read_completion_action_journal(
    file: File,
    path: &Path,
) -> anyhow::Result<CompletionActionJournalRead> {
    let metadata = file.metadata().with_context(|| {
        format!(
            "failed to inspect completion action journal {}",
            path.display()
        )
    })?;
    if metadata.len() > MAX_COMPLETION_ACTION_JOURNAL_BYTES {
        bail!(
            "completion action journal exceeds maximum size of {MAX_COMPLETION_ACTION_JOURNAL_BYTES} bytes: {}",
            path.display()
        );
    }
    let mut bytes = Vec::new();
    let mut bounded = file.take(MAX_COMPLETION_ACTION_JOURNAL_BYTES + 1);
    bounded.read_to_end(&mut bytes).with_context(|| {
        format!(
            "failed to read completion action journal {}",
            path.display()
        )
    })?;
    let bytes_read = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    if bytes_read > MAX_COMPLETION_ACTION_JOURNAL_BYTES {
        bail!(
            "completion action journal grew beyond maximum size of {MAX_COMPLETION_ACTION_JOURNAL_BYTES} bytes while reading: {}",
            path.display()
        );
    }
    super::action_journal::read_journal(&bytes).map_err(|error| {
        anyhow!(
            "invalid completion action journal {}: {error}",
            path.display()
        )
    })
}

fn serialize_completion_action_journal(
    journal: &CompletionActionJournal,
) -> anyhow::Result<Vec<u8>> {
    journal.validate().map_err(anyhow::Error::from)?;
    let mut bytes = serde_json::to_vec_pretty(journal)
        .context("failed to serialize completion action journal")?;
    bytes.push(b'\n');
    let serialized_len = u64::try_from(bytes.len())
        .context("serialized completion action journal length does not fit in u64")?;
    if serialized_len > MAX_COMPLETION_ACTION_JOURNAL_BYTES {
        bail!(
            "serialized completion action journal exceeds maximum size of {MAX_COMPLETION_ACTION_JOURNAL_BYTES} bytes ({serialized_len} bytes)"
        );
    }
    let roundtrip = CompletionActionJournal::parse_current(&bytes)?;
    if &roundtrip != journal {
        bail!("serialized completion action journal did not round-trip exactly");
    }
    Ok(bytes)
}

fn action_not_published(error: anyhow::Error) -> CompletionActionJournalStoreError {
    CompletionActionJournalStoreError::NotPublished(error)
}

fn action_uncertain(error: anyhow::Error) -> CompletionActionJournalStoreError {
    CompletionActionJournalStoreError::PublishedButDurabilityUnconfirmed(error)
}

fn map_action_publish_error(error: AtomicPublishError) -> CompletionActionJournalStoreError {
    match error {
        AtomicPublishError::BeforePublish(error) => action_not_published(error),
        AtomicPublishError::PublishedButDurabilityUnconfirmed(error) => action_uncertain(error),
    }
}
