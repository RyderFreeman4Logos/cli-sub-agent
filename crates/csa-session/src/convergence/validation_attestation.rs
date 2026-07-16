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

/// Re-read both content-addressed terminal artifacts before an attestation is constructed.
///
/// This deliberately checks only the two terminal artifacts. The full reader additionally
/// verifies every discovery, disposition, and repair artifact referenced by the committed ledger.
pub fn verify_terminal_artifact_pair<R: AttestationArtifactReader + ?Sized>(
    gate: &GateEvidenceRecord,
    review: &CleanRoomReviewRecord,
    reader: &R,
) -> Result<()> {
    gate.validate()?;
    review.validate()?;
    if gate.artifact() != review.gate_artifact() {
        bail!("clean-room review is not bound to the terminal gate artifact");
    }
    verify_artifact(reader, gate.artifact(), GATE_EVIDENCE_SCHEMA_ID)?;
    verify_artifact(reader, review.artifact(), CLEAN_ROOM_REVIEW_SCHEMA_ID)
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
        let schema = if artifact == attestation.gate_evidence.artifact() {
            Some(GATE_EVIDENCE_SCHEMA_ID)
        } else if artifact == review.artifact() {
            Some(CLEAN_ROOM_REVIEW_SCHEMA_ID)
        } else {
            None
        };
        verify_artifact_optional_schema(reader, artifact, schema)?;
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
    if attestation.execution_binding.campaign_id() != campaign_id
        || attestation.execution_binding.epoch_id() != &review.tuple.epoch_id
    {
        bail!("terminal execution binding does not match the terminal campaign epoch");
    }
    let authorization =
        latest_completion_authorization(entries, campaign_id, &review.tuple.epoch_id)?;
    if authorization.workspace_lease().generation()
        != attestation
            .execution_binding
            .authorization_lease_generation()
        || authorization.policy_digest() != attestation.execution_binding.policy_digest()
        || !attestation
            .cleanup_confirmation
            .matches_lease(authorization.workspace_lease())
    {
        bail!("terminal attestation does not bind the latest completion authorization lease");
    }
    let expected = compute_from_entries(entries, campaign_id, &attestation.gate_evidence, review)?;
    if expected != attestation.bindings {
        bail!("merge attestation does not bind complete authoritative ledger state");
    }
    Ok(())
}

fn verify_artifact<R: AttestationArtifactReader + ?Sized>(
    reader: &R,
    artifact: &super::ArtifactEvidenceRef,
    schema: &str,
) -> Result<()> {
    verify_artifact_optional_schema(reader, artifact, Some(schema))
}

fn verify_artifact_optional_schema<R: AttestationArtifactReader + ?Sized>(
    reader: &R,
    artifact: &super::ArtifactEvidenceRef,
    schema: Option<&str>,
) -> Result<()> {
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
    if let Some(schema) = schema {
        require_schema(&bytes, schema)?;
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
    let authorization = latest_completion_authorization(entries, campaign_id, epoch.id())?;
    if gate.policy_digest != *policy
        || gate.provider_command_authority_digest != *campaign.command_authority_digest()
        || gate.final_gate_authority_digest != *authorization.final_gate_authority_digest()
    {
        bail!("terminal policy, provider authority, or final-gate authority mismatch");
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

fn latest_completion_authorization<'a>(
    entries: &'a [ConvergenceLedgerEntry],
    campaign_id: &CampaignId,
    epoch_id: &super::EpochId,
) -> Result<&'a super::CompletionAuthorizationRecord> {
    entries
        .iter()
        .rev()
        .find_map(|entry| match entry.event() {
            ConvergenceEvent::CompletionAuthorizationRecorded(authorization)
                if entry.campaign_id() == campaign_id && authorization.epoch_id() == epoch_id =>
            {
                Some(authorization)
            }
            _ => None,
        })
        .context("terminal attestation lacks a completion authorization for its exact epoch")
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
