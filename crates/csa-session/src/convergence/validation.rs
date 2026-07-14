use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, bail};

use super::{
    CONVERGENCE_LEDGER_SCHEMA_VERSION, CampaignId, CandidateDisposition, CandidateId,
    ConvergenceEvent, ConvergenceLedgerEntry, CoverageCellId, CoverageRequirement, CsaSessionId,
    DiscoveryAttemptId, EpochId, LedgerEventId, StableFindingId,
};

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
                .insert(entry.campaign_id().clone(), CampaignState::default())
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
                        expected_candidate_count: record.reported_candidate_count(),
                        observed_candidate_count: 0,
                        producing_session: record.csa_session_id().clone(),
                        coverage_cell_id: record.coverage_cell_id().clone(),
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
            validate_candidate_relation(state, record, &source, entry.campaign_id())?;
            state
                .disposed_candidates
                .insert(record.candidate_id().clone());
        }
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

fn validate_candidate_relation(
    state: &CampaignState,
    record: &super::CandidateDispositionRecord,
    source: &CandidateState,
    campaign_id: &CampaignId,
) -> Result<()> {
    let target_id = match record.disposition() {
        CandidateDisposition::Duplicate {
            canonical_candidate_id,
        } => Some((canonical_candidate_id, true, "duplicates")),
        CandidateDisposition::Superseded {
            replacement_candidate_id,
        } => Some((replacement_candidate_id, false, "references superseding")),
        CandidateDisposition::Verified
        | CandidateDisposition::RejectedWithEvidence
        | CandidateDisposition::NeedsContractOrDocumentation
        | CandidateDisposition::PreExistingOutsideDiffScope => None,
    };
    let Some((target_id, requires_same_stable_id, relation)) = target_id else {
        return Ok(());
    };
    if target_id == record.candidate_id() {
        bail!(
            "candidate {} cannot relate to itself in campaign {}",
            record.candidate_id(),
            campaign_id
        );
    }
    let target = state.candidates.get(target_id).with_context(|| {
        format!(
            "candidate {} {relation} missing candidate {} in campaign {}",
            record.candidate_id(),
            target_id,
            campaign_id
        )
    })?;
    require_finalized_attempt(state, target_id, target, campaign_id)?;
    if requires_same_stable_id && target.stable_finding_id != source.stable_finding_id {
        bail!(
            "candidate {} cannot duplicate candidate {} with a different stable finding id in campaign {}",
            record.candidate_id(),
            target_id,
            campaign_id
        );
    }
    Ok(())
}

#[derive(Default)]
struct CampaignState {
    epochs: HashSet<EpochId>,
    finalized_epochs: HashSet<EpochId>,
    cells: HashMap<CoverageCellId, CoverageCellState>,
    attempts: HashMap<DiscoveryAttemptId, AttemptState>,
    candidates: HashMap<CandidateId, CandidateState>,
    disposed_candidates: HashSet<CandidateId>,
}

struct CoverageCellState {
    epoch_id: EpochId,
    requirement: Option<CoverageRequirement>,
}

struct AttemptState {
    expected_candidate_count: u32,
    observed_candidate_count: u32,
    producing_session: CsaSessionId,
    coverage_cell_id: CoverageCellId,
    finalized: bool,
}

#[derive(Clone)]
struct CandidateState {
    stable_finding_id: StableFindingId,
    discovery_attempt_id: DiscoveryAttemptId,
}
