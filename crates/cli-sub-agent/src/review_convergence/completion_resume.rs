use std::collections::HashSet;

use csa_session::convergence::{
    CampaignId, CandidateId, CompletionActionJournalRead, ConvergenceEvent, ConvergenceLedger,
    EpochId, ProviderTurnExecutionState, RootClusterId, Sha256Digest,
};

use super::completion::{
    AuthorizedRepairBatch, CompletionAction, CompletionBudget, CompletionError, CompletionEvent,
    CompletionPhase, CompletionStart, CompletionState, CompletionTransition, issue,
    reduce_completion, require_bounded_set, require_unique, validate_cluster_batches,
};
use super::completion_types::ClusteredCompletionStart;

impl CompletionStart {
    /// Derive an exact clustered checkpoint from one persisted campaign.
    ///
    /// The CLI never accepts candidate, cluster, or repair identifiers from its input. Instead,
    /// it reconstructs the claim from the durable ledger and then sends that claim through the
    /// same strict validator used by recovery and unit tests.
    pub(crate) fn from_persisted_clustered_campaign(
        ledger: &ConvergenceLedger,
        action_journal: &CompletionActionJournalRead,
        campaign_id: CampaignId,
    ) -> Result<Self, CompletionError> {
        ledger
            .validate()
            .map_err(|_| CompletionError::IdentityMismatch)?;
        let campaign = ledger
            .entries()
            .iter()
            .filter(|entry| entry.campaign_id() == &campaign_id)
            .find_map(|entry| match entry.event() {
                ConvergenceEvent::CampaignStarted(campaign) => Some(campaign),
                _ => None,
            })
            .ok_or(CompletionError::IdentityMismatch)?;
        let policy_digest = campaign
            .policy_digest()
            .cloned()
            .ok_or(CompletionError::PolicyDigestMismatch)?;
        let epoch = ledger
            .entries()
            .iter()
            .filter(|entry| entry.campaign_id() == &campaign_id)
            .filter_map(|entry| match entry.event() {
                ConvergenceEvent::EpochOpened(epoch) => Some(epoch),
                _ => None,
            })
            .next_back()
            .cloned()
            .ok_or(CompletionError::IdentityMismatch)?;
        let discovery_attempts = ledger
            .entries()
            .iter()
            .filter(|entry| entry.campaign_id() == &campaign_id)
            .filter_map(|entry| match entry.event() {
                ConvergenceEvent::DiscoveryAttemptRecorded(attempt)
                    if attempt.epoch_id() == epoch.id() =>
                {
                    Some(attempt.id().clone())
                }
                _ => None,
            })
            .collect::<HashSet<_>>();
        let candidate_ids = ledger
            .entries()
            .iter()
            .filter(|entry| entry.campaign_id() == &campaign_id)
            .filter_map(|entry| match entry.event() {
                ConvergenceEvent::CandidateRecorded(candidate)
                    if discovery_attempts.contains(candidate.discovery_attempt_id()) =>
                {
                    Some(candidate.id().clone())
                }
                _ => None,
            })
            .collect();
        let root_cluster_ids = ledger
            .entries()
            .iter()
            .filter(|entry| entry.campaign_id() == &campaign_id)
            .filter_map(|entry| match entry.event() {
                ConvergenceEvent::RootClusterRecorded(cluster)
                    if cluster.epoch_id() == epoch.id() =>
                {
                    Some(cluster.id().clone())
                }
                _ => None,
            })
            .collect();
        let repair_batches = ledger
            .entries()
            .iter()
            .filter(|entry| entry.campaign_id() == &campaign_id)
            .filter_map(|entry| match entry.event() {
                ConvergenceEvent::RepairBatchRecorded(batch) if batch.epoch_id() == epoch.id() => {
                    Some(AuthorizedRepairBatch::new(
                        batch.root_cluster_id().clone(),
                        batch.id().clone(),
                    ))
                }
                _ => None,
            })
            .collect();
        let ledger_generation = ledger.entries().last().map_or(
            0,
            csa_session::convergence::ConvergenceLedgerEntry::sequence,
        );
        let (cycles, provider_turns) =
            durable_resume_usage(action_journal, &campaign_id, epoch.id(), &policy_digest)?;
        Self::clustered(
            ledger,
            super::completion_types::ClusteredCompletionClaim {
                campaign_id,
                epoch,
                candidate_ids,
                root_cluster_ids,
                repair_batches,
                cycles,
                provider_turns,
                ledger_generation,
                policy_digest,
            },
        )
    }

