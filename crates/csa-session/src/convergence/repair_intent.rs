//! Durable intent for a repair that crosses the source-repository and ledger boundary.
//!
//! A started intent is deliberately not a success record. Recovery may promote it only after
//! independently proving the matching epoch commit in the ledger and source repository; every
//! other combination is uncertain and blocks completion.

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use super::{CompletionActionClaim, EpochRecord, RepairBatchId, Sha256Digest};

/// The only repair-intent schema written by this binary.
pub const REPAIR_INTENT_SCHEMA_VERSION: u32 = 1;
/// Bound the exact batch set retained in one repair intent.
pub const MAX_REPAIR_INTENT_BATCHES: usize = 1_000;

/// Persisted recovery state for a source-repository repair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum RepairIntentState {
    /// The intent was durable before repair execution began; success is not yet known.
    Started,
    /// The source and ledger were both verified at the observed changed epoch.
    Committed { observed_epoch: EpochRecord },
    /// Recovery could not prove one safe outcome, so completion must stop.
    Uncertain,
}

/// Immutable repair authority recorded before a writer may mutate the source repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepairIntent {
    schema_version: u32,
    claim: CompletionActionClaim,
    expected_epoch: EpochRecord,
    repair_batch_set_digest: Sha256Digest,
    repair_batch_ids: Vec<RepairBatchId>,
    state: RepairIntentState,
}

impl RepairIntent {
    /// Bind one current action claim to its complete, ledger-authorized repair batch set.
    ///
    /// # Errors
    ///
    /// Returns an error if the claim and expected epoch differ, or the batch IDs are empty,
    /// duplicated, or exceed the bounded recovery contract.
    pub fn new(
        claim: CompletionActionClaim,
        expected_epoch: EpochRecord,
        repair_batch_set_digest: Sha256Digest,
        repair_batch_ids: Vec<RepairBatchId>,
    ) -> Result<Self> {
        let repair_batch_ids = canonical_batch_ids(repair_batch_ids)?;
        if claim.epoch_id() != expected_epoch.id() {
            bail!("repair intent claim epoch does not match the expected epoch");
        }
        let intent = Self {
            schema_version: REPAIR_INTENT_SCHEMA_VERSION,
            claim,
            expected_epoch,
            repair_batch_set_digest,
            repair_batch_ids,
            state: RepairIntentState::Started,
        };
        intent.validate()?;
        Ok(intent)
    }

    /// Verify immutable bindings and the persisted recovery state.
    ///
    /// # Errors
    ///
    /// Returns an error when deserialized state is malformed, stale, or internally inconsistent.
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != REPAIR_INTENT_SCHEMA_VERSION {
            bail!(
                "unsupported repair intent schema version {}; expected {REPAIR_INTENT_SCHEMA_VERSION}",
                self.schema_version
            );
        }
        self.expected_epoch.validate()?;
        if self.claim.epoch_id() != self.expected_epoch.id() {
            bail!("repair intent claim epoch does not match the expected epoch");
        }
        if self.repair_batch_ids.is_empty() {
            bail!("repair intent requires at least one authorized repair batch");
        }
        if self.repair_batch_ids.len() > MAX_REPAIR_INTENT_BATCHES {
            bail!("repair intent contains more than {MAX_REPAIR_INTENT_BATCHES} repair batch IDs");
        }
        let canonical = canonical_batch_ids(self.repair_batch_ids.clone())?;
        if canonical != self.repair_batch_ids {
            bail!("repair intent batch IDs are not canonical");
        }
        if let RepairIntentState::Committed { observed_epoch } = &self.state {
            observed_epoch.validate()?;
            if observed_epoch.base_oid() != self.expected_epoch.base_oid()
                || observed_epoch.head_oid() == self.expected_epoch.head_oid()
            {
                bail!("committed repair intent does not describe a changed expected epoch");
            }
        }
        Ok(())
    }

    /// Return the exact fenced completion action that owns this repair.
    #[must_use]
    pub fn claim(&self) -> &CompletionActionClaim {
        &self.claim
    }

    /// Return the immutable source epoch required before repair execution begins.
    #[must_use]
    pub fn expected_epoch(&self) -> &EpochRecord {
        &self.expected_epoch
    }

    /// Return the expected source HEAD object ID.
    #[must_use]
    pub fn expected_commit(&self) -> &super::GitObjectId {
        self.expected_epoch.head_oid()
    }

    /// Return the complete content-addressed repair-batch set binding.
    #[must_use]
    pub fn repair_batch_set_digest(&self) -> &Sha256Digest {
        &self.repair_batch_set_digest
    }

    /// Return the canonical exact set of authorized repair batch IDs.
    #[must_use]
    pub fn repair_batch_ids(&self) -> &[RepairBatchId] {
        &self.repair_batch_ids
    }

    /// Return the persisted recovery state.
    #[must_use]
    pub fn state(&self) -> &RepairIntentState {
        &self.state
    }

    /// Record a source and ledger epoch that independently prove this intent completed.
    ///
    /// # Errors
    ///
    /// Returns an error unless the intent is still started and the observed epoch is a changed
    /// descendant comparison against the same immutable base object.
    pub fn mark_committed(&mut self, observed_epoch: EpochRecord) -> Result<()> {
        if self.state != RepairIntentState::Started {
            bail!("only a started repair intent can be committed");
        }
        self.state = RepairIntentState::Committed { observed_epoch };
        self.validate()
    }

    /// Record that recovery cannot prove a safe source/ledger outcome.
    ///
    /// # Errors
    ///
    /// Returns an error unless the intent is still started.
    pub fn mark_uncertain(&mut self) -> Result<()> {
        if self.state != RepairIntentState::Started {
            bail!("only a started repair intent can become uncertain");
        }
        self.state = RepairIntentState::Uncertain;
        self.validate()
    }
}

