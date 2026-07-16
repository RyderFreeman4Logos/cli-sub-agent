//! Immutable records for headless convergence attestations.

use std::collections::HashSet;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use super::{
    ArtifactEvidenceRef, CampaignId, EpochId, EpochRecord, GitObjectId, ModelEvidence,
    Sha256Digest, hash_fields, normalize_nonblank,
};

/// Schema required from the immutable gate artifact.
pub const GATE_EVIDENCE_SCHEMA_ID: &str = "csa.convergence.gate-evidence/v2";
/// Schema required from the immutable final clean-room artifact.
pub const CLEAN_ROOM_REVIEW_SCHEMA_ID: &str = "csa.convergence.clean-room-review/v2";
/// Historical v1 artifacts are inspection-only and cannot be used to form new terminal evidence.
pub const LEGACY_CLEAN_ROOM_REVIEW_SCHEMA_ID: &str = "csa.convergence.clean-room-review/v1";
/// Schema identity of merge attestation records.
pub const MERGE_ATTESTATION_SCHEMA_ID: &str = "csa.convergence.merge-attestation/v1";

const GATE_DOMAIN: &[u8] = b"csa-convergence-gate-evidence-v1\0";
const REVIEW_DOMAIN: &[u8] = b"csa-convergence-clean-review-v1\0";
const ATTESTATION_DOMAIN: &[u8] = b"csa-convergence-merge-attestation-v1\0";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AttestationTuple {
    pub(super) campaign_id: CampaignId,
    pub(super) epoch_id: EpochId,
    pub(super) base_oid: GitObjectId,
    pub(super) head_oid: GitObjectId,
    pub(super) diff_digest: Sha256Digest,
}

impl AttestationTuple {
    pub(super) fn new(campaign_id: CampaignId, epoch: &EpochRecord) -> Self {
        Self {
            campaign_id,
            epoch_id: epoch.id().clone(),
            base_oid: epoch.base_oid().clone(),
            head_oid: epoch.head_oid().clone(),
            diff_digest: epoch.diff_digest().clone(),
        }
    }

    pub(super) fn validate(&self) -> Result<()> {
        let epoch = EpochRecord::new(
            self.base_oid.clone(),
            self.head_oid.clone(),
            self.diff_digest.clone(),
        );
        if epoch.id() != &self.epoch_id {
            bail!("attestation epoch does not match its exact tuple");
        }
        Ok(())
    }
}

/// One canonical gate command and its exact process result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GateCommandResult {
    command: String,
    exit_code: i32,
}

impl GateCommandResult {
    /// Construct one successful gate result.
    pub fn new(command: &str, exit_code: i32) -> Result<Self> {
        let command = normalize_nonblank("gate command", command)?;
        if exit_code != 0 {
            bail!("gate command '{command}' failed with exit code {exit_code}");
        }
        Ok(Self { command, exit_code })
    }

    fn validate(&self) -> Result<()> {
        if Self::new(&self.command, self.exit_code)? != *self {
            bail!("invalid gate command result");
        }
        Ok(())
    }
}

/// Immutable evidence for the canonical gate command sequence on one exact tuple.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GateEvidenceRecord {
    pub(super) tuple: AttestationTuple,
    pub(super) policy_digest: Sha256Digest,
    pub(super) command_authority_digest: Sha256Digest,
    commands: Vec<GateCommandResult>,
    artifact: ArtifactEvidenceRef,
    pub(super) content_digest: Sha256Digest,
}

impl GateEvidenceRecord {
    /// Construct exact-tuple gate evidence in authoritative execution order.
    pub fn new(
        campaign_id: CampaignId,
        epoch: &EpochRecord,
        policy_digest: Sha256Digest,
        command_authority_digest: Sha256Digest,
        commands: Vec<GateCommandResult>,
        artifact: ArtifactEvidenceRef,
    ) -> Result<Self> {
        validate_commands(&commands)?;
        let tuple = AttestationTuple::new(campaign_id, epoch);
        let mut record = Self {
            tuple,
            policy_digest,
            command_authority_digest,
            commands,
            artifact,
            content_digest: Sha256Digest::compute(&[]),
        };
        record.content_digest = record.digest();
        Ok(record)
    }

    /// Return gate results in authoritative execution order.
    #[must_use]
    pub fn commands(&self) -> &[GateCommandResult] {
        &self.commands
    }

    /// Return the immutable gate artifact reference.
    #[must_use]
    pub fn artifact(&self) -> &ArtifactEvidenceRef {
        &self.artifact
    }

