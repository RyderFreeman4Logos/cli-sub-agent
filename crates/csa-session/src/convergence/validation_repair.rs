use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, bail};

use super::{
    CampaignState, RepairBatchState, RootClusterState, campaign_state, require_finalized_attempt,
};
use crate::convergence::{
    CampaignId, CandidateDisposition, CandidateDispositionRecord, ConvergenceLedgerEntry,
    RepairBatchRecord, RepairHandoffRecord, RootClusterRecord,
};

pub(super) fn validate_complete_clustering(
    campaign_id: &CampaignId,
    state: &CampaignState,
) -> Result<()> {
    if state.root_clusters.is_empty() && state.repair_batches.is_empty() {
        return Ok(());
    }
    let clustered_epochs = state
        .root_clusters
        .values()
        .map(|cluster| &cluster.epoch_id)
        .collect::<HashSet<_>>();
    let blocking_candidates = state
        .dispositions
        .iter()
        .filter_map(|(candidate_id, record)| {
            (clustered_epochs.contains(record.epoch_id())
                && matches!(
                    record.disposition(),
                    CandidateDisposition::Verified
                        | CandidateDisposition::NeedsContractOrDocumentation
                ))
            .then_some(candidate_id)
        })
        .collect::<HashSet<_>>();
    let clustered_candidates = state
        .clustered_blocking_candidates
        .iter()
        .filter(|candidate_id| {
            state
                .dispositions
                .get(*candidate_id)
                .is_some_and(|record| clustered_epochs.contains(record.epoch_id()))
        })
        .collect::<HashSet<_>>();
    if clustered_candidates != blocking_candidates {
        bail!(
            "root clustering for campaign {campaign_id} does not cover every verified blocking candidate exactly once"
        );
    }
    if state.root_clusters.len() != state.repair_batches_by_cluster.len()
        || state
            .root_clusters
            .keys()
            .any(|cluster_id| !state.repair_batches_by_cluster.contains_key(cluster_id))
    {
        bail!(
            "root clustering for campaign {campaign_id} does not provide exactly one consolidated repair batch per cluster"
        );
    }
    Ok(())
}

pub(super) fn record_root_cluster(
    campaigns: &mut HashMap<CampaignId, CampaignState>,
    entry: &ConvergenceLedgerEntry,
    record: &RootClusterRecord,
) -> Result<()> {
    let state = campaign_state(campaigns, entry, "root cluster")?;
    record.validate().with_context(|| {
        format!(
            "invalid root cluster {} in campaign {}",
            record.id(),
            entry.campaign_id()
        )
    })?;
    if !state.epochs.contains(record.epoch_id()) {
        bail!(
            "root cluster {} references unopened epoch {} in campaign {}",
            record.id(),
            record.epoch_id(),
            entry.campaign_id()
        );
    }
    let mut dispositions = Vec::with_capacity(record.candidate_ids().len());
    for candidate_id in record.candidate_ids() {
        let candidate = state.candidates.get(candidate_id).with_context(|| {
            format!(
                "root cluster {} references unknown candidate {} in campaign {}",
                record.id(),
                candidate_id,
                entry.campaign_id()
            )
        })?;
        require_finalized_attempt(state, candidate_id, candidate, entry.campaign_id())?;
        if !state.disposed_candidates.contains(candidate_id) {
            bail!(
                "root cluster {} references candidate {} without a terminal disposition in campaign {}",
                record.id(),
                candidate_id,
                entry.campaign_id()
            );
        }
        let disposition = state.dispositions.get(candidate_id).with_context(|| {
            format!(
                "root cluster {} lost terminal disposition for candidate {} in campaign {}",
                record.id(),
                candidate_id,
                entry.campaign_id()
            )
        })?;
        if !matches!(
            disposition.disposition(),
            CandidateDisposition::Verified | CandidateDisposition::NeedsContractOrDocumentation
        ) {
            bail!(
                "root cluster {} contains nonblocking candidate {} in campaign {}",
                record.id(),
                candidate_id,
                entry.campaign_id()
            );
        }
        dispositions.push(disposition.clone());
    }
    if record
        .candidate_ids()
        .iter()
        .any(|candidate_id| state.clustered_blocking_candidates.contains(candidate_id))
    {
        bail!(
            "root cluster {} overlaps an already clustered blocking candidate in campaign {}",
            record.id(),
            entry.campaign_id()
        );
    }
    if CandidateDispositionRecord::set_digest(&dispositions) != *record.disposition_set_digest() {
        bail!(
            "root cluster {} disposition set digest does not bind its complete candidates in campaign {}",
            record.id(),
            entry.campaign_id()
        );
    }
    if state
        .root_clusters
        .insert(
            record.id().clone(),
            RootClusterState {
                epoch_id: record.epoch_id().clone(),
                candidate_ids: record.candidate_ids().to_vec(),
                candidate_set_digest: record.candidate_set_digest().clone(),
                disposition_set_digest: record.disposition_set_digest().clone(),
                content_digest: record.content_digest().clone(),
            },
        )
        .is_some()
    {
        bail!(
            "duplicate root cluster {} in campaign {}",
            record.id(),
            entry.campaign_id()
        );
    }
    state
        .clustered_blocking_candidates
        .extend(record.candidate_ids().iter().cloned());
    state.root_cluster_records.push(record.clone());
    Ok(())
}

