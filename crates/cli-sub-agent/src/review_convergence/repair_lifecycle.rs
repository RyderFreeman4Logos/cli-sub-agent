//! Fenced repair-intent lifecycle and deterministic crash reconciliation.

use std::path::Path;

use anyhow::{Context, Result, bail};
use csa_session::convergence::{
    CampaignId, CompletionActionClaim, CompletionActionId, CompletionActionJournalRead,
    CompletionActionState, ConsolidatedRepairAuthorization, ConvergenceEvent, ConvergenceLedger,
    ConvergenceLedgerStore, EpochRecord, RepairBatchRecord, RepairIntent, RepairIntentRead,
    RepairIntentState,
};

use super::repair_source::capture_epoch;

/// Recover a started intent only when source and ledger independently prove the exact commit.
pub(super) fn reconcile_incomplete_repair_intent(
    store: &ConvergenceLedgerStore,
    project_root: &Path,
    campaign_id: &CampaignId,
) -> Result<()> {
    let CompletionActionJournalRead::Current(journal) = store.load_completion_action_journal()?
    else {
        return Ok(());
    };
    if journal.campaign_id() != campaign_id {
        bail!(
            "completion action journal belongs to campaign {}; refusing cross-campaign repair resume",
            journal.campaign_id()
        );
    }
    let Some(action) = journal.actions().last() else {
        return Ok(());
    };
    match action.state() {
        CompletionActionState::Finished => Ok(()),
        CompletionActionState::Uncertain => bail!(
            "completion action {} is uncertain; repair cannot resume",
            action.claim().action_id()
        ),
        CompletionActionState::Started => {
            let claim = action.claim();
            let intent = match store.load_repair_intent(claim) {
                Ok(RepairIntentRead::Current(intent)) => intent,
                Ok(RepairIntentRead::Missing) => {
                    store
                        .mark_completion_action_uncertain(claim)
                        .map_err(anyhow::Error::from)?;
                    bail!(
                        "started repair action {} has no durable repair intent",
                        claim.action_id()
                    );
                }
                Err(error) => {
                    store
                        .mark_completion_action_uncertain(claim)
                        .map_err(anyhow::Error::from)?;
                    return Err(error).context("started repair intent could not be read");
                }
            };
            let observed = capture_epoch(project_root, intent.expected_epoch().base_oid());
            let ledger = store.load();
            let committed = match (&observed, &ledger) {
                (Ok(observed), Ok(ledger)) if observed.clean => {
                    repair_intent_matches_committed_epoch(&intent, &observed.epoch, ledger)
                }
                _ => false,
            };
            if committed {
                if matches!(intent.state(), RepairIntentState::Started) {
                    store
                        .mark_repair_intent_committed(claim, observed?.epoch)
                        .map_err(anyhow::Error::from)?;
                }
                store
                    .finish_completion_action(claim)
                    .map_err(anyhow::Error::from)?;
                bail!(
                    "repair action {} was recovered from its committed epoch; restart from the new epoch",
                    claim.action_id()
                );
            }
            mark_repair_uncertain(store, claim)?;
            bail!(
                "repair action {} has an incomplete or drifting source/ledger combination",
                claim.action_id()
            );
        }
    }
}

/// Claim the next action only for the exact campaign, epoch, and policy of an authorization.
pub(super) fn claim_repair_action(
    store: &ConvergenceLedgerStore,
    authorization: &ConsolidatedRepairAuthorization,
) -> Result<CompletionActionClaim> {
    let policy_digest = authorization
        .campaign()
        .policy_digest()
        .context("repair campaign is missing its immutable completion policy digest")?
        .clone();
    let generation = match store.load_completion_action_journal()? {
        CompletionActionJournalRead::Missing => store
            .initialize_completion_action_journal(
                authorization.campaign().id().clone(),
                authorization.epoch().id().clone(),
                policy_digest.clone(),
            )
            .map_err(anyhow::Error::from)?
            .generation(),
        CompletionActionJournalRead::LegacyV1(_) => {
            bail!("legacy completion action journal cannot authorize repair")
        }
        CompletionActionJournalRead::Current(journal) => {
            if journal.campaign_id() != authorization.campaign().id()
                || journal.epoch_id() != authorization.epoch().id()
                || journal.policy_digest() != &policy_digest
            {
                bail!("completion action journal does not match the authorized repair epoch");
            }
            if !journal.permits_attestation() {
                bail!("completion action journal contains an unresolved action");
            }
            journal.generation()
        }
    };
    store
        .claim_completion_action(generation, CompletionActionId::generate())
        .map_err(anyhow::Error::from)
}