    pub(super) fn validate(&self) -> Result<()> {
        self.tuple.validate()?;
        validate_commands(&self.commands)?;
        if self.digest() != self.content_digest {
            bail!("gate evidence content digest mismatch");
        }
        Ok(())
    }

    fn digest(&self) -> Sha256Digest {
        digest_serialized(
            GATE_DOMAIN,
            &(
                &self.tuple,
                &self.policy_digest,
                &self.command_authority_digest,
                &self.commands,
                &self.artifact,
            ),
        )
    }
}

/// Fresh zero-finding clean-room review evidence for one exact tuple.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CleanRoomReviewRecord {
    pub(super) tuple: AttestationTuple,
    pub(super) model_evidence: ModelEvidence,
    pub(super) gate_artifact: ArtifactEvidenceRef,
    artifact: ArtifactEvidenceRef,
    pub(super) findings_count: u32,
    pub(super) questions_count: u32,
    pub(super) unchecked_count: u32,
    pub(super) content_digest: Sha256Digest,
}

/// The two host-owned artifacts that bind a v2 clean-room review.
///
/// Keeping this pair separate from provider content prevents callers from accidentally treating
/// a review artifact as its prerequisite gate artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CleanRoomReviewArtifactBindings {
    gate_artifact: ArtifactEvidenceRef,
    review_artifact: ArtifactEvidenceRef,
}

impl CleanRoomReviewArtifactBindings {
    /// Bind one authoritative gate artifact to the independently published review artifact.
    #[must_use]
    pub fn new(gate_artifact: ArtifactEvidenceRef, review_artifact: ArtifactEvidenceRef) -> Self {
        Self {
            gate_artifact,
            review_artifact,
        }
    }
}

impl CleanRoomReviewRecord {
    /// Construct v2 attestable zero/zero/zero clean-room evidence.
    ///
    /// The model evidence and gate artifact are host-owned bindings. A provider response cannot
    /// supply either value, and a transport report cannot become a verified actual-model claim
    /// without the independent proof required by [`ModelEvidence`].
    pub fn new(
        campaign_id: CampaignId,
        epoch: &EpochRecord,
        model_evidence: ModelEvidence,
        artifacts: CleanRoomReviewArtifactBindings,
        findings_count: u32,
        questions_count: u32,
        unchecked_count: u32,
    ) -> Result<Self> {
        require_zero(findings_count, questions_count, unchecked_count)?;
        let tuple = AttestationTuple::new(campaign_id, epoch);
        let CleanRoomReviewArtifactBindings {
            gate_artifact,
            review_artifact: artifact,
        } = artifacts;
        let mut record = Self {
            tuple,
            model_evidence,
            gate_artifact,
            artifact,
            findings_count,
            questions_count,
            unchecked_count,
            content_digest: Sha256Digest::compute(&[]),
        };
        record.content_digest = record.digest();
        Ok(record)
    }

    /// Return the immutable clean-room artifact reference.
    #[must_use]
    pub fn artifact(&self) -> &ArtifactEvidenceRef {
        &self.artifact
    }

    /// Return host-authoritative layered model evidence for this review execution.
    #[must_use]
    pub fn model_evidence(&self) -> &ModelEvidence {
        &self.model_evidence
    }

    /// Return the gate artifact that this review explicitly verifies.
    #[must_use]
    pub fn gate_artifact(&self) -> &ArtifactEvidenceRef {
        &self.gate_artifact
    }

    pub(super) fn validate(&self) -> Result<()> {
        self.tuple.validate()?;
        require_zero(
            self.findings_count,
            self.questions_count,
            self.unchecked_count,
        )?;
        if self.digest() != self.content_digest {
            bail!("clean-room review content digest mismatch");
        }
        Ok(())
    }

    fn digest(&self) -> Sha256Digest {
        digest_serialized(
            REVIEW_DOMAIN,
            &(
                &self.tuple,
                &self.model_evidence,
                &self.gate_artifact,
                &self.artifact,
                self.findings_count,
                self.questions_count,
                self.unchecked_count,
            ),
        )
    }
}

