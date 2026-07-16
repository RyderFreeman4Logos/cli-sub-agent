use std::{collections::BTreeMap, path::PathBuf};

use anyhow::{Context, Result, anyhow};
use chrono::{TimeZone, Utc};
use csa_process::ProviderTurnCompletion;
use serde_json::{Value, json};
use ulid::Ulid;

use crate::convergence::{
    AdmittedModelIdentity, ArtifactEvidenceRef, AttestationBindingDigests, CampaignId,
    CampaignRecord, CandidateDisposition, CandidateDispositionRecord, CandidateId, CandidateRecord,
    CandidateVerificationEvidence, CleanRoomReviewArtifactBindings, CleanRoomReviewRecord,
    CleanupConfirmation, CommandAuthorityCatalogIdentity, CommandAuthorityPolicy,
    CommandAuthoritySnapshot, CommandAuthoritySource, CompletionActionId,
    CompletionAuthorizationRecord, ConvergenceEvent, ConvergenceLedger, CoverageCellRecord,
    CoverageDispositionRecord, CoveragePlanFinalizationRecord, CoverageRequirement, CoverageScope,
    CsaSessionId, DiscoveryAttemptFinalizationRecord, DiscoveryAttemptId, DiscoveryAttemptRecord,
    EpochRecord, GateCommandResult, GateEvidenceRecord, GitObjectId, MergeAttestationRecord,
    ModelEvidence, ObservedToolEvidence, ProviderTurnExecutionId, RepairBatchRecord,
    RootClusterRecord, SemanticFindingIdentity, SemanticLens, SessionRelativeArtifactPath,
    Sha256Digest, TerminalExecutionBinding, VerificationIndependence, WorkspaceLeaseIdentity,
    compute_attestation_bindings, verify_merge_attestation,
};

const CAMPAIGN: &str = "01ARZ3NDEKTSV4RRFFQ69G5FC0";
const DISCOVERY_ATTEMPT: &str = "01ARZ3NDEKTSV4RRFFQ69G5FC1";
const DISCOVERY_SESSION: &str = "01ARZ3NDEKTSV4RRFFQ69G5FC2";
const CANDIDATE: &str = "01ARZ3NDEKTSV4RRFFQ69G5FC3";
const VERIFIER_SESSION: &str = "01ARZ3NDEKTSV4RRFFQ69G5FC4";
const GATE_SESSION: &str = "01ARZ3NDEKTSV4RRFFQ69G5FC5";
const REVIEW_SESSION: &str = "01ARZ3NDEKTSV4RRFFQ69G5FC6";
const GATE_SCHEMA: &str = "csa.convergence.gate-evidence/v2";
const REVIEW_SCHEMA: &str = "csa.convergence.clean-room-review/v2";

type ArtifactKey = (String, String);

fn digest(fill: char) -> Sha256Digest {
    Sha256Digest::parse(&format!("sha256:{}", fill.to_string().repeat(64))).unwrap()
}

fn oid(fill: char) -> GitObjectId {
    GitObjectId::parse(&fill.to_string().repeat(40)).unwrap()
}

fn model() -> AdmittedModelIdentity {
    AdmittedModelIdentity::new("codex", "openai", "gpt-5.6", "xhigh").unwrap()
}

fn model_evidence() -> ModelEvidence {
    ModelEvidence::host_observed(
        model(),
        ObservedToolEvidence::new("codex", "test-version").unwrap(),
        Some("gpt-5.6"),
        ProviderTurnExecutionId::generate(),
    )
    .unwrap()
}

fn authority() -> CommandAuthoritySnapshot {
    CommandAuthoritySnapshot::new(
        CommandAuthoritySource::tier("tier-4-critical", "review.tier").unwrap(),
        CommandAuthorityPolicy::new(false, vec!["codex".to_string()], false, true).unwrap(),
        CommandAuthorityCatalogIdentity::new("merged:model-catalog.toml", "catalog-v9").unwrap(),
        vec![model()],
    )
    .unwrap()
}

