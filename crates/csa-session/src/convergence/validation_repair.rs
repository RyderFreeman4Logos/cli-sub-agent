use std::collections::HashMap;

use anyhow::{Context, Result, bail};

use super::{
    CampaignState, RepairBatchState, RootClusterState, campaign_state, require_finalized_attempt,
};
use crate::convergence::{
    CampaignId, ConvergenceLedgerEntry, RepairBatchRecord, RepairHandoffRecord, RootClusterRecord,
};

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
    }
    if state
        .root_clusters
        .insert(
            record.id().clone(),
            RootClusterState {
                epoch_id: record.epoch_id().clone(),
                candidate_set_digest: record.candidate_set_digest().clone(),
                disposition_set_digest: record.disposition_set_digest().clone(),
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
        || record.candidate_set_digest() != &cluster.candidate_set_digest
        || record.disposition_set_digest() != &cluster.disposition_set_digest
    {
        bail!(
            "repair batch {} does not preserve its root cluster immutable union in campaign {}",
            record.id(),
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
    {
        bail!(
            "repair handoff {} does not preserve its repair batch immutable union in campaign {}",
            record.id(),
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