/// Construct the only exact repair selection that a writer is permitted to execute.
pub(super) fn repair_intent(
    authorization: &ConsolidatedRepairAuthorization,
    claim: CompletionActionClaim,
) -> Result<RepairIntent> {
    let batch_ids = authorization
        .batches()
        .iter()
        .map(|batch| batch.id().clone())
        .collect();
    RepairIntent::new(
        claim,
        authorization.epoch().clone(),
        RepairBatchRecord::set_digest(authorization.batches()),
        batch_ids,
    )
}

/// Reject every repair selection that is not an exact ledger-authorized batch set.
pub(super) fn validate_exact_repair_batches(
    authorization: &ConsolidatedRepairAuthorization,
    intent: &RepairIntent,
) -> Result<()> {
    if intent.claim().campaign_id() != authorization.campaign().id()
        || intent.expected_epoch() != authorization.epoch()
        || intent.repair_batch_set_digest()
            != &RepairBatchRecord::set_digest(authorization.batches())
    {
        bail!("repair intent does not match the ledger-authorized repair authority");
    }
    let mut authorized = authorization
        .batches()
        .iter()
        .map(|batch| batch.id().clone())
        .collect::<Vec<_>>();
    authorized.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    if authorized != intent.repair_batch_ids() {
        bail!("repair intent does not contain the exact ledger-authorized repair batch IDs");
    }
    Ok(())
}

fn repair_intent_matches_committed_epoch(
    intent: &RepairIntent,
    observed_epoch: &EpochRecord,
    ledger: &ConvergenceLedger,
) -> bool {
    if ledger.validate().is_err()
        || observed_epoch.base_oid() != intent.expected_epoch().base_oid()
        || observed_epoch.head_oid() == intent.expected_commit()
    {
        return false;
    }
    let entries = ledger
        .entries()
        .iter()
        .filter(|entry| entry.campaign_id() == intent.claim().campaign_id())
        .collect::<Vec<_>>();
    let Some(expected_epoch_position) = entries.iter().rposition(|entry| {
        matches!(entry.event(), ConvergenceEvent::EpochOpened(epoch) if epoch == intent.expected_epoch())
    }) else {
        return false;
    };
    let suffix = &entries[expected_epoch_position + 1..];
    if suffix.len() != intent.repair_batch_ids().len() + 1 {
        return false;
    }
    let Some((last, handoffs)) = suffix.split_last() else {
        return false;
    };
    if !matches!(last.event(), ConvergenceEvent::EpochOpened(epoch) if epoch == observed_epoch) {
        return false;
    }
    let mut completed = Vec::with_capacity(handoffs.len());
    for handoff in handoffs {
        let ConvergenceEvent::RepairHandoffRecorded(record) = handoff.event() else {
            return false;
        };
        if record.epoch_id() != intent.expected_epoch().id()
            || record.repair_batch_set_digest() != intent.repair_batch_set_digest()
        {
            return false;
        }
        completed.push(record.repair_batch_id().clone());
    }
    completed.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    completed == intent.repair_batch_ids()
}

/// Preserve the original failure while making the repair action terminally non-attestable.
pub(super) fn finish_failed_repair(
    store: &ConvergenceLedgerStore,
    claim: &CompletionActionClaim,
    error: anyhow::Error,
) -> Result<i32> {
    match mark_repair_uncertain(store, claim) {
        Ok(()) => Err(error),
        Err(recovery_error) => Err(error.context(format!(
            "repair recovery could not be marked uncertain: {recovery_error:#}"
        ))),
    }
}

fn mark_repair_uncertain(
    store: &ConvergenceLedgerStore,
    claim: &CompletionActionClaim,
) -> Result<()> {
    let intent_result = match store.load_repair_intent(claim) {
        Ok(RepairIntentRead::Current(intent))
            if matches!(intent.state(), RepairIntentState::Started) =>
        {
            store
                .mark_repair_intent_uncertain(claim)
                .map_err(anyhow::Error::from)
        }
        Ok(RepairIntentRead::Current(_)) | Ok(RepairIntentRead::Missing) => Ok(()),
        Err(error) => Err(error),
    };
    let action_result = store
        .mark_completion_action_uncertain(claim)
        .map_err(anyhow::Error::from);
    match (intent_result, action_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(intent_error), Ok(())) => Err(intent_error),
        (Ok(()), Err(action_error)) => Err(action_error),
        (Err(intent_error), Err(action_error)) => Err(intent_error.context(format!(
            "completion action could not be marked uncertain: {action_error:#}"
        ))),
    }
}
