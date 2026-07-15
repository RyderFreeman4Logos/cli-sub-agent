use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, bail};

use super::{
    AdmittedModelIdentity, CONVERGENCE_LEDGER_SCHEMA_VERSION, CampaignId,
    CandidateDispositionRecord, CandidateId, CommandAuthoritySnapshot, ConvergenceEvent,
    ConvergenceLedgerEntry, CoverageCellId, CoverageRequirement, CsaSessionId, DiscoveryAttemptId,
    EpochId, LedgerEventId, RepairBatchId, RepairHandoffId, RootClusterId, StableFindingId,
};

#[path = "validation_disposition.rs"]
mod disposition_validation;
#[path = "validation_repair.rs"]
mod repair_validation;

/// Replay a convergence ledger and enforce all cross-event protocol invariants.
pub(super) fn validate_ledger(
    schema_version: u32,
    entries: &[ConvergenceLedgerEntry],
) -> Result<()> {
    if schema_version != CONVERGENCE_LEDGER_SCHEMA_VERSION {
        bail!(
            "unsupported convergence ledger schema version {schema_version}; expected {CONVERGENCE_LEDGER_SCHEMA_VERSION}"
        );
    }

    let mut event_ids = HashSet::<LedgerEventId>::new();
    let mut campaigns = HashMap::<CampaignId, CampaignState>::new();
    for (index, entry) in entries.iter().enumerate() {
        let expected_sequence = u64::try_from(index)
            .context("convergence ledger contains more entries than u64 can address")?
            .checked_add(1)
            .context("convergence ledger sequence overflow")?;
        if entry.sequence() != expected_sequence {
            bail!(
                "noncontiguous convergence ledger sequence: expected {expected_sequence}, got {}",
                entry.sequence()
            );
        }
        if !event_ids.insert(entry.event_id().clone()) {
            bail!("duplicate convergence ledger event id {}", entry.event_id());
        }
        apply_event(&mut campaigns, entry)?;
    }
    for (campaign_id, state) in &campaigns {
        repair_validation::validate_complete_clustering(campaign_id, state)?;
    }
    super::validation_attestation::validate_terminal_pair(entries)?;
    Ok(())
}