fn canonical_batch_ids(mut values: Vec<RepairBatchId>) -> Result<Vec<RepairBatchId>> {
    if values.is_empty() {
        bail!("repair intent requires at least one authorized repair batch");
    }
    if values.len() > MAX_REPAIR_INTENT_BATCHES {
        bail!("repair intent contains more than {MAX_REPAIR_INTENT_BATCHES} repair batch IDs");
    }
    values.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    if values
        .windows(2)
        .any(|pair| pair[0].as_str() == pair[1].as_str())
    {
        bail!("repair intent contains a duplicate repair batch ID");
    }
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::convergence::{
        CampaignId, CompletionActionId, CompletionActionJournal, GitObjectId,
    };

    fn epoch(head: char) -> EpochRecord {
        EpochRecord::new(
            GitObjectId::parse("1111111111111111111111111111111111111111").unwrap(),
            GitObjectId::parse(&head.to_string().repeat(40)).unwrap(),
            Sha256Digest::compute(b"diff"),
        )
    }

    fn claim(expected: &EpochRecord) -> CompletionActionClaim {
        let mut journal = CompletionActionJournal::new(
            CampaignId::generate(),
            expected.id().clone(),
            Sha256Digest::compute(b"policy"),
        );
        journal
            .claim_next(0, CompletionActionId::generate())
            .unwrap()
    }

    #[test]
    fn repair_intent_binds_exact_canonical_batch_set_to_claim_and_expected_commit() {
        let expected = epoch('2');
        let first = RepairBatchId::generate();
        let second = RepairBatchId::generate();
        let intent = RepairIntent::new(
            claim(&expected),
            expected.clone(),
            Sha256Digest::compute(b"batch-set"),
            vec![second.clone(), first.clone()],
        )
        .unwrap();

        assert_eq!(intent.expected_commit(), expected.head_oid());
        assert_eq!(intent.claim().epoch_id(), expected.id());
        assert_eq!(intent.state(), &RepairIntentState::Started);
        assert!(
            intent
                .repair_batch_ids()
                .windows(2)
                .all(|pair| { pair[0].as_str() < pair[1].as_str() })
        );
    }

    #[test]
    fn repair_intent_only_commits_a_changed_epoch_from_the_same_base() {
        let expected = epoch('2');
        let mut intent = RepairIntent::new(
            claim(&expected),
            expected.clone(),
            Sha256Digest::compute(b"batch-set"),
            vec![RepairBatchId::generate()],
        )
        .unwrap();

        intent.mark_committed(epoch('3')).unwrap();
        assert!(matches!(
            intent.state(),
            RepairIntentState::Committed { observed_epoch } if observed_epoch == &epoch('3')
        ));
        assert!(intent.mark_uncertain().is_err());
    }
}
