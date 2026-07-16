use std::collections::HashSet;

use csa_session::convergence::{
    CampaignId, CandidateId, ConvergenceEvent, ConvergenceLedger, RootClusterId,
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
        Self::clustered(
            ledger,
            super::completion_types::ClusteredCompletionClaim {
                campaign_id,
                epoch,
                candidate_ids,
                root_cluster_ids,
                repair_batches,
                cycles: 0,
                provider_turns: 0,
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