fn apply_event(
    campaigns: &mut HashMap<CampaignId, CampaignState>,
    entry: &ConvergenceLedgerEntry,
) -> Result<()> {
    match entry.event() {
        ConvergenceEvent::CampaignStarted(record) => {
            if record.id() != entry.campaign_id() {
                bail!(
                    "campaign start record id {} does not match entry campaign id {}",
                    record.id(),
                    entry.campaign_id()
                );
            }
            if campaigns
                .insert(
                    entry.campaign_id().clone(),
                    CampaignState::new(record.command_authority().clone()),
                )
                .is_some()
            {
                bail!("duplicate campaign start for {}", entry.campaign_id());
            }
        }
        ConvergenceEvent::EpochOpened(record) => {
            let state = campaign_state(campaigns, entry, "epoch")?;
            record.validate().with_context(|| {
                format!("invalid epoch record for campaign {}", entry.campaign_id())
            })?;
            if !state.epochs.insert(record.id().clone()) {
                bail!(
                    "duplicate epoch {} in campaign {}",
                    record.id(),
                    entry.campaign_id()
                );
            }
        }
        ConvergenceEvent::CoverageCellDefined(record) => {
            let state = campaign_state(campaigns, entry, "coverage cell")?;
            record.validate().with_context(|| {
                format!(
                    "invalid coverage cell record for campaign {}",
                    entry.campaign_id()
                )
            })?;
            if !state.epochs.contains(record.epoch_id()) {
                bail!(
                    "coverage cell {} references unopened epoch {} in campaign {}",
                    record.id(),
                    record.epoch_id(),
                    entry.campaign_id()
                );
            }
            if state.finalized_epochs.contains(record.epoch_id()) {
                bail!(
                    "coverage cell {} cannot be defined after epoch {} coverage plan finalization in campaign {}",
                    record.id(),
                    record.epoch_id(),
                    entry.campaign_id()
                );
            }
            if state
                .cells
                .insert(
                    record.id().clone(),
                    CoverageCellState {
                        epoch_id: record.epoch_id().clone(),
                        requirement: None,
                    },
                )
                .is_some()
            {
                bail!(
                    "duplicate coverage cell {} in campaign {}",
                    record.id(),
                    entry.campaign_id()
                );
            }
        }
        ConvergenceEvent::CoverageDispositionRecorded(record) => {
            let state = campaign_state(campaigns, entry, "coverage disposition")?;
            record.validate().with_context(|| {
                format!(
                    "invalid coverage disposition for {} in campaign {}",
                    record.coverage_cell_id(),
                    entry.campaign_id()
                )
            })?;
            let cell_epoch = state
                .cells
                .get(record.coverage_cell_id())
                .map(|cell| cell.epoch_id.clone())
                .with_context(|| {
                    format!(
                        "coverage disposition references undefined cell {} in campaign {}",
                        record.coverage_cell_id(),
                        entry.campaign_id()
                    )
                })?;
            if state.finalized_epochs.contains(&cell_epoch) {
                bail!(
                    "coverage disposition for cell {} cannot be recorded after epoch {} plan finalization in campaign {}",
                    record.coverage_cell_id(),
                    cell_epoch,
                    entry.campaign_id()
                );
            }
            let cell = state
                .cells
                .get_mut(record.coverage_cell_id())
                .context("validated coverage cell disappeared during replay")?;
            if cell.requirement.is_some() {
                bail!(
                    "duplicate coverage disposition for cell {} in campaign {}",
                    record.coverage_cell_id(),
                    entry.campaign_id()
                );
            }
            cell.requirement = Some(record.requirement());
        }
        ConvergenceEvent::CoveragePlanFinalized(record) => {
            let state = campaign_state(campaigns, entry, "coverage plan finalization")?;
            if !state.epochs.contains(record.epoch_id()) {
                bail!(
                    "coverage plan finalization references unopened epoch {} in campaign {}",
                    record.epoch_id(),
                    entry.campaign_id()
                );
            }
            if state.finalized_epochs.contains(record.epoch_id()) {
                bail!(
                    "duplicate coverage plan finalization for epoch {} in campaign {}",
                    record.epoch_id(),
                    entry.campaign_id()
                );
            }
            if let Some((cell_id, _)) = state
                .cells
                .iter()
                .find(|(_, cell)| &cell.epoch_id == record.epoch_id() && cell.requirement.is_none())
            {
                bail!(
                    "coverage plan for epoch {} cannot finalize before cell {} has a disposition in campaign {}",
                    record.epoch_id(),
                    cell_id,
                    entry.campaign_id()
                );
            }
            state.finalized_epochs.insert(record.epoch_id().clone());
        }
        ConvergenceEvent::DiscoveryAttemptRecorded(record) => {
            let state = campaign_state(campaigns, entry, "discovery attempt")?;
            record.validate().with_context(|| {
                format!(
                    "invalid discovery attempt {} in campaign {}",
                    record.id(),
                    entry.campaign_id()
                )
            })?;
            if !state.epochs.contains(record.epoch_id()) {
                bail!(
                    "discovery attempt {} references unopened epoch {} in campaign {}",
                    record.id(),
                    record.epoch_id(),
                    entry.campaign_id()
                );
            }
            if !state.finalized_epochs.contains(record.epoch_id()) {
                bail!(
                    "discovery attempt {} requires finalized coverage plan for epoch {} in campaign {}",
                    record.id(),
                    record.epoch_id(),
                    entry.campaign_id()
                );
            }
            let cell = state
                .cells
                .get(record.coverage_cell_id())
                .with_context(|| {
                    format!(
                        "discovery attempt {} references undefined coverage cell {} in campaign {}",
                        record.id(),
                        record.coverage_cell_id(),
                        entry.campaign_id()
                    )
                })?;
            if &cell.epoch_id != record.epoch_id() {
                bail!(
                    "discovery attempt {} epoch {} does not match coverage cell {} epoch {} in campaign {}",
                    record.id(),
                    record.epoch_id(),
                    record.coverage_cell_id(),
                    cell.epoch_id,
                    entry.campaign_id()
                );
            }
            if cell.requirement != Some(CoverageRequirement::Required) {
                bail!(
                    "discovery attempt {} requires coverage cell {} to have a Required disposition in campaign {}",
                    record.id(),
                    record.coverage_cell_id(),
                    entry.campaign_id()
                );
            }
            if state
                .attempts
                .insert(
                    record.id().clone(),
                    AttemptState {
                        epoch_id: record.epoch_id().clone(),
                        expected_candidate_count: record.reported_candidate_count(),
                        observed_candidate_count: 0,
                        producing_session: record.csa_session_id().clone(),
                        coverage_cell_id: record.coverage_cell_id().clone(),
                        model_identity: record.model_identity().clone(),
                        finalized: false,
                    },
                )
                .is_some()
            {
                bail!(
                    "duplicate discovery attempt {} in campaign {}",
                    record.id(),
                    entry.campaign_id()
                );
            }
        }
        ConvergenceEvent::CandidateRecorded(record) => {
            let state = campaign_state(campaigns, entry, "candidate")?;
            record.validate().with_context(|| {
                format!(
                    "invalid candidate {} in campaign {}",
                    record.id(),
                    entry.campaign_id()
                )
            })?;
            if state.candidates.contains_key(record.id()) {
                bail!(
                    "duplicate candidate {} in campaign {}",
                    record.id(),
                    entry.campaign_id()
                );
            }
            let attempt = state
                .attempts
                .get_mut(record.discovery_attempt_id())
                .with_context(|| {
                    format!(
                        "candidate {} references unknown discovery attempt {} in campaign {}",
                        record.id(),
                        record.discovery_attempt_id(),
                        entry.campaign_id()
                    )
                })?;
            if attempt.finalized {
                bail!(
                    "candidate {} cannot be recorded after discovery attempt {} finalization in campaign {}",
                    record.id(),
                    record.discovery_attempt_id(),
                    entry.campaign_id()
                );
            }
            if record.artifact().csa_session_id() != &attempt.producing_session {
                bail!(
                    "candidate {} artifact session {} does not match discovery attempt {} session {} for coverage cell {} in campaign {}",
                    record.id(),
                    record.artifact().csa_session_id(),
                    record.discovery_attempt_id(),
                    attempt.producing_session,
                    attempt.coverage_cell_id,
                    entry.campaign_id()
                );
            }
            if attempt.observed_candidate_count >= attempt.expected_candidate_count {
                bail!(
                    "candidate {} exceeds discovery attempt {} reported candidate count {} in campaign {}",
                    record.id(),
                    record.discovery_attempt_id(),
                    attempt.expected_candidate_count,
                    entry.campaign_id()
                );
            }
            attempt.observed_candidate_count = attempt
                .observed_candidate_count
                .checked_add(1)
                .context("discovery attempt observed candidate count overflow")?;
            state.candidates.insert(
                record.id().clone(),
                CandidateState {
                    stable_finding_id: record.stable_finding_id().clone(),
                    discovery_attempt_id: record.discovery_attempt_id().clone(),
                },
            );
            state
                .canonical_candidates
                .entry(record.stable_finding_id().clone())
                .or_insert_with(|| record.id().clone());
        }
        ConvergenceEvent::DiscoveryAttemptFinalized(record) => {
            let state = campaign_state(campaigns, entry, "discovery attempt finalization")?;
            let attempt = state
                .attempts
                .get_mut(record.discovery_attempt_id())
                .with_context(|| {
                    format!(
                        "discovery attempt finalization references unknown attempt {} in campaign {}",
                        record.discovery_attempt_id(),
                        entry.campaign_id()
                    )
                })?;
            if attempt.finalized {
                bail!(
                    "duplicate finalization for discovery attempt {} in campaign {}",
                    record.discovery_attempt_id(),
                    entry.campaign_id()
                );
            }
            if attempt.observed_candidate_count != attempt.expected_candidate_count {
                bail!(
                    "discovery attempt {} cannot finalize with {} observed candidates; expected {} in campaign {}",
                    record.discovery_attempt_id(),
                    attempt.observed_candidate_count,
                    attempt.expected_candidate_count,
                    entry.campaign_id()
                );
            }
            attempt.finalized = true;
        }
        ConvergenceEvent::CandidateDispositionRecorded(record) => {
            let state = campaign_state(campaigns, entry, "candidate disposition")?;
            record.validate().with_context(|| {
                format!(
                    "invalid candidate disposition for {} in campaign {}",
                    record.candidate_id(),
                    entry.campaign_id()
                )
            })?;
            let source = state
                .candidates
                .get(record.candidate_id())
                .cloned()
                .with_context(|| {
                    format!(
                        "candidate disposition references unknown candidate {} in campaign {}",
                        record.candidate_id(),
                        entry.campaign_id()
                    )
                })?;
            require_finalized_attempt(state, record.candidate_id(), &source, entry.campaign_id())?;
            if state.disposed_candidates.contains(record.candidate_id()) {
                bail!(
                    "duplicate terminal disposition for candidate {} in campaign {}",
                    record.candidate_id(),
                    entry.campaign_id()
                );
            }
            disposition_validation::validate_candidate_relation(
                state,
                record,
                &source,
                entry.campaign_id(),
            )?;
            disposition_validation::validate_verifier_evidence(
                state,
                record,
                &source,
                entry.campaign_id(),
            )?;
            state
                .dispositions
                .insert(record.candidate_id().clone(), record.clone());
            state
                .disposed_candidates
                .insert(record.candidate_id().clone());
        }
        ConvergenceEvent::RootClusterRecorded(record) => {
            repair_validation::record_root_cluster(campaigns, entry, record)?;
        }
        ConvergenceEvent::RepairBatchRecorded(record) => {
            repair_validation::record_repair_batch(campaigns, entry, record)?;
        }
        ConvergenceEvent::RepairHandoffRecorded(record) => {
            repair_validation::record_repair_handoff(campaigns, entry, record)?;
        }
        ConvergenceEvent::FinalReviewRecorded(_)
        | ConvergenceEvent::MergeAttestationRecorded(_) => {}
    }
    Ok(())
}

