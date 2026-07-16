//! Durable storage operations for the completion action journal.

use std::ffi::OsStr;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, anyhow, bail};
use fd_lock::RwLock;
use thiserror::Error;

use super::action_journal_selector::{ActiveJournalSelectorRead, CompletionActionJournalSelector};
use super::secure_fs::{self, SecureDirectory};
use super::{
    CompletionActionClaim, CompletionActionId, CompletionActionJournal,
    CompletionActionJournalError, CompletionActionJournalRead, CompletionActionState,
    ConvergenceLedgerStore, ProviderTurnExecutionId, ProviderTurnReservation,
    TerminalExecutionBinding,
};
use crate::atomic_state_write::{self, AtomicPublishError};

const MAX_COMPLETION_ACTION_JOURNAL_BYTES: u64 = 8 * 1024 * 1024;

pub(super) fn action_journal_name() -> &'static OsStr {
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

    /// Test-only crash injection for the durable action claim publication boundary.
    #[cfg(test)]
    pub(crate) fn claim_completion_action_with_fault(
        &self,
        expected_generation: u64,
        action_id: CompletionActionId,
        fault: crate::atomic_state_write::AtomicWriteFault,
    ) -> Result<CompletionActionClaim, CompletionActionJournalStoreError> {
        self.update_completion_action_journal_with_fault(
            |journal| journal.claim_next(expected_generation, action_id),
            fault,
        )
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

    /// Persist a provider-turn reservation before the caller may send the provider request.
    pub fn reserve_completion_provider_turn(
        &self,
        claim: &CompletionActionClaim,
        execution_id: ProviderTurnExecutionId,
        reserved_turns: u32,
    ) -> Result<ProviderTurnReservation, CompletionActionJournalStoreError> {
        self.update_completion_action_journal(|journal| {
            journal.reserve_provider_turn(claim, execution_id, reserved_turns)
        })
    }

    /// Test-only crash injection for the durable provider reservation boundary.
    #[cfg(test)]
    pub(crate) fn reserve_completion_provider_turn_with_fault(
        &self,
        claim: &CompletionActionClaim,
        execution_id: ProviderTurnExecutionId,
        reserved_turns: u32,
        fault: crate::atomic_state_write::AtomicWriteFault,
    ) -> Result<ProviderTurnReservation, CompletionActionJournalStoreError> {
        self.update_completion_action_journal_with_fault(
            |journal| journal.reserve_provider_turn(claim, execution_id, reserved_turns),
            fault,
        )
    }

    /// Reconcile a provider execution exactly once with its host-observed turn delta.
    pub fn reconcile_completion_provider_turn(
        &self,
        reservation: &ProviderTurnReservation,
        observed_turn_delta: u32,
    ) -> Result<(), CompletionActionJournalStoreError> {
        self.update_completion_action_journal(|journal| {
            journal.reconcile_provider_turn(reservation, observed_turn_delta)
        })
    }

    /// Release a reservation only after proving that the provider was never sent.
    pub fn release_completion_provider_turn_before_send(
        &self,
        reservation: &ProviderTurnReservation,
    ) -> Result<(), CompletionActionJournalStoreError> {
        self.update_completion_action_journal(|journal| {
            journal.release_provider_turn_before_send(reservation)
        })
    }

    /// Mark a provider reservation indeterminate when recovery cannot safely bound its usage.
    pub fn mark_completion_provider_turn_usage_indeterminate(
        &self,
        reservation: &ProviderTurnReservation,
    ) -> Result<(), CompletionActionJournalStoreError> {
        self.update_completion_action_journal(|journal| {
            journal.mark_provider_turn_usage_indeterminate(reservation)
        })
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

    /// Require the current completed action and authorization generation that terminal evidence
    /// claims, while the caller already holds the ledger transaction lock.
    pub(super) fn require_terminal_execution_binding(
        &self,
        directory: &SecureDirectory,
        binding: &TerminalExecutionBinding,
    ) -> anyhow::Result<()> {
        let CompletionActionJournalRead::Current(journal) = self
            .load_completion_action_journal_partition(
                directory,
                &CompletionActionJournalSelector::new(
                    binding.campaign_id().clone(),
                    binding.epoch_id().clone(),
                    binding.policy_digest().clone(),
                ),
            )?
        else {
            bail!("terminal publication requires a current completion action journal");
        };
        if journal.campaign_id() != binding.campaign_id()
            || journal.epoch_id() != binding.epoch_id()
            || !journal.permits_attestation()
        {
            bail!("terminal execution journal does not match or permit the claimed terminal epoch");
        }
        if journal.generation() != binding.action_generation() {
            bail!(
                "terminal execution binding does not name the latest completion action generation"
            );
        }
        let latest = journal
            .actions()
            .last()
            .context("terminal execution binding names an empty completion action journal")?;
        if latest.state() != CompletionActionState::Finished
            || latest.claim().action_id() != binding.action_id()
            || latest.claim().generation() != binding.action_generation()
        {
            bail!(
                "terminal execution binding does not name the latest completed completion action"
            );
        }
        Ok(())
    }

    pub(super) fn load_completion_action_journal_from_directory(
        &self,
        directory: &SecureDirectory,
    ) -> anyhow::Result<CompletionActionJournalRead> {
        match self.load_active_journal_selector(directory)? {
            ActiveJournalSelectorRead::Missing => Ok(CompletionActionJournalRead::Missing),
            ActiveJournalSelectorRead::Current(selector) => {
                self.load_completion_action_journal_partition(directory, &selector)
            }
            ActiveJournalSelectorRead::Legacy(journal) => Ok(journal),
        }
    }

    pub(super) fn load_active_journal_selector(
        &self,
        directory: &SecureDirectory,
    ) -> anyhow::Result<ActiveJournalSelectorRead> {
        let path = completion_action_journal_selector_path(self);
        let Some(file) = directory
            .open_private_file(action_journal_name())
            .with_context(|| {
                format!(
                    "failed to securely open completion action journal {}",
                    path.display()
                )
            })?
        else {
            return Ok(ActiveJournalSelectorRead::Missing);
        };
        let bytes = read_completion_action_journal_bytes(file, &path)?;
        match serde_json::from_slice::<CompletionActionJournalSelector>(&bytes) {
            Ok(selector) => {
                selector.validate()?;
                Ok(ActiveJournalSelectorRead::Current(selector))
            }
            Err(_) => Ok(ActiveJournalSelectorRead::Legacy(
                super::action_journal::read_journal(&bytes).map_err(|error| {
                    anyhow!(
                        "invalid completion action journal selector {}: {error}",
                        path.display()
                    )
                })?,
            )),
        }
    }

    pub(super) fn load_completion_action_journal_partition(
        &self,
        directory: &SecureDirectory,
        selector: &CompletionActionJournalSelector,
    ) -> anyhow::Result<CompletionActionJournalRead> {
        let name = selector.journal_name();
        let path = completion_action_journal_partition_path(self, selector);
        let Some(file) = directory.open_private_file(&name).with_context(|| {
            format!(
                "failed to securely open completion action journal partition {}",
                path.display()
            )
        })?
        else {
            return Ok(CompletionActionJournalRead::Missing);
        };
        let journal = read_completion_action_journal(file, &path)?;
        if let CompletionActionJournalRead::Current(current) = &journal
            && CompletionActionJournalSelector::from_journal(current) != *selector
        {
            bail!("completion action journal partition identity does not match its filename scope");
        }
        Ok(journal)
    }
    pub(super) fn open_completion_action_journal_directory(
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

    pub(super) fn open_completion_action_journal_lock(
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
        self.update_completion_action_journal_with_publisher(update, |directory, journal| {
            self.publish_completion_action_journal(directory, journal)
        })
    }

    fn update_completion_action_journal_with_publisher<T>(
        &self,
        update: impl FnOnce(&mut CompletionActionJournal) -> Result<T, CompletionActionJournalError>,
        publish: impl FnOnce(
            &SecureDirectory,
            &CompletionActionJournal,
        ) -> Result<(), CompletionActionJournalStoreError>,
    ) -> Result<T, CompletionActionJournalStoreError> {
        let directory = self.open_completion_action_journal_directory()?;
        let mut lock = self.open_completion_action_journal_lock(&directory)?;
        let _guard = lock.write().map_err(|error| {
            action_not_published(anyhow!(error).context("acquire completion action journal lock"))
        })?;
        directory.verify_link().map_err(action_not_published)?;
        let selector = match self
            .load_active_journal_selector(&directory)
            .map_err(action_not_published)?
        {
            ActiveJournalSelectorRead::Current(selector) => selector,
            ActiveJournalSelectorRead::Legacy(_) => {
                return Err(action_not_published(anyhow!(
                    CompletionActionJournalError::LegacyReadOnly
                )));
            }
            ActiveJournalSelectorRead::Missing => {
                return Err(action_not_published(anyhow!(
                    "completion action journal selector is missing"
                )));
            }
        };
        let mut journal = match self
            .load_completion_action_journal_partition(&directory, &selector)
            .map_err(action_not_published)?
        {
            CompletionActionJournalRead::Current(journal) => journal,
            CompletionActionJournalRead::Missing => {
                return Err(action_not_published(anyhow!(
                    "selected completion action journal partition is missing"
                )));
            }
            CompletionActionJournalRead::LegacyV1(_) => {
                return Err(action_not_published(anyhow!(
                    CompletionActionJournalError::LegacyReadOnly
                )));
            }
        };
        let result = update(&mut journal).map_err(|error| action_not_published(anyhow!(error)))?;
        publish(&directory, &journal)?;
        directory.verify_link().map_err(action_uncertain)?;
        Ok(result)
    }

    #[cfg(test)]
    fn update_completion_action_journal_with_fault<T>(
        &self,
        update: impl FnOnce(&mut CompletionActionJournal) -> Result<T, CompletionActionJournalError>,
        fault: crate::atomic_state_write::AtomicWriteFault,
    ) -> Result<T, CompletionActionJournalStoreError> {
        self.update_completion_action_journal_with_publisher(update, |directory, journal| {
            self.publish_completion_action_journal_with_fault(directory, journal, fault)
        })
    }

    pub(super) fn publish_completion_action_journal(
        &self,
        directory: &SecureDirectory,
        journal: &CompletionActionJournal,
    ) -> Result<(), CompletionActionJournalStoreError> {
        let bytes = serialize_completion_action_journal(journal).map_err(action_not_published)?;
        let selector = CompletionActionJournalSelector::from_journal(journal);
        let name = selector.journal_name();
        atomic_state_write::publish_bytes_in(
            directory.file(),
            Some(directory.parent()),
            &name,
            &completion_action_journal_partition_path(self, &selector),
            &bytes,
        )
        .map_err(map_action_publish_error)
    }

    #[cfg(test)]
    fn publish_completion_action_journal_with_fault(
        &self,
        directory: &SecureDirectory,
        journal: &CompletionActionJournal,
        fault: crate::atomic_state_write::AtomicWriteFault,
    ) -> Result<(), CompletionActionJournalStoreError> {
        let bytes = serialize_completion_action_journal(journal).map_err(action_not_published)?;
        let selector = CompletionActionJournalSelector::from_journal(journal);
        let name = selector.journal_name();
        atomic_state_write::publish_bytes_in_with_fault(
            directory.file(),
            Some(directory.parent()),
            &name,
            &completion_action_journal_partition_path(self, &selector),
            &bytes,
            fault,
        )
        .map_err(map_action_publish_error)
    }
}

pub(super) fn completion_action_journal_selector_path(
    store: &ConvergenceLedgerStore,
) -> std::path::PathBuf {
    store
        .project_state_root()
        .join("convergence")
        .join(action_journal_name())
}

fn completion_action_journal_partition_path(
    store: &ConvergenceLedgerStore,
    selector: &CompletionActionJournalSelector,
) -> std::path::PathBuf {
    store
        .project_state_root()
        .join("convergence")
        .join(selector.journal_name())
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
    let bytes = read_completion_action_journal_bytes(file, path)?;
    super::action_journal::read_journal(&bytes).map_err(|error| {
        anyhow!(
            "invalid completion action journal {}: {error}",
            path.display()
        )
    })
}

fn read_completion_action_journal_bytes(file: File, path: &Path) -> anyhow::Result<Vec<u8>> {
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
    Ok(bytes)
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

pub(super) fn action_not_published(error: anyhow::Error) -> CompletionActionJournalStoreError {
    CompletionActionJournalStoreError::NotPublished(error)
}

pub(super) fn action_uncertain(error: anyhow::Error) -> CompletionActionJournalStoreError {
    CompletionActionJournalStoreError::PublishedButDurabilityUnconfirmed(error)
}

pub(super) fn map_action_publish_error(
    error: AtomicPublishError,
) -> CompletionActionJournalStoreError {
    match error {
        AtomicPublishError::BeforePublish(error) => action_not_published(error),
        AtomicPublishError::PublishedButDurabilityUnconfirmed(error) => action_uncertain(error),
    }
}