fn artifact(session: &str, path: &str, bytes: &[u8]) -> ArtifactEvidenceRef {
    ArtifactEvidenceRef::new(
        CsaSessionId::parse(session).unwrap(),
        SessionRelativeArtifactPath::new(path).unwrap(),
        Sha256Digest::compute(bytes),
    )
}

fn artifact_key(reference: &ArtifactEvidenceRef) -> ArtifactKey {
    (
        reference.csa_session_id().as_str().to_string(),
        reference.path().as_str().to_string(),
    )
}

#[derive(Clone)]
pub(crate) struct Fixture {
    pub(crate) campaign_id: CampaignId,
    pub(crate) epoch: EpochRecord,
    pub(crate) prefix_events: Vec<ConvergenceEvent>,
    pub(crate) ledger: ConvergenceLedger,
    pub(crate) gate: GateEvidenceRecord,
    pub(crate) review: CleanRoomReviewRecord,
    pub(crate) cleanup_lease: WorkspaceLeaseIdentity,
    pub(crate) policy_digest: Sha256Digest,
    pub(crate) terminal_execution_binding: TerminalExecutionBinding,
    pub(crate) terminal_action_id: CompletionActionId,
    artifacts: BTreeMap<ArtifactKey, Vec<u8>>,
}

impl Fixture {
    pub(crate) fn new() -> Self {
        Self::with_review_bytes(
            format!(r#"{{"schema":"{REVIEW_SCHEMA}","result":"clean"}}"#).into_bytes(),
        )
    }

    pub(crate) fn with_review_bytes(review_bytes: Vec<u8>) -> Self {
        let campaign_id = CampaignId::parse(CAMPAIGN).unwrap();
        let epoch = EpochRecord::new(oid('a'), oid('b'), digest('c'));
        let command_authority = authority();
        let campaign = CampaignRecord::new(
            campaign_id.clone(),
            Utc.with_ymd_and_hms(2026, 7, 14, 12, 0, 0).unwrap(),
            Some(digest('d')),
            command_authority.clone(),
        );
        let policy_digest = campaign.policy_digest().unwrap().clone();
        let cell = CoverageCellRecord::new(
            epoch.id().clone(),
            CoverageScope::new("crate", "csa-session").unwrap(),
            SemanticLens::new("correctness").unwrap(),
        );
        let attempt_id = DiscoveryAttemptId::parse(DISCOVERY_ATTEMPT).unwrap();
        let discovery_bytes = br#"{"kind":"discovery","complete":true}"#.to_vec();
        let candidate_bytes = br#"{"kind":"candidate","blocking":true}"#.to_vec();
        let disposition_bytes = br#"{"kind":"disposition","verified":true}"#.to_vec();
        let discovery_artifact = artifact(
            DISCOVERY_SESSION,
            "discovery/attempt.json",
            &discovery_bytes,
        );
        let candidate_artifact = artifact(
            DISCOVERY_SESSION,
            "candidates/blocker.json",
            &candidate_bytes,
        );
        let disposition_artifact = artifact(
            VERIFIER_SESSION,
            "dispositions/blocker.json",
            &disposition_bytes,
        );
        let candidate = CandidateRecord::new(
            CandidateId::parse(CANDIDATE).unwrap(),
            attempt_id.clone(),
            SemanticFindingIdentity::new(
                "terminal evidence is atomic",
                "a partial terminal suffix becomes visible",
                "csa-session convergence store",
                "atomic publication violation",
            )
            .unwrap(),
            candidate_artifact.clone(),
        );
        let attempt = DiscoveryAttemptRecord::new(
            attempt_id.clone(),
            epoch.id().clone(),
            cell.id().clone(),
            Utc.with_ymd_and_hms(2026, 7, 14, 12, 1, 0).unwrap(),
            ProviderTurnCompletion::Natural,
            model(),
            discovery_artifact.clone(),
            8,
            1,
            false,
            Vec::new(),
        )
        .unwrap();
        let disposition = CandidateDispositionRecord::new(
            candidate.id().clone(),
            CandidateDisposition::Verified,
            CandidateVerificationEvidence::new(
                epoch.id().clone(),
                model(),
                VerificationIndependence::degraded(
                    "the immutable test authority admits one executor",
                )
                .unwrap(),
                disposition_artifact.clone(),
            ),
        );
        let disposition_digest =
            CandidateDispositionRecord::set_digest(std::slice::from_ref(&disposition));
        let cluster = RootClusterRecord::new(
            epoch.id().clone(),
            "terminal publication must be atomic",
            vec![candidate.id().clone()],
            disposition_digest.clone(),
        )
        .unwrap();
        let batch = RepairBatchRecord::new(
            cluster.id().clone(),
            cluster.content_digest().clone(),
            epoch.id().clone(),
            vec![candidate.id().clone()],
            disposition_digest,
            vec!["publish the terminal pair in one append".to_string()],
            vec!["inject a failure before the publication rename".to_string()],
            Vec::new(),
            vec!["preserve B1-B4 ledger readers".to_string()],
            Vec::new(),
        )
        .unwrap();
        let cleanup_lease = WorkspaceLeaseIdentity::new(
            campaign_id.clone(),
            epoch.clone(),
            1,
            PathBuf::from("/attestation-fixture-lease"),
            1,
            2,
            Ulid::new().to_string(),
        )
        .unwrap();
        let final_gate_authority_digest = digest('e');
        let completion_authorization = CompletionAuthorizationRecord::new(
            campaign_id.clone(),
            &epoch,
            1,
            model(),
            policy_digest.clone(),
            final_gate_authority_digest.clone(),
            cleanup_lease.clone(),
        )
        .unwrap();
        let prefix_events = vec![
            ConvergenceEvent::CampaignStarted(campaign.clone()),
            ConvergenceEvent::EpochOpened(epoch.clone()),
            ConvergenceEvent::CoverageCellDefined(cell.clone()),
            ConvergenceEvent::CoverageDispositionRecorded(
                CoverageDispositionRecord::new(
                    cell.id().clone(),
                    CoverageRequirement::Required,
                    "review_policy",
                    "The frozen review policy requires correctness coverage.",
                )
                .unwrap(),
            ),
            ConvergenceEvent::CoveragePlanFinalized(CoveragePlanFinalizationRecord::new(
                epoch.id().clone(),
            )),
            ConvergenceEvent::DiscoveryAttemptRecorded(attempt),
            ConvergenceEvent::CandidateRecorded(candidate),
            ConvergenceEvent::DiscoveryAttemptFinalized(DiscoveryAttemptFinalizationRecord::new(
                attempt_id,
            )),
            ConvergenceEvent::CandidateDispositionRecorded(disposition),
            ConvergenceEvent::RootClusterRecorded(cluster),
            ConvergenceEvent::RepairBatchRecorded(batch),
            ConvergenceEvent::CompletionAuthorizationRecorded(completion_authorization),
        ];
        let mut ledger = ConvergenceLedger::empty();
        ledger
            .append_batch(campaign_id.clone(), prefix_events.clone())
            .unwrap();

        let gate_bytes = format!(r#"{{"schema":"{GATE_SCHEMA}","passed":true}}"#).into_bytes();
        let gate_artifact = artifact(GATE_SESSION, "gates/final.json", &gate_bytes);
        let gate = GateEvidenceRecord::new(
            campaign_id.clone(),
            &epoch,
            policy_digest.clone(),
            command_authority.digest(),
            final_gate_authority_digest,
            vec![
                GateCommandResult::new("cargo test -p csa-session convergence_", 0).unwrap(),
                GateCommandResult::new(
                    "cargo clippy -p csa-session --all-targets -- -D warnings",
                    0,
                )
                .unwrap(),
            ],
            gate_artifact.clone(),
        )
        .unwrap();
        let terminal_action_id = CompletionActionId::generate();
        let terminal_execution_binding = TerminalExecutionBinding::new(
            campaign_id.clone(),
            epoch.id().clone(),
            terminal_action_id.clone(),
            1,
            cleanup_lease.generation(),
        )
        .unwrap();
        let review_artifact = artifact(REVIEW_SESSION, "review/final.json", &review_bytes);
        let review = CleanRoomReviewRecord::new(
            campaign_id.clone(),
            &epoch,
            model_evidence(),
            CleanRoomReviewArtifactBindings::new(gate_artifact.clone(), review_artifact.clone()),
            0,
            0,
            0,
        )
        .unwrap();
        let artifacts = [
            (discovery_artifact, discovery_bytes),
            (candidate_artifact, candidate_bytes),
            (disposition_artifact, disposition_bytes),
            (gate_artifact, gate_bytes),
            (review_artifact, review_bytes),
        ]
        .into_iter()
        .map(|(reference, bytes)| (artifact_key(&reference), bytes))
        .collect();
        Self {
            campaign_id,
            epoch,
            prefix_events,
            ledger,
            gate,
            review,
            cleanup_lease,
            policy_digest,
            terminal_execution_binding,
            terminal_action_id,
            artifacts,
        }
    }

    pub(crate) fn terminal_pair(&self) -> (CleanRoomReviewRecord, MergeAttestationRecord) {
        let bindings =
            compute_attestation_bindings(&self.ledger, &self.campaign_id, &self.gate, &self.review)
                .unwrap();
        let attestation = MergeAttestationRecord::new(
            &self.gate,
            &self.review,
            CleanupConfirmation::after_successful_cleanup(&self.cleanup_lease),
            self.terminal_execution_binding.clone(),
            bindings,
        )
        .unwrap();
        (self.review.clone(), attestation)
    }

    pub(crate) fn terminal_ledger(&self) -> ConvergenceLedger {
        let (review, attestation) = self.terminal_pair();
        let mut ledger = self.ledger.clone();
        ledger
            .append_batch(
                self.campaign_id.clone(),
                vec![
                    ConvergenceEvent::FinalReviewRecorded(Box::new(review)),
                    ConvergenceEvent::MergeAttestationRecorded(Box::new(attestation)),
                ],
            )
            .unwrap();
        ledger
    }

    pub(crate) fn read_artifact(&self, reference: &ArtifactEvidenceRef) -> Result<Vec<u8>> {
        self.artifacts
            .get(&artifact_key(reference))
            .cloned()
            .with_context(|| format!("missing test artifact {}", reference.path()))
    }
}

#[test]
fn attestation_hashes_bind_every_accepted_ledger_set() {
    let fixture = Fixture::new();
    let bindings = compute_attestation_bindings(
        &fixture.ledger,
        &fixture.campaign_id,
        &fixture.gate,
        &fixture.review,
    )
    .unwrap();
    let value = serde_json::to_value(&bindings).unwrap();
    let fields = value
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(fields.len(), 14, "every required binding must be explicit");

    for field in fields {
        let mut changed = value.clone();
        changed[&field] = json!(digest('f'));
        let changed: AttestationBindingDigests = serde_json::from_value(changed).unwrap();
        let changed_record = MergeAttestationRecord::new(
            &fixture.gate,
            &fixture.review,
            CleanupConfirmation::after_successful_cleanup(&fixture.cleanup_lease),
            fixture.terminal_execution_binding.clone(),
            changed,
        )
        .unwrap();
        let mut ledger = fixture.ledger.clone();
        let error = ledger
            .append_batch(
                fixture.campaign_id.clone(),
                vec![
                    ConvergenceEvent::FinalReviewRecorded(Box::new(fixture.review.clone())),
                    ConvergenceEvent::MergeAttestationRecorded(Box::new(changed_record)),
                ],
            )
            .expect_err("a changed binding must fail closed");
        assert!(error.to_string().contains("bind"), "{field}: {error:#}");
    }

    let reversed_gate = GateEvidenceRecord::new(
        fixture.campaign_id.clone(),
        &fixture.epoch,
        digest('d'),
        authority().digest(),
        digest('e'),
        fixture.gate.commands().iter().cloned().rev().collect(),
        fixture.gate.artifact().clone(),
    )
    .unwrap();
    let reversed = compute_attestation_bindings(
        &fixture.ledger,
        &fixture.campaign_id,
        &reversed_gate,
        &fixture.review,
    )
    .unwrap();
    assert_ne!(bindings, reversed);
}

#[test]
fn incomplete_authoritative_sets_and_gate_evidence_are_rejected() {
    #[derive(Clone, Copy)]
    enum Omission {
        Coverage,
        CandidateDisposition,
        Cluster,
        Batch,
    }

    let fixture = Fixture::new();
    for omission in [
        Omission::Coverage,
        Omission::CandidateDisposition,
        Omission::Cluster,
        Omission::Batch,
    ] {
        let events = fixture
            .prefix_events
            .iter()
            .filter(|event| {
                !matches!(
                    (omission, event),
                    (
                        Omission::Coverage,
                        ConvergenceEvent::CoveragePlanFinalized(_)
                    ) | (
                        Omission::CandidateDisposition,
                        ConvergenceEvent::CandidateDispositionRecorded(_)
                            | ConvergenceEvent::RootClusterRecorded(_)
                            | ConvergenceEvent::RepairBatchRecorded(_)
                    ) | (
                        Omission::Cluster,
                        ConvergenceEvent::RootClusterRecorded(_)
                            | ConvergenceEvent::RepairBatchRecorded(_)
                    ) | (Omission::Batch, ConvergenceEvent::RepairBatchRecorded(_))
                )
            })
            .cloned()
            .collect();
        let mut ledger = ConvergenceLedger::empty();
        let rejected = match ledger.append_batch(fixture.campaign_id.clone(), events) {
            Err(_) => true,
            Ok(_) => compute_attestation_bindings(
                &ledger,
                &fixture.campaign_id,
                &fixture.gate,
                &fixture.review,
            )
            .is_err(),
        };
        assert!(rejected);
    }

    assert!(
        GateEvidenceRecord::new(
            fixture.campaign_id,
            &fixture.epoch,
            digest('b'),
            authority().digest(),
            digest('e'),
            Vec::new(),
            fixture.gate.artifact().clone(),
        )
        .is_err()
    );
}

#[test]
fn attestation_rejects_mismatched_campaign_epoch_and_catalog() {
    let fixture = Fixture::new();
    let other_campaign = CampaignId::parse("01ARZ3NDEKTSV4RRFFQ69G5FC7").unwrap();
    let wrong_campaign = GateEvidenceRecord::new(
        other_campaign,
        &fixture.epoch,
        digest('d'),
        authority().digest(),
        digest('e'),
        fixture.gate.commands().to_vec(),
        fixture.gate.artifact().clone(),
    )
    .unwrap();
    assert!(
        compute_attestation_bindings(
            &fixture.ledger,
            &fixture.campaign_id,
            &wrong_campaign,
            &fixture.review,
        )
        .is_err()
    );

    let changed_epoch = EpochRecord::new(oid('a'), oid('e'), digest('c'));
    let wrong_epoch = CleanRoomReviewRecord::new(
        fixture.campaign_id.clone(),
        &changed_epoch,
        model_evidence(),
        CleanRoomReviewArtifactBindings::new(
            fixture.gate.artifact().clone(),
            fixture.review.artifact().clone(),
        ),
        0,
        0,
        0,
    )
    .unwrap();
    assert!(
        compute_attestation_bindings(
            &fixture.ledger,
            &fixture.campaign_id,
            &fixture.gate,
            &wrong_epoch,
        )
        .is_err()
    );

    let mut value = serde_json::to_value(
        compute_attestation_bindings(
            &fixture.ledger,
            &fixture.campaign_id,
            &fixture.gate,
            &fixture.review,
        )
        .unwrap(),
    )
    .unwrap();
    value["command_catalog"] = json!(digest('e'));
    let changed: AttestationBindingDigests = serde_json::from_value(value).unwrap();
    let attestation = MergeAttestationRecord::new(
        &fixture.gate,
        &fixture.review,
        CleanupConfirmation::after_successful_cleanup(&fixture.cleanup_lease),
        fixture.terminal_execution_binding.clone(),
        changed,
    )
    .unwrap();
    let mut ledger = fixture.ledger.clone();
    assert!(
        ledger
            .append_batch(
                fixture.campaign_id.clone(),
                vec![
                    ConvergenceEvent::FinalReviewRecorded(Box::new(fixture.review)),
                    ConvergenceEvent::MergeAttestationRecorded(Box::new(attestation)),
                ],
            )
            .is_err()
    );
}

#[test]
fn attestation_reader_rejects_missing_tampered_or_invalid_schema_artifacts() {
    let fixture = Fixture::new();
    let ledger = fixture.terminal_ledger();
    verify_merge_attestation(
        &ledger,
        &fixture.campaign_id,
        &|reference: &ArtifactEvidenceRef| fixture.read_artifact(reference),
    )
    .unwrap();

    assert!(
        verify_merge_attestation(&ledger, &fixture.campaign_id, &|_: &ArtifactEvidenceRef| {
            Err(anyhow!("missing artifact"))
        })
        .is_err()
    );
    assert!(
        verify_merge_attestation(
            &ledger,
            &fixture.campaign_id,
            &|reference: &ArtifactEvidenceRef| {
                if reference == fixture.gate.artifact() {
                    Ok(b"tampered".to_vec())
                } else {
                    fixture.read_artifact(reference)
                }
            }
        )
        .is_err()
    );

    let invalid = Fixture::with_review_bytes(br#"{"schema":"wrong/v1"}"#.to_vec());
    let invalid_ledger = invalid.terminal_ledger();
    assert!(
        verify_merge_attestation(
            &invalid_ledger,
            &invalid.campaign_id,
            &|reference: &ArtifactEvidenceRef| { invalid.read_artifact(reference) }
        )
        .is_err()
    );
}

#[test]
fn events_after_merge_attestation_are_rejected() {
    let fixture = Fixture::new();
    let mut ledger = fixture.terminal_ledger();
    let before = ledger.clone();
    let later_epoch = EpochRecord::new(oid('a'), oid('e'), digest('f'));
    assert!(
        ledger
            .append(
                fixture.campaign_id,
                ConvergenceEvent::EpochOpened(later_epoch),
            )
            .is_err()
    );
    assert_eq!(ledger, before);
}

#[test]
fn historical_b1_b3_b4_prefixes_still_deserialize_and_validate() {
    let fixture = Fixture::new();
    let serialized = serde_json::to_value(&fixture.ledger).unwrap();
    for end in [2_usize, 9, fixture.ledger.entries().len()] {
        let mut prefix = serialized.clone();
        prefix["entries"] = Value::Array(serialized["entries"].as_array().unwrap()[..end].to_vec());
        let decoded: ConvergenceLedger = serde_json::from_value(prefix).unwrap();
        decoded.validate().unwrap();
        assert_eq!(decoded.entries().len(), end);
    }
}

#[test]
fn nonzero_or_unpaired_final_review_is_rejected() {
    let fixture = Fixture::new();
    assert!(
        CleanRoomReviewRecord::new(
            fixture.campaign_id.clone(),
            &fixture.epoch,
            model_evidence(),
            CleanRoomReviewArtifactBindings::new(
                fixture.gate.artifact().clone(),
                fixture.review.artifact().clone(),
            ),
            1,
            0,
            0,
        )
        .is_err()
    );
    let mut ledger = fixture.ledger;
    assert!(
        ledger
            .append(
                fixture.campaign_id,
                ConvergenceEvent::FinalReviewRecorded(Box::new(fixture.review)),
            )
            .is_err()
    );
}