    /// Restore an executable clustered start only when its checkpoint exactly matches the ledger.
    ///
    /// The ledger is validated before checking the caller-provided checkpoint, so an otherwise
    /// well-formed claim cannot revive an incomplete, stale, or cross-campaign cluster set.
    pub(crate) fn clustered(
        ledger: &ConvergenceLedger,
        claim: super::completion_types::ClusteredCompletionClaim,
    ) -> Result<Self, CompletionError> {
        ledger
            .validate()
            .map_err(|_| CompletionError::IdentityMismatch)?;
        let ledger_generation = ledger.entries().last().map_or(
            0,
            csa_session::convergence::ConvergenceLedgerEntry::sequence,
        );
        if claim.ledger_generation != ledger_generation {
            return Err(CompletionError::StaleLedgerGeneration);
        }

        let campaign = ledger
            .entries()
            .iter()
            .filter(|entry| entry.campaign_id() == &claim.campaign_id)
            .find_map(|entry| match entry.event() {
                ConvergenceEvent::CampaignStarted(campaign) => Some(campaign),
                _ => None,
            })
            .ok_or(CompletionError::IdentityMismatch)?;
        let policy_digest = campaign
            .policy_digest()
            .ok_or(CompletionError::PolicyDigestMismatch)?;
        if policy_digest != &claim.policy_digest {
            return Err(CompletionError::PolicyDigestMismatch);
        }
        let epoch = ledger
            .entries()
            .iter()
            .filter(|entry| entry.campaign_id() == &claim.campaign_id)
            .filter_map(|entry| match entry.event() {
                ConvergenceEvent::EpochOpened(epoch) => Some(epoch),
                _ => None,
            })
            .next_back()
            .ok_or(CompletionError::IdentityMismatch)?;
        if epoch != &claim.epoch {
            return Err(CompletionError::IdentityMismatch);
        }

        let discovery_attempts = ledger
            .entries()
            .iter()
            .filter(|entry| entry.campaign_id() == &claim.campaign_id)
            .filter_map(|entry| match entry.event() {
                ConvergenceEvent::DiscoveryAttemptRecorded(attempt)
                    if attempt.epoch_id() == epoch.id() =>
                {
                    Some(attempt.id().clone())
                }
                _ => None,
            })
            .collect::<HashSet<_>>();
        let candidate_ids = ledger
            .entries()
            .iter()
            .filter(|entry| entry.campaign_id() == &claim.campaign_id)
            .filter_map(|entry| match entry.event() {
                ConvergenceEvent::CandidateRecorded(candidate)
                    if discovery_attempts.contains(candidate.discovery_attempt_id()) =>
                {
                    Some(candidate.id().clone())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let root_cluster_ids = ledger
            .entries()
            .iter()
            .filter(|entry| entry.campaign_id() == &claim.campaign_id)
            .filter_map(|entry| match entry.event() {
                ConvergenceEvent::RootClusterRecorded(cluster)
                    if cluster.epoch_id() == epoch.id() =>
                {
                    Some(cluster.id().clone())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let repair_batches = ledger
            .entries()
            .iter()
            .filter(|entry| entry.campaign_id() == &claim.campaign_id)
            .filter_map(|entry| match entry.event() {
                ConvergenceEvent::RepairBatchRecorded(batch) if batch.epoch_id() == epoch.id() => {
                    Some(AuthorizedRepairBatch::new(
                        batch.root_cluster_id().clone(),
                        batch.id().clone(),
                    ))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let handed_off_batches = ledger
            .entries()
            .iter()
            .filter(|entry| entry.campaign_id() == &claim.campaign_id)
            .filter_map(|entry| match entry.event() {
                ConvergenceEvent::RepairHandoffRecorded(handoff)
                    if handoff.epoch_id() == epoch.id() =>
                {
                    Some(handoff.repair_batch_id())
                }
                _ => None,
            })
            .collect::<HashSet<_>>();
        if repair_batches
            .iter()
            .any(|batch| handed_off_batches.contains(batch.repair_batch_id()))
        {
            return Err(CompletionError::InvalidTransition(
                "clustered repair batch is already handed off",
            ));
        }

        let candidate_ids = canonical_candidates(&candidate_ids)?;
        let claimed_candidates = canonical_candidates(&claim.candidate_ids)?;
        require_same_set(&claimed_candidates, &candidate_ids)?;
        validate_cluster_batches(&claim.root_cluster_ids, &claim.repair_batches)?;
        let root_cluster_ids = canonical_root_clusters(&root_cluster_ids)?;
        let claimed_root_clusters = canonical_root_clusters(&claim.root_cluster_ids)?;
        require_same_set(&claimed_root_clusters, &root_cluster_ids)?;
        validate_cluster_batches(&root_cluster_ids, &repair_batches)?;
        let repair_batches = canonical_repair_batches(&repair_batches)?;
        let claimed_batches = canonical_repair_batches(&claim.repair_batches)?;
        require_same_set(&claimed_batches, &repair_batches)?;

        Ok(Self::Clustered(ClusteredCompletionStart {
            campaign_id: claim.campaign_id,
            epoch: claim.epoch,
            candidate_ids,
            root_cluster_ids,
            repair_batches,
            cycles: claim.cycles,
            provider_turns: claim.provider_turns,
            ledger_generation,
            policy_digest: claim.policy_digest,
        }))
    }
}

/// Reconstruct the completion budget already consumed by one exact durable journal.
///
/// A completed action consumes one reducer cycle. Provider budget consumption is only the sum
/// of host-observed reconciliations; reservations with uncertain usage cannot be resumed.
fn durable_resume_usage(
    action_journal: &CompletionActionJournalRead,
    campaign_id: &CampaignId,
    epoch_id: &EpochId,
    policy_digest: &Sha256Digest,
) -> Result<(u32, u32), CompletionError> {
    let CompletionActionJournalRead::Current(journal) = action_journal else {
        return match action_journal {
            CompletionActionJournalRead::Missing => Ok((0, 0)),
            CompletionActionJournalRead::LegacyV1(_) => Err(CompletionError::UsageIndeterminate),
            CompletionActionJournalRead::Current(_) => unreachable!("matched current journal"),
        };
    };
    if journal.campaign_id() != campaign_id || journal.epoch_id() != epoch_id {
        return Err(CompletionError::IdentityMismatch);
    }
    if journal.policy_digest() != policy_digest {
        return Err(CompletionError::PolicyDigestMismatch);
    }
    let cycles =
        u32::try_from(journal.actions().len()).map_err(|_| CompletionError::BudgetExhausted)?;
    let provider_turns = journal
        .actions()
        .iter()
        .flat_map(|action| action.provider_turns())
        .try_fold(0_u32, |total, execution| match execution.state() {
            ProviderTurnExecutionState::Reconciled {
                observed_turn_delta,
            } => total
                .checked_add(observed_turn_delta)
                .ok_or(CompletionError::BudgetExhausted),
            ProviderTurnExecutionState::ReleasedBeforeSend => Ok(total),
            ProviderTurnExecutionState::Reserved
            | ProviderTurnExecutionState::UsageIndeterminate => {
                Err(CompletionError::UsageIndeterminate)
            }
        })?;
    if !journal.permits_attestation() {
        return Err(CompletionError::InvalidTransition(
            "completion action journal has an unfinished action",
        ));
    }
    Ok((cycles, provider_turns))
}

/// Create the first completion action without replaying discovery or clustering for a resume.
pub(crate) fn start_completion(
    budget: CompletionBudget,
    start: CompletionStart,
) -> Result<CompletionTransition, CompletionError> {
    match start {
        CompletionStart::Fresh {
            initial_epoch,
            selection,
        } => reduce_completion(
            &CompletionState::new(budget),
            CompletionEvent::Started {
                epoch: initial_epoch,
                selection,
            },
        ),
        CompletionStart::Clustered(resume) => {
            if resume.cycles > budget.max_cycles
                || resume.provider_turns >= budget.max_provider_turns
            {
                return Err(CompletionError::BudgetExhausted);
            }
            let state = CompletionState {
                phase: if resume.repair_batches.is_empty() {
                    CompletionPhase::RunFinalGates
                } else {
                    CompletionPhase::RunAuthorizedRepairs
                },
                budget,
                cycles: resume.cycles,
                provider_turns: resume.provider_turns,
                reconciled_execution_ids: Vec::new(),
                campaign_id: Some(resume.campaign_id.clone()),
                epoch: Some(resume.epoch.clone()),
                discovery_focus: None,
                campaign_selection: None,
                pending_candidates: Vec::new(),
                clustered_candidates: resume.candidate_ids,
                root_clusters: resume.root_cluster_ids,
                repair_batches: resume.repair_batches.clone(),
                ledger_generation: Some(resume.ledger_generation),
                policy_digest: Some(resume.policy_digest),
                gate_artifact: None,
                clean_room: None,
            };
            let action = match state.phase {
                CompletionPhase::RunAuthorizedRepairs => CompletionAction::RunAuthorizedRepairs {
                    campaign_id: resume.campaign_id,
                    epoch: resume.epoch,
                    batches: resume.repair_batches,
                },
                CompletionPhase::RunFinalGates => CompletionAction::RunFinalGates {
                    campaign_id: resume.campaign_id,
                    epoch: resume.epoch,
                },
                _ => unreachable!("clustered completion start only has two reachable phases"),
            };
            issue(state, action)
        }
    }
}

fn canonical_candidates(values: &[CandidateId]) -> Result<Vec<CandidateId>, CompletionError> {
    require_bounded_set(values.len())?;
    require_unique(values.iter().map(CandidateId::as_str))?;
    let mut canonical = values.to_vec();
    canonical.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    Ok(canonical)
}

fn canonical_root_clusters(
    values: &[RootClusterId],
) -> Result<Vec<RootClusterId>, CompletionError> {
    require_bounded_set(values.len())?;
    require_unique(values.iter().map(RootClusterId::as_str))?;
    let mut canonical = values.to_vec();
    canonical.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    Ok(canonical)
}

fn canonical_repair_batches(
    values: &[AuthorizedRepairBatch],
) -> Result<Vec<AuthorizedRepairBatch>, CompletionError> {
    require_bounded_set(values.len())?;
    require_unique(values.iter().map(|batch| batch.repair_batch_id.as_str()))?;
    require_unique(values.iter().map(|batch| batch.root_cluster_id.as_str()))?;
    let mut canonical = values.to_vec();
    canonical.sort_by(|left, right| {
        left.root_cluster_id
            .as_str()
            .cmp(right.root_cluster_id.as_str())
            .then_with(|| {
                left.repair_batch_id
                    .as_str()
                    .cmp(right.repair_batch_id.as_str())
            })
    });
    Ok(canonical)
}

fn require_same_set<T: PartialEq>(expected: &[T], actual: &[T]) -> Result<(), CompletionError> {
    if expected.len() != actual.len() {
        return Err(CompletionError::CardinalityMismatch);
    }
    if expected != actual {
        return Err(CompletionError::IdentityMismatch);
    }
    Ok(())
}
