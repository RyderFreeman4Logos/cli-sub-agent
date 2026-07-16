//! Immutable selector for the active partitioned completion action journal.

use std::ffi::OsString;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use super::{
    CampaignId, CompletionActionJournal, CompletionActionJournalRead, EpochId, Sha256Digest,
};

pub(super) const COMPLETION_ACTION_JOURNAL_SELECTOR_SCHEMA_VERSION: u32 = 1;

/// Atomic selector for the active exact campaign/epoch/policy journal partition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CompletionActionJournalSelector {
    pub(super) selector_schema_version: u32,
    pub(super) campaign_id: CampaignId,
    pub(super) epoch_id: EpochId,
    pub(super) policy_digest: Sha256Digest,
}

impl CompletionActionJournalSelector {
    pub(super) fn new(
        campaign_id: CampaignId,
        epoch_id: EpochId,
        policy_digest: Sha256Digest,
    ) -> Self {
        Self {
            selector_schema_version: COMPLETION_ACTION_JOURNAL_SELECTOR_SCHEMA_VERSION,
            campaign_id,
            epoch_id,
            policy_digest,
        }
    }

    pub(super) fn from_journal(journal: &CompletionActionJournal) -> Self {
        Self::new(
            journal.campaign_id().clone(),
            journal.epoch_id().clone(),
            journal.policy_digest().clone(),
        )
    }

    /// Validate the selector's schema before it controls journal resolution.
    pub(super) fn validate(&self) -> Result<()> {
        if self.selector_schema_version != COMPLETION_ACTION_JOURNAL_SELECTOR_SCHEMA_VERSION {
            bail!(
                "unsupported completion action journal selector schema {}; expected {}",
                self.selector_schema_version,
                COMPLETION_ACTION_JOURNAL_SELECTOR_SCHEMA_VERSION
            );
        }
        Ok(())
    }

    /// Return the deterministic file name for this exact journal partition.
    pub(super) fn journal_name(&self) -> OsString {
        let digest = self
            .policy_digest
            .as_str()
            .strip_prefix("sha256:")
            .expect("Sha256Digest always has the sha256 prefix");
        OsString::from(format!(
            "completion-actions-{}-{}-{digest}.json",
            self.campaign_id, self.epoch_id
        ))
    }
}

/// Result of resolving the active-journal selector file.
pub(super) enum ActiveJournalSelectorRead {
    /// No selector was published.
    Missing,
    /// A valid selector names the active exact partition.
    Current(CompletionActionJournalSelector),
    /// A legacy v1 journal occupies the selector path and remains read-only.
    Legacy(CompletionActionJournalRead),
}
