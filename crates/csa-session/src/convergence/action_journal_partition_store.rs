//! Atomic selection and initialization of exact completion-journal partitions.

use anyhow::{Context, anyhow};

use super::action_journal_selector::{ActiveJournalSelectorRead, CompletionActionJournalSelector};
use super::action_journal_store::{action_not_published, action_uncertain};
use super::{
    CampaignId, CompletionActionJournalError, CompletionActionJournalRead, ConvergenceLedgerStore,
    EpochId, Sha256Digest,
};

impl ConvergenceLedgerStore {
    /// Read one exact journal partition without consulting the active-journal selector.
    ///
    /// Terminal publication uses this boundary so an E1 selector can never satisfy E2 evidence.
    pub fn load_completion_action_journal_for(
        &self,
        campaign_id: &CampaignId,
        epoch_id: &EpochId,
        policy_digest: &Sha256Digest,
    ) -> anyhow::Result<CompletionActionJournalRead> {
        let Some(directory) = super::secure_fs::open_convergence_directory(
            self.secure_boundary(),
            self.project_state_root(),
            false,
        )?
        else {
            return Ok(CompletionActionJournalRead::Missing);
        };
        directory.verify_link()?;
        let selector = CompletionActionJournalSelector::new(
            campaign_id.clone(),
            epoch_id.clone(),
            policy_digest.clone(),
        );
        let journal = self.load_completion_action_journal_partition(&directory, &selector)?;
        directory.verify_link()?;
        Ok(journal)
    }

    /// Initialize the only schema this binary can write for an exact campaign, epoch, and policy.
    ///
    /// # Errors
    ///
    /// Returns an error when a journal for the requested scope already exists, when the active
    /// journal is unfinished, or when the requested scope is not the ledger's current scope.
    pub fn initialize_completion_action_journal(
        &self,
        campaign_id: CampaignId,
        epoch_id: EpochId,
        policy_digest: Sha256Digest,
    ) -> Result<super::CompletionActionJournal, super::CompletionActionJournalStoreError> {
        let directory = self.open_completion_action_journal_directory()?;
        let mut lock = self.open_completion_action_journal_lock(&directory)?;
        let _guard = lock.write().map_err(|error| {
            action_not_published(anyhow!(error).context("acquire completion action journal lock"))
        })?;
        directory.verify_link().map_err(action_not_published)?;
        let requested = CompletionActionJournalSelector::new(campaign_id, epoch_id, policy_digest);
        match self
            .load_active_journal_selector(&directory)
            .map_err(action_not_published)?
        {
            ActiveJournalSelectorRead::Missing => {
                self.initialize_selected_completion_action_journal(&directory, &requested)?;
            }
            ActiveJournalSelectorRead::Legacy(_) => {
                return Err(action_not_published(anyhow!(
                    CompletionActionJournalError::LegacyReadOnly
                )));
            }
            ActiveJournalSelectorRead::Current(active) if active == requested => {
                return Err(action_not_published(anyhow!(
                    "completion action journal already exists"
                )));
            }
            ActiveJournalSelectorRead::Current(active) => {
                let active_journal = self
                    .load_completion_action_journal_partition(&directory, &active)
                    .map_err(action_not_published)?;
                let CompletionActionJournalRead::Current(active_journal) = active_journal else {
                    return Err(action_not_published(anyhow!(
                        "active completion action journal selector does not reference a current journal"
                    )));
                };
                if !active_journal.permits_attestation() {
                    return Err(action_not_published(anyhow!(
                        "completion action journal contains an unresolved action"
                    )));
                }
                self.require_current_transition_scope(&directory, &requested)
                    .map_err(action_not_published)?;
                self.initialize_selected_completion_action_journal(&directory, &requested)?;
            }
        }
        let CompletionActionJournalRead::Current(journal) = self
            .load_completion_action_journal_partition(&directory, &requested)
            .map_err(action_not_published)?
        else {
            return Err(action_not_published(anyhow!(
                "initialized completion action journal partition is missing or legacy"
            )));
        };
        directory.verify_link().map_err(action_uncertain)?;
        Ok(journal)
    }

    fn initialize_selected_completion_action_journal(
        &self,
        directory: &super::secure_fs::SecureDirectory,
        selector: &CompletionActionJournalSelector,
    ) -> Result<(), super::CompletionActionJournalStoreError> {
        match self
            .load_completion_action_journal_partition(directory, selector)
            .map_err(action_not_published)?
        {
            CompletionActionJournalRead::Missing => {
                let journal = super::CompletionActionJournal::new(
                    selector.campaign_id.clone(),
                    selector.epoch_id.clone(),
                    selector.policy_digest.clone(),
                );
                self.publish_completion_action_journal(directory, &journal)?;
            }
            CompletionActionJournalRead::Current(journal) if journal.actions().is_empty() => {}
            CompletionActionJournalRead::Current(_) => {
                return Err(action_not_published(anyhow!(
                    "completion action journal partition already contains actions"
                )));
            }
            CompletionActionJournalRead::LegacyV1(_) => {
                return Err(action_not_published(anyhow!(
                    CompletionActionJournalError::LegacyReadOnly
                )));
            }
        }
        self.publish_active_journal_selector(directory, selector)
    }

    fn publish_active_journal_selector(
        &self,
        directory: &super::secure_fs::SecureDirectory,
        selector: &CompletionActionJournalSelector,
    ) -> Result<(), super::CompletionActionJournalStoreError> {
        let mut bytes = serde_json::to_vec_pretty(selector)
            .context("failed to serialize completion action journal selector")
            .map_err(action_not_published)?;
        bytes.push(b'\n');
        crate::atomic_state_write::publish_bytes_in(
            directory.file(),
            Some(directory.parent()),
            super::action_journal_store::action_journal_name(),
            &super::action_journal_store::completion_action_journal_selector_path(self),
            &bytes,
        )
        .map_err(super::action_journal_store::map_action_publish_error)
    }

    fn require_current_transition_scope(
        &self,
        directory: &super::secure_fs::SecureDirectory,
        requested: &CompletionActionJournalSelector,
    ) -> anyhow::Result<()> {
        let ledger = self.load_from_directory(directory)?;
        let campaign = ledger
            .entries()
            .iter()
            .find_map(|entry| match entry.event() {
                super::ConvergenceEvent::CampaignStarted(record)
                    if entry.campaign_id() == &requested.campaign_id =>
                {
                    Some(record)
                }
                _ => None,
            })
            .context("completion journal transition campaign is missing")?;
        let current_epoch = ledger
            .entries()
            .iter()
            .rev()
            .find_map(|entry| match entry.event() {
                super::ConvergenceEvent::EpochOpened(epoch)
                    if entry.campaign_id() == &requested.campaign_id =>
                {
                    Some(epoch)
                }
                _ => None,
            })
            .context("completion journal transition epoch is missing")?;
        if current_epoch.id() != &requested.epoch_id
            || campaign.policy_digest() != Some(&requested.policy_digest)
        {
            anyhow::bail!(
                "completion journal transition does not target the current campaign epoch policy"
            );
        }
        Ok(())
    }
}
