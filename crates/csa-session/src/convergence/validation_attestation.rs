//! Authoritative replay and fail-closed verification for terminal attestations.

use anyhow::{Context, Result, bail};

use super::attestation::{
    AttestationArtifactReader, AttestationBindingDigests, CLEAN_ROOM_REVIEW_SCHEMA_ID,
    CleanRoomReviewRecord, GATE_EVIDENCE_SCHEMA_ID, GateEvidenceRecord, artifact_fields,
};
use super::ledger::{artifact_refs, bind, require_schema, set_digests, terminal_pair};
use super::{
    CampaignId, CampaignRecord, ConvergenceEvent, ConvergenceLedger, ConvergenceLedgerEntry,
    EpochRecord, Sha256Digest,
};

/// Compute all terminal bindings from validated authoritative ledger state.
pub fn compute_attestation_bindings(
    ledger: &ConvergenceLedger,
    campaign_id: &CampaignId,
    gate: &GateEvidenceRecord,
    review: &CleanRoomReviewRecord,
) -> Result<AttestationBindingDigests> {
    ledger.validate()?;
    compute_from_entries(ledger.entries(), campaign_id, gate, review)
}

/// Verify terminal bindings and every referenced artifact through an injected reader.
pub fn verify_merge_attestation<R: AttestationArtifactReader + ?Sized>(
    ledger: &ConvergenceLedger,
    campaign_id: &CampaignId,
    reader: &R,
) -> Result<()> {
    ledger.validate()?;
    let (review, attestation) = terminal_pair(ledger.entries())?;
    let expected = compute_from_entries(
        ledger.entries(),
        campaign_id,
        &attestation.gate_evidence,
        review,
    )?;
    if expected != attestation.bindings {
        bail!("merge attestation binding mismatch");
    }
    for artifact in artifact_refs(ledger.entries(), campaign_id) {
        let bytes = reader.read_artifact(artifact).with_context(|| {
            format!(
                "read attestation artifact {}/{}",
                artifact.csa_session_id(),
                artifact.path()
            )
        })?;
        if Sha256Digest::compute(&bytes) != *artifact.digest() {
            bail!("artifact digest mismatch for {}", artifact.path());
        }
        if artifact == attestation.gate_evidence.artifact() {
            require_schema(&bytes, GATE_EVIDENCE_SCHEMA_ID)?;
        }
        if artifact == review.artifact() {
            require_schema(&bytes, CLEAN_ROOM_REVIEW_SCHEMA_ID)?;
        }
    }
    Ok(())
}

/// Validate the optional atomic terminal pair while accepting historical prefixes.
pub(super) fn validate_terminal_pair(entries: &[ConvergenceLedgerEntry]) -> Result<()> {
    let terminal_count = entries
        .iter()
        .filter(|entry| {
            matches!(
                entry.event(),
                ConvergenceEvent::FinalReviewRecorded(_)
                    | ConvergenceEvent::MergeAttestationRecorded(_)
            )
        })
        .count();
    if terminal_count == 0 {
        return Ok(());
    }
    if terminal_count != 2 {
        bail!("history requires one terminal final-review/attestation pair");
    }
    let (review, attestation) = terminal_pair(entries)?;
    review.validate()?;
    attestation.validate()?;
    let campaign_id = entries
        .last()
        .expect("terminal pair is nonempty")
        .campaign_id();
    if entries[entries.len() - 2].campaign_id() != campaign_id
        || review.tuple.campaign_id != *campaign_id
        || attestation.tuple != review.tuple
        || attestation.gate_evidence.artifact() != review.gate_artifact()
        || attestation.clean_room_artifact != *review.artifact()
    {
        bail!("terminal pair identity, tuple, or clean-room artifact mismatch");
    }
    let expected = compute_from_entries(entries, campaign_id, &attestation.gate_evidence, review)?;
    if expected != attestation.bindings {
        bail!("merge attestation does not bind complete authoritative ledger state");
    }
    Ok(())
}

fn compute_from_entries(
    entries: &[ConvergenceLedgerEntry],
    campaign_id: &CampaignId,
    gate: &GateEvidenceRecord,
    review: &CleanRoomReviewRecord,
) -> Result<AttestationBindingDigests> {
    gate.validate()?;
    review.validate()?;
    if gate.tuple != review.tuple || gate.tuple.campaign_id != *campaign_id {
        bail!("terminal evidence campaign or tuple mismatch");
    }
    let (campaign, epoch) = campaign_epoch(entries, campaign_id)?;
    if gate.tuple.epoch_id != *epoch.id() {
        bail!("terminal evidence does not target the current epoch");
    }
    let policy = campaign
        .policy_digest()
        .context("attestation requires a frozen policy digest")?;
    if gate.policy_digest != *policy
        || gate.command_authority_digest != *campaign.command_authority_digest()
    {
        bail!("terminal policy or command authority mismatch");
    }
    if !campaign
        .command_authority()
        .contains(review.model_evidence.admitted_model())
    {
        bail!("clean-room model is outside command authority");
    }
    let sets = set_digests(entries, campaign_id, epoch)?;
    let tuple = &gate.tuple;
    let catalog = campaign.command_authority().catalog();
    Ok(AttestationBindingDigests {
        base_oid: bind("base_oid", &[tuple.base_oid.as_str()]),
        head_oid: bind("head_oid", &[tuple.head_oid.as_str()]),
        diff_digest: bind("diff_digest", &[tuple.diff_digest.as_str()]),
        policy_digest: bind("policy_digest", &[policy.as_str()]),
        command_authority: bind(
            "command_authority",
            &[campaign.command_authority_digest().as_str()],
        ),
        command_catalog: bind("command_catalog", &[catalog.source(), catalog.version()]),
        coverage_manifest: sets.coverage,
        candidate_set: sets.candidates,
        disposition_set: bind("disposition_set", &[sets.dispositions.as_str()]),
        root_cluster_set: bind("root_cluster_set", &[sets.clusters.as_str()]),
        repair_set: bind("repair_set", &[sets.repairs.as_str()]),
        gate_evidence: bind("gate_evidence", &[gate.content_digest.as_str()]),
        clean_room_artifact: bind(
            "clean_room_artifact",
            &artifact_fields(CLEAN_ROOM_REVIEW_SCHEMA_ID, review.artifact()),
        ),
        clean_room_model: bind(
            "clean_room_model",
            &[
                review.model_evidence.admitted_model().tool(),
                review.model_evidence.admitted_model().provider(),
                review.model_evidence.admitted_model().model(),
                review.model_evidence.admitted_model().reasoning(),
                review.model_evidence.observed_tool().tool(),
                review.model_evidence.observed_tool().version(),
                review.model_evidence.execution_id().as_str(),
            ],
        ),
    })
}

fn campaign_epoch<'a>(
    entries: &'a [ConvergenceLedgerEntry],
    campaign_id: &CampaignId,
) -> Result<(&'a CampaignRecord, &'a EpochRecord)> {
    let campaign = entries.iter().find_map(|entry| match entry.event() {
        ConvergenceEvent::CampaignStarted(record) if entry.campaign_id() == campaign_id => {
            Some(record)
        }
        _ => None,
    });
    let epoch = entries.iter().rev().find_map(|entry| match entry.event() {
        ConvergenceEvent::EpochOpened(record) if entry.campaign_id() == campaign_id => Some(record),
        _ => None,
    });
    Ok((
        campaign.context("campaign missing")?,
        epoch.context("epoch missing")?,
    ))
}