fn campaign_state<'a>(
    campaigns: &'a mut HashMap<CampaignId, CampaignState>,
    entry: &ConvergenceLedgerEntry,
    event_label: &str,
) -> Result<&'a mut CampaignState> {
    campaigns.get_mut(entry.campaign_id()).with_context(|| {
        format!(
            "{event_label} recorded before campaign {} started",
            entry.campaign_id()
        )
    })
}

fn require_finalized_attempt(
    state: &CampaignState,
    candidate_id: &CandidateId,
    candidate: &CandidateState,
    campaign_id: &CampaignId,
) -> Result<()> {
    let attempt = state
        .attempts
        .get(&candidate.discovery_attempt_id)
        .context("candidate references a discovery attempt absent from replay state")?;
    if !attempt.finalized {
        bail!(
            "candidate {} cannot receive a disposition before discovery attempt {} evidence finalization in campaign {}",
            candidate_id,
            candidate.discovery_attempt_id,
            campaign_id
        );
    }
    Ok(())
}

struct CampaignState {
    command_authority: CommandAuthoritySnapshot,
    epochs: HashSet<EpochId>,
    finalized_epochs: HashSet<EpochId>,
    cells: HashMap<CoverageCellId, CoverageCellState>,
    attempts: HashMap<DiscoveryAttemptId, AttemptState>,
    candidates: HashMap<CandidateId, CandidateState>,
    canonical_candidates: HashMap<StableFindingId, CandidateId>,
    disposed_candidates: HashSet<CandidateId>,
    dispositions: HashMap<CandidateId, CandidateDispositionRecord>,
    root_clusters: HashMap<RootClusterId, RootClusterState>,
    root_cluster_records: Vec<super::RootClusterRecord>,
    clustered_blocking_candidates: HashSet<CandidateId>,
    repair_batches: HashMap<RepairBatchId, RepairBatchState>,
    repair_batch_records: Vec<super::RepairBatchRecord>,
    repair_batches_by_cluster: HashMap<RootClusterId, RepairBatchId>,
    repair_handoffs: HashSet<RepairHandoffId>,
    repair_handoffs_by_batch: HashSet<RepairBatchId>,
}