/// Domain-separated digests binding every accepted terminal input set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttestationBindingDigests {
    pub(super) base_oid: Sha256Digest,
    pub(super) head_oid: Sha256Digest,
    pub(super) diff_digest: Sha256Digest,
    pub(super) policy_digest: Sha256Digest,
    pub(super) command_authority: Sha256Digest,
    pub(super) command_catalog: Sha256Digest,
    pub(super) coverage_manifest: Sha256Digest,
    pub(super) candidate_set: Sha256Digest,
    pub(super) disposition_set: Sha256Digest,
    pub(super) root_cluster_set: Sha256Digest,
    pub(super) repair_set: Sha256Digest,
    pub(super) gate_evidence: Sha256Digest,
    pub(super) clean_room_artifact: Sha256Digest,
    pub(super) clean_room_model: Sha256Digest,
}

/// Immutable terminal merge attestation and all artifact/binding evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MergeAttestationRecord {
    schema_identity: String,
    pub(super) tuple: AttestationTuple,
    pub(super) gate_evidence: GateEvidenceRecord,
    pub(super) clean_room_artifact: ArtifactEvidenceRef,
    pub(super) bindings: AttestationBindingDigests,
    content_digest: Sha256Digest,
}

impl MergeAttestationRecord {
    /// Construct a terminal attestation from matching gate and review evidence.
    pub fn new(
        gate: &GateEvidenceRecord,
        review: &CleanRoomReviewRecord,
        bindings: AttestationBindingDigests,
    ) -> Result<Self> {
        gate.validate()?;
        review.validate()?;
        if gate.tuple != review.tuple {
            bail!("gate and review do not bind the same campaign and exact tuple");
        }
        if gate.artifact != review.gate_artifact {
            bail!("clean-room review is not bound to the exact gate artifact");
        }
        let mut record = Self {
            schema_identity: MERGE_ATTESTATION_SCHEMA_ID.to_string(),
            tuple: review.tuple.clone(),
            gate_evidence: gate.clone(),
            clean_room_artifact: review.artifact.clone(),
            bindings,
            content_digest: Sha256Digest::compute(&[]),
        };
        record.content_digest = record.digest();
        Ok(record)
    }

    pub(super) fn validate(&self) -> Result<()> {
        if self.schema_identity != MERGE_ATTESTATION_SCHEMA_ID {
            bail!("unsupported merge attestation schema identity");
        }
        self.tuple.validate()?;
        self.gate_evidence.validate()?;
        if self.gate_evidence.tuple != self.tuple || self.digest() != self.content_digest {
            bail!("merge attestation identity or content digest mismatch");
        }
        Ok(())
    }

    fn digest(&self) -> Sha256Digest {
        digest_serialized(
            ATTESTATION_DOMAIN,
            &(
                &self.schema_identity,
                &self.tuple,
                &self.gate_evidence.content_digest,
                &self.gate_evidence.artifact,
                &self.clean_room_artifact,
                &self.bindings,
            ),
        )
    }
}

/// Narrow headless boundary for immutable artifact bytes.
pub trait AttestationArtifactReader {
    /// Read exactly the referenced session-relative artifact.
    fn read_artifact(&self, artifact: &ArtifactEvidenceRef) -> Result<Vec<u8>>;
}

impl<F> AttestationArtifactReader for F
where
    F: Fn(&ArtifactEvidenceRef) -> Result<Vec<u8>>,
{
    fn read_artifact(&self, artifact: &ArtifactEvidenceRef) -> Result<Vec<u8>> {
        self(artifact)
    }
}

fn validate_commands(commands: &[GateCommandResult]) -> Result<()> {
    if commands.is_empty() {
        bail!("gate evidence command set must not be empty");
    }
    let mut seen = HashSet::new();
    for command in commands {
        command.validate()?;
        if !seen.insert(command.command.as_str()) {
            bail!("duplicate gate command '{}'", command.command);
        }
    }
    Ok(())
}

fn require_zero(findings: u32, questions: u32, unchecked: u32) -> Result<()> {
    if (findings, questions, unchecked) != (0, 0, 0) {
        bail!(
            "review is not attestable: findings={findings}, questions={questions}, unchecked={unchecked}"
        );
    }
    Ok(())
}

fn digest_serialized(domain: &[u8], value: &impl Serialize) -> Sha256Digest {
    let bytes = serde_json::to_vec(value).expect("attestation content serializes");
    let payload = Sha256Digest::compute(&bytes);
    hash_fields(domain, &[payload.as_str()])
}

pub(super) fn artifact_fields<'a>(
    schema: &'a str,
    artifact: &'a ArtifactEvidenceRef,
) -> [&'a str; 4] {
    [
        schema,
        artifact.csa_session_id().as_str(),
        artifact.path().as_str(),
        artifact.digest().as_str(),
    ]
}
