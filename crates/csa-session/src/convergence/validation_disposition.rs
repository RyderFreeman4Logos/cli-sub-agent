//! Candidate-disposition replay validation kept separate from the campaign state machine.

use anyhow::{Context, Result, bail};

use super::{CampaignState, CandidateState, require_finalized_attempt};
use crate::convergence::{
    CampaignId, CandidateDisposition, CandidateDispositionRecord, VerificationIndependence,
};

pub(super) fn validate_candidate_relation(
    state: &CampaignState,
    record: &CandidateDispositionRecord,
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
    let source_attempt = state
        .attempts
        .get(&source.discovery_attempt_id)
        .context("candidate source attempt disappeared during relation validation")?;
    let target_attempt = state
        .attempts
        .get(&target.discovery_attempt_id)
        .context("candidate target attempt disappeared during relation validation")?;
    if source_attempt.epoch_id != target_attempt.epoch_id {
        bail!(
            "candidate {} cannot {relation} candidate {} across epochs in campaign {}",
            record.candidate_id(),
            target_id,
            campaign_id
        );
    }
    if requires_same_stable_id && target.stable_finding_id != source.stable_finding_id {
        bail!(
            "candidate {} cannot duplicate candidate {} with a different stable finding id in campaign {}",
            record.candidate_id(),
            target_id,
            campaign_id
        );
    }
    if requires_same_stable_id {
        let canonical = state
            .canonical_candidates
            .get(&source.stable_finding_id)
            .context("stable candidate canonicalization disappeared during validation")?;
        if canonical != target_id {
            bail!(
                "candidate {} must duplicate first canonical candidate {} rather than {} in campaign {}",
                record.candidate_id(),
                canonical,
                target_id,
                campaign_id
            );
        }
        if matches!(
            state.dispositions.get(target_id),
            Some(record) if matches!(record.disposition(), CandidateDisposition::Duplicate { .. })
        ) {
            bail!(
                "candidate {} cannot duplicate noncanonical duplicate target {} in campaign {}",
                record.candidate_id(),
                target_id,
                campaign_id
            );
        }
    }
    Ok(())
}

pub(super) fn validate_verifier_evidence(
    state: &CampaignState,
    record: &CandidateDispositionRecord,
    source: &CandidateState,
    campaign_id: &CampaignId,
) -> Result<()> {
    let attempt = state
        .attempts
        .get(&source.discovery_attempt_id)
        .context("candidate source attempt disappeared during verifier validation")?;
    if record.epoch_id() != &attempt.epoch_id {
        bail!(
            "candidate disposition for {} binds epoch {} but discovery attempt belongs to epoch {} in campaign {}",
            record.candidate_id(),
            record.epoch_id(),
            attempt.epoch_id,
            campaign_id
        );
    }
    if !state.command_authority.contains(record.actual_executor()) {
        bail!(
            "candidate disposition for {} uses executor outside frozen command authority in campaign {}",
            record.candidate_id(),
            campaign_id
        );
    }
    match record.independence() {
        VerificationIndependence::Heterogeneous
            if record.actual_executor() == &attempt.model_identity =>
        {
            bail!(
                "candidate disposition for {} claims heterogeneous verification with its discovery executor in campaign {}",
                record.candidate_id(),
                campaign_id
            );
        }
        VerificationIndependence::Degraded { .. }
            if record.actual_executor() != &attempt.model_identity =>
        {
            bail!(
                "candidate disposition for {} records degraded independence despite a heterogeneous executor in campaign {}",
                record.candidate_id(),
                campaign_id
            );
        }
        VerificationIndependence::Heterogeneous | VerificationIndependence::Degraded { .. } => {}
    }
    Ok(())
}