impl CampaignState {
    fn new(command_authority: CommandAuthoritySnapshot) -> Self {
        Self {
            command_authority,
            epochs: HashSet::new(),
            finalized_epochs: HashSet::new(),
            cells: HashMap::new(),
            attempts: HashMap::new(),
            candidates: HashMap::new(),
            canonical_candidates: HashMap::new(),
            disposed_candidates: HashSet::new(),
            dispositions: HashMap::new(),
            root_clusters: HashMap::new(),
            root_cluster_records: Vec::new(),
            clustered_blocking_candidates: HashSet::new(),
            repair_batches: HashMap::new(),
            repair_batch_records: Vec::new(),
            repair_batches_by_cluster: HashMap::new(),
            repair_handoffs: HashSet::new(),
            repair_handoffs_by_batch: HashSet::new(),
        }
    }
}

struct CoverageCellState {
    epoch_id: EpochId,
    requirement: Option<CoverageRequirement>,
}

struct AttemptState {
    epoch_id: EpochId,
    expected_candidate_count: u32,
    observed_candidate_count: u32,
    producing_session: CsaSessionId,
    coverage_cell_id: CoverageCellId,
    model_identity: AdmittedModelIdentity,
    finalized: bool,
}

#[derive(Clone)]
struct CandidateState {
    stable_finding_id: StableFindingId,
    discovery_attempt_id: DiscoveryAttemptId,
}

struct RootClusterState {
    epoch_id: EpochId,
    candidate_ids: Vec<CandidateId>,
    candidate_set_digest: super::Sha256Digest,
    disposition_set_digest: super::Sha256Digest,
    content_digest: super::Sha256Digest,
}

struct RepairBatchState {
    epoch_id: EpochId,
    candidate_set_digest: super::Sha256Digest,
    disposition_set_digest: super::Sha256Digest,
    content_digest: super::Sha256Digest,
}