pub(super) fn record_repair_batch(
    campaigns: &mut HashMap<CampaignId, CampaignState>,
    entry: &ConvergenceLedgerEntry,
    record: &RepairBatchRecord,
) -> Result<()> {
    let state = campaign_state(campaigns, entry, "repair batch")?;
    record.validate().with_context(|| {
        format!(
            "invalid repair batch {} in campaign {}",
            record.id(),
            entry.campaign_id()
        )
    })?;
    let cluster = state
        .root_clusters
        .get(record.root_cluster_id())
        .with_context(|| {
            format!(
                "repair batch {} references unknown root cluster {} in campaign {}",
                record.id(),
                record.root_cluster_id(),
                entry.campaign_id()
            )
        })?;
    if record.epoch_id() != &cluster.epoch_id
        || record.candidate_ids() != cluster.candidate_ids.as_slice()
        || record.candidate_set_digest() != &cluster.candidate_set_digest
        || record.disposition_set_digest() != &cluster.disposition_set_digest
        || record.root_cluster_content_digest() != &cluster.content_digest
    {
        bail!(
            "repair batch {} does not preserve its root cluster immutable union in campaign {}",
            record.id(),
            entry.campaign_id()
        );
    }
    if state
        .repair_batches_by_cluster
        .contains_key(record.root_cluster_id())
    {
        bail!(
            "root cluster {} already has a consolidated repair batch in campaign {}",
            record.root_cluster_id(),
            entry.campaign_id()
        );
    }
    if state
        .repair_batches
        .insert(
            record.id().clone(),
            RepairBatchState {
                epoch_id: record.epoch_id().clone(),
                candidate_set_digest: record.candidate_set_digest().clone(),
                disposition_set_digest: record.disposition_set_digest().clone(),
                content_digest: record.content_digest().clone(),
            },
        )
        .is_some()
    {
        bail!(
            "duplicate repair batch {} in campaign {}",
            record.id(),
            entry.campaign_id()
        );
    }
    state
        .repair_batches_by_cluster
        .insert(record.root_cluster_id().clone(), record.id().clone());
    state.repair_batch_records.push(record.clone());
    Ok(())
}

pub(super) fn record_repair_handoff(
    campaigns: &mut HashMap<CampaignId, CampaignState>,
    entry: &ConvergenceLedgerEntry,
    record: &RepairHandoffRecord,
) -> Result<()> {
    let state = campaign_state(campaigns, entry, "repair handoff")?;
    record.validate().with_context(|| {
        format!(
            "invalid repair handoff {} in campaign {}",
            record.id(),
            entry.campaign_id()
        )
    })?;
    if record.campaign_id() != entry.campaign_id() {
        bail!(
            "repair handoff {} campaign id does not match entry campaign {}",
            record.id(),
            entry.campaign_id()
        );
    }
    let batch = state
        .repair_batches
        .get(record.repair_batch_id())
        .with_context(|| {
            format!(
                "repair handoff {} references unknown batch {} in campaign {}",
                record.id(),
                record.repair_batch_id(),
                entry.campaign_id()
            )
        })?;
    if record.epoch_id() != &batch.epoch_id
        || record.candidate_set_digest() != &batch.candidate_set_digest
        || record.disposition_set_digest() != &batch.disposition_set_digest
        || record.repair_batch_content_digest() != &batch.content_digest
    {
        bail!(
            "repair handoff {} does not preserve its repair batch immutable union in campaign {}",
            record.id(),
            entry.campaign_id()
        );
    }
    if record.command_authority_digest() != &state.command_authority.digest()
        || !state.command_authority.contains(record.actual_executor())
    {
        bail!(
            "repair handoff {} actual executor or command authority does not match campaign {}",
            record.id(),
            entry.campaign_id()
        );
    }
    let clusters = state
        .root_cluster_records
        .iter()
        .filter(|cluster| cluster.epoch_id() == record.epoch_id())
        .cloned()
        .collect::<Vec<_>>();
    let batches = state
        .repair_batch_records
        .iter()
        .filter(|candidate| candidate.epoch_id() == record.epoch_id())
        .cloned()
        .collect::<Vec<_>>();
    if record.cluster_set_digest() != &RootClusterRecord::set_digest(&clusters)
        || record.repair_batch_set_digest() != &RepairBatchRecord::set_digest(&batches)
    {
        bail!(
            "repair handoff {} does not bind the complete cluster and batch sets in campaign {}",
            record.id(),
            entry.campaign_id()
        );
    }
    if !state
        .repair_handoffs_by_batch
        .insert(record.repair_batch_id().clone())
    {
        bail!(
            "repair batch {} already has a writer handoff in campaign {}",
            record.repair_batch_id(),
            entry.campaign_id()
        );
    }
    if !state.repair_handoffs.insert(record.id().clone()) {
        bail!(
            "duplicate repair handoff {} in campaign {}",
            record.id(),
            entry.campaign_id()
        );
    }
    Ok(())
}
