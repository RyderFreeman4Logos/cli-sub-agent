use chrono::{TimeZone, Utc};
use csa_process::ProviderTurnCompletion;
use serde_json::json;

use crate::convergence::{
    AdmittedModelIdentity, ArtifactEvidenceRef, CampaignId, CampaignRecord, CandidateDisposition,
    CandidateDispositionRecord, CandidateId, CandidateRecord, CandidateVerificationEvidence,
    ConvergenceEvent, ConvergenceLedger, CoverageCellRecord, CoverageDispositionRecord,
    CoveragePlanFinalizationRecord, CoverageRequirement, CoverageScope, CsaSessionId,
    DiscoveryAttemptFinalizationRecord, DiscoveryAttemptId, DiscoveryAttemptRecord, EpochRecord,
    GitObjectId, SemanticFindingIdentity, SemanticLens, SessionRelativeArtifactPath, Sha256Digest,
    VerificationIndependence,
};

const CAMPAIGN: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const SESSION_A: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
const SESSION_B: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAX";
const ATTEMPT_A: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAY";
const ATTEMPT_B: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAZ";
const CANDIDATE_A: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB0";
const CANDIDATE_B: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB1";
const CANDIDATE_C: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB2";

fn campaign_id() -> CampaignId {
    CampaignId::parse(CAMPAIGN).unwrap()
}

fn session(value: &str) -> CsaSessionId {
    CsaSessionId::parse(value).unwrap()
}

fn digest(fill: char) -> Sha256Digest {
    Sha256Digest::parse(&format!("sha256:{}", fill.to_string().repeat(64))).unwrap()
}

fn oid(fill: char) -> GitObjectId {
    GitObjectId::parse(&fill.to_string().repeat(40)).unwrap()
}

fn artifact(session_id: &str, path: &str, bytes: &[u8]) -> ArtifactEvidenceRef {
    ArtifactEvidenceRef::new(
        session(session_id),
        SessionRelativeArtifactPath::new(path).unwrap(),
        Sha256Digest::compute(bytes),
    )
}

fn model() -> AdmittedModelIdentity {
    AdmittedModelIdentity::new("codex", "openai", "gpt-5.6", "high").unwrap()
}

fn identity() -> SemanticFindingIdentity {
    SemanticFindingIdentity::new(
        "state transitions must be checked",
        "unchecked transition",
        "csa-session",
        "missing guard",
    )
    .unwrap()
}

#[derive(Clone)]
struct Fixture {
    campaign_id: CampaignId,
    epoch: EpochRecord,
    cell: CoverageCellRecord,
}

impl Fixture {
    fn new() -> Self {
        let epoch = EpochRecord::new(oid('a'), oid('b'), digest('c'));
        let cell = CoverageCellRecord::new(
            epoch.id().clone(),
            CoverageScope::new("crate", "csa-session").unwrap(),
            SemanticLens::new("correctness").unwrap(),
        );
        Self {
            campaign_id: campaign_id(),
            epoch,
            cell,
        }
    }

    fn append_open_cell(&self, ledger: &mut ConvergenceLedger) {
        ledger
            .append(
                self.campaign_id.clone(),
                ConvergenceEvent::CampaignStarted(CampaignRecord::for_test(
                    self.campaign_id.clone(),
                    Utc.with_ymd_and_hms(2026, 7, 14, 12, 0, 0).unwrap(),
                    None,
                )),
            )
            .unwrap();
        ledger
            .append(
                self.campaign_id.clone(),
                ConvergenceEvent::EpochOpened(self.epoch.clone()),
            )
            .unwrap();
        ledger
            .append(
                self.campaign_id.clone(),
                ConvergenceEvent::CoverageCellDefined(self.cell.clone()),
            )
            .unwrap();
    }

    fn append_plan(&self, ledger: &mut ConvergenceLedger, requirement: CoverageRequirement) {
        self.append_open_cell(ledger);
        ledger
            .append(
                self.campaign_id.clone(),
                ConvergenceEvent::CoverageDispositionRecorded(
                    CoverageDispositionRecord::new(
                        self.cell.id().clone(),
                        requirement,
                        "review_policy",
                        "The frozen review policy determines this requirement.",
                    )
                    .unwrap(),
                ),
            )
            .unwrap();
        ledger
            .append(
                self.campaign_id.clone(),
                ConvergenceEvent::CoveragePlanFinalized(CoveragePlanFinalizationRecord::new(
                    self.epoch.id().clone(),
                )),
            )
            .unwrap();
    }

    fn attempt(&self, id: &str, count: u32, session_id: &str) -> DiscoveryAttemptRecord {
        DiscoveryAttemptRecord::new(
            DiscoveryAttemptId::parse(id).unwrap(),
            self.epoch.id().clone(),
            self.cell.id().clone(),
            Utc.with_ymd_and_hms(2026, 7, 14, 12, 1, 0).unwrap(),
            ProviderTurnCompletion::Unknown,
            model(),
            artifact(session_id, "discovery/attempt.json", id.as_bytes()),
            8,
            count,
            false,
            Vec::new(),
        )
        .unwrap()
    }

    fn candidate(
        &self,
        id: &str,
        attempt_id: &DiscoveryAttemptId,
        session_id: &str,
    ) -> CandidateRecord {
        CandidateRecord::new(
            CandidateId::parse(id).unwrap(),
            attempt_id.clone(),
            identity(),
            artifact(session_id, &format!("candidates/{id}.json"), id.as_bytes()),
        )
    }
}

fn verification(
    fixture: &Fixture,
    session_id: &str,
    path: &str,
    bytes: &[u8],
) -> CandidateVerificationEvidence {
    CandidateVerificationEvidence::new(
        fixture.epoch.id().clone(),
        model(),
        VerificationIndependence::degraded("the frozen test authority admits one executor")
            .unwrap(),
        artifact(session_id, path, bytes),
    )
}

fn append_attempt(
    ledger: &mut ConvergenceLedger,
    fixture: &Fixture,
    attempt: DiscoveryAttemptRecord,
) {
    ledger
        .append(
            fixture.campaign_id.clone(),
            ConvergenceEvent::DiscoveryAttemptRecorded(attempt),
        )
        .unwrap();
}

fn finalize_attempt(
    ledger: &mut ConvergenceLedger,
    fixture: &Fixture,
    attempt_id: DiscoveryAttemptId,
) -> anyhow::Result<()> {
    ledger.append(
        fixture.campaign_id.clone(),
        ConvergenceEvent::DiscoveryAttemptFinalized(DiscoveryAttemptFinalizationRecord::new(
            attempt_id,
        )),
    )
}

#[test]
fn convergence_protocol_artifact_provenance_is_self_contained_and_cross_checked() {
    let evidence_ref = artifact(SESSION_A, "discovery/evidence.json", b"evidence");
    let value = serde_json::to_value(&evidence_ref).unwrap();
    assert_eq!(value["csa_session_id"], SESSION_A);
    assert_eq!(evidence_ref.csa_session_id(), &session(SESSION_A));
    assert_eq!(
        serde_json::from_value::<ArtifactEvidenceRef>(value.clone()).unwrap(),
        evidence_ref
    );

    let mut invalid_session = value;
    invalid_session["csa_session_id"] = json!("invalid");
    assert!(serde_json::from_value::<ArtifactEvidenceRef>(invalid_session).is_err());

    let fixture = Fixture::new();
    let mut ledger = ConvergenceLedger::empty();
    fixture.append_plan(&mut ledger, CoverageRequirement::Required);
    let attempt = fixture.attempt(ATTEMPT_A, 1, SESSION_A);
    let attempt_id = attempt.id().clone();
    append_attempt(&mut ledger, &fixture, attempt);

    let before = ledger.clone();
    assert!(
        ledger
            .append(
                fixture.campaign_id.clone(),
                ConvergenceEvent::CandidateRecorded(fixture.candidate(
                    CANDIDATE_A,
                    &attempt_id,
                    SESSION_B,
                )),
            )
            .is_err()
    );
    assert_eq!(ledger, before);

    ledger
        .append(
            fixture.campaign_id.clone(),
            ConvergenceEvent::CandidateRecorded(fixture.candidate(
                CANDIDATE_A,
                &attempt_id,
                SESSION_A,
            )),
        )
        .unwrap();
    finalize_attempt(&mut ledger, &fixture, attempt_id).unwrap();
    let disposition = CandidateDispositionRecord::new(
        CandidateId::parse(CANDIDATE_A).unwrap(),
        CandidateDisposition::Verified,
        verification(
            &fixture,
            SESSION_B,
            "dispositions/verifier.json",
            b"verified elsewhere",
        ),
    );
    ledger
        .append(
            fixture.campaign_id.clone(),
            ConvergenceEvent::CandidateDispositionRecorded(disposition.clone()),
        )
        .unwrap();
    assert_eq!(disposition.artifact().csa_session_id(), &session(SESSION_B));
    ledger.validate().unwrap();
}

#[test]
fn convergence_protocol_nul_inputs_are_rejected_and_hash_fields_are_framed() {
    assert!(SessionRelativeArtifactPath::new("discovery/a\0b.json").is_err());
    assert!(
        serde_json::from_value::<SessionRelativeArtifactPath>(json!("discovery/a\0b.json"))
            .is_err()
    );
    assert!(AdmittedModelIdentity::new("codex\0x", "openai", "gpt", "high").is_err());
    assert!(CoverageScope::new("crate", "csa\0session").is_err());
    assert!(SemanticLens::new("correctness\0security").is_err());
    assert!(SemanticFindingIdentity::new("a", "b", "c\0d", "e").is_err());

    let fixture = Fixture::new();
    assert!(
        DiscoveryAttemptRecord::new(
            DiscoveryAttemptId::parse(ATTEMPT_A).unwrap(),
            fixture.epoch.id().clone(),
            fixture.cell.id().clone(),
            Utc.with_ymd_and_hms(2026, 7, 14, 12, 1, 0).unwrap(),
            ProviderTurnCompletion::Unknown,
            model(),
            artifact(SESSION_A, "discovery/attempt.json", b"attempt"),
            1,
            0,
            false,
            vec!["src/a\0b.rs".to_string()],
        )
        .is_err()
    );

    let former_collision_a = crate::convergence::hash_fields(b"test-domain\0", &["a", "b\0c", "d"]);
    let former_collision_b = crate::convergence::hash_fields(b"test-domain\0", &["a\0b", "c", "d"]);
    assert_ne!(former_collision_a, former_collision_b);
}

#[test]
fn convergence_protocol_append_builds_history_and_rolls_back_invalid_events() {
    let fixture = Fixture::new();
    let mut ledger = ConvergenceLedger::empty();
    fixture.append_plan(&mut ledger, CoverageRequirement::Required);
    let attempt = fixture.attempt(ATTEMPT_A, 1, SESSION_A);
    let attempt_id = attempt.id().clone();
    append_attempt(&mut ledger, &fixture, attempt);
    ledger
        .append(
            fixture.campaign_id.clone(),
            ConvergenceEvent::CandidateRecorded(fixture.candidate(
                CANDIDATE_A,
                &attempt_id,
                SESSION_A,
            )),
        )
        .unwrap();
    finalize_attempt(&mut ledger, &fixture, attempt_id).unwrap();
    ledger
        .append(
            fixture.campaign_id.clone(),
            ConvergenceEvent::CandidateDispositionRecorded(CandidateDispositionRecord::new(
                CandidateId::parse(CANDIDATE_A).unwrap(),
                CandidateDisposition::Verified,
                verification(&fixture, SESSION_B, "dispositions/a.json", b"verified"),
            )),
        )
        .unwrap();

    ledger.validate().unwrap();
    assert_eq!(ledger.entries().len(), 9);
    assert_eq!(ledger.entries().last().unwrap().sequence(), 9);

    let before = ledger.clone();
    let before_json = serde_json::to_vec(&ledger).unwrap();
    assert!(
        ledger
            .append(
                fixture.campaign_id.clone(),
                ConvergenceEvent::CandidateDispositionRecorded(CandidateDispositionRecord::new(
                    CandidateId::parse(CANDIDATE_A).unwrap(),
                    CandidateDisposition::Verified,
                    verification(&fixture, SESSION_B, "dispositions/again.json", b"again"),
                )),
            )
            .is_err()
    );
    assert_eq!(ledger, before);
    assert_eq!(serde_json::to_vec(&ledger).unwrap(), before_json);
}

#[test]
fn convergence_protocol_coverage_plan_finalization_is_a_strict_boundary() {
    let fixture = Fixture::new();
    let mut missing = ConvergenceLedger::empty();
    fixture.append_open_cell(&mut missing);
    let before = missing.clone();
    assert!(
        missing
            .append(
                fixture.campaign_id.clone(),
                ConvergenceEvent::CoveragePlanFinalized(CoveragePlanFinalizationRecord::new(
                    fixture.epoch.id().clone(),
                )),
            )
            .is_err()
    );
    assert_eq!(missing, before);
    missing.validate().unwrap();

    let mut early_attempt = missing;
    early_attempt
        .append(
            fixture.campaign_id.clone(),
            ConvergenceEvent::CoverageDispositionRecorded(
                CoverageDispositionRecord::new(
                    fixture.cell.id().clone(),
                    CoverageRequirement::Required,
                    "review_policy",
                    "Required before the plan is finalized.",
                )
                .unwrap(),
            ),
        )
        .unwrap();
    assert!(
        early_attempt
            .append(
                fixture.campaign_id.clone(),
                ConvergenceEvent::DiscoveryAttemptRecorded(
                    fixture.attempt(ATTEMPT_A, 0, SESSION_A,)
                ),
            )
            .is_err()
    );

    let mut not_applicable = ConvergenceLedger::empty();
    fixture.append_plan(&mut not_applicable, CoverageRequirement::NotApplicable);
    assert!(
        not_applicable
            .append(
                fixture.campaign_id.clone(),
                ConvergenceEvent::DiscoveryAttemptRecorded(
                    fixture.attempt(ATTEMPT_A, 0, SESSION_A,)
                ),
            )
            .is_err()
    );

    let mut finalized = ConvergenceLedger::empty();
    fixture.append_plan(&mut finalized, CoverageRequirement::Required);
    let other_cell = CoverageCellRecord::new(
        fixture.epoch.id().clone(),
        CoverageScope::new("crate", "csa-process").unwrap(),
        SemanticLens::new("correctness").unwrap(),
    );
    assert!(
        finalized
            .append(
                fixture.campaign_id.clone(),
                ConvergenceEvent::CoverageCellDefined(other_cell),
            )
            .is_err()
    );
    assert!(
        finalized
            .append(
                fixture.campaign_id.clone(),
                ConvergenceEvent::CoverageDispositionRecorded(
                    CoverageDispositionRecord::new(
                        fixture.cell.id().clone(),
                        CoverageRequirement::Required,
                        "review_policy",
                        "No planning changes are accepted after finalization.",
                    )
                    .unwrap(),
                ),
            )
            .is_err()
    );
    assert!(
        finalized
            .append(
                fixture.campaign_id.clone(),
                ConvergenceEvent::CoveragePlanFinalized(CoveragePlanFinalizationRecord::new(
                    fixture.epoch.id().clone(),
                )),
            )
            .is_err()
    );
}

#[test]
fn convergence_protocol_attempt_counts_are_reconciled_before_sealing() {
    let fixture = Fixture::new();

    let mut zero = ConvergenceLedger::empty();
    fixture.append_plan(&mut zero, CoverageRequirement::Required);
    let zero_attempt = fixture.attempt(ATTEMPT_A, 0, SESSION_A);
    let zero_id = zero_attempt.id().clone();
    append_attempt(&mut zero, &fixture, zero_attempt);
    assert!(
        zero.append(
            fixture.campaign_id.clone(),
            ConvergenceEvent::CandidateRecorded(fixture.candidate(
                CANDIDATE_A,
                &zero_id,
                SESSION_A,
            )),
        )
        .is_err()
    );
    finalize_attempt(&mut zero, &fixture, zero_id).unwrap();

    let mut overflow = ConvergenceLedger::empty();
    fixture.append_plan(&mut overflow, CoverageRequirement::Required);
    let one_attempt = fixture.attempt(ATTEMPT_A, 1, SESSION_A);
    let one_id = one_attempt.id().clone();
    append_attempt(&mut overflow, &fixture, one_attempt);
    overflow
        .append(
            fixture.campaign_id.clone(),
            ConvergenceEvent::CandidateRecorded(fixture.candidate(CANDIDATE_A, &one_id, SESSION_A)),
        )
        .unwrap();
    assert!(
        overflow
            .append(
                fixture.campaign_id.clone(),
                ConvergenceEvent::CandidateRecorded(fixture.candidate(
                    CANDIDATE_B,
                    &one_id,
                    SESSION_A,
                )),
            )
            .is_err()
    );
    finalize_attempt(&mut overflow, &fixture, one_id.clone()).unwrap();
    assert!(
        overflow
            .append(
                fixture.campaign_id.clone(),
                ConvergenceEvent::CandidateRecorded(fixture.candidate(
                    CANDIDATE_C,
                    &one_id,
                    SESSION_A,
                )),
            )
            .is_err()
    );

    let mut undercount = ConvergenceLedger::empty();
    fixture.append_plan(&mut undercount, CoverageRequirement::Required);
    let two_attempt = fixture.attempt(ATTEMPT_B, 2, SESSION_A);
    let two_id = two_attempt.id().clone();
    append_attempt(&mut undercount, &fixture, two_attempt);
    undercount
        .append(
            fixture.campaign_id.clone(),
            ConvergenceEvent::CandidateRecorded(fixture.candidate(CANDIDATE_A, &two_id, SESSION_A)),
        )
        .unwrap();
    assert!(finalize_attempt(&mut undercount, &fixture, two_id).is_err());
}

#[test]
fn convergence_protocol_dispositions_require_finalized_source_and_target_attempts() {
    let fixture = Fixture::new();
    let mut ledger = ConvergenceLedger::empty();
    fixture.append_plan(&mut ledger, CoverageRequirement::Required);
    let attempt_a = fixture.attempt(ATTEMPT_A, 1, SESSION_A);
    let attempt_b = fixture.attempt(ATTEMPT_B, 1, SESSION_A);
    let attempt_a_id = attempt_a.id().clone();
    let attempt_b_id = attempt_b.id().clone();
    append_attempt(&mut ledger, &fixture, attempt_a);
    append_attempt(&mut ledger, &fixture, attempt_b);
    let candidate_a = fixture.candidate(CANDIDATE_A, &attempt_a_id, SESSION_A);
    let candidate_b = fixture.candidate(CANDIDATE_B, &attempt_b_id, SESSION_A);
    ledger
        .append(
            fixture.campaign_id.clone(),
            ConvergenceEvent::CandidateRecorded(candidate_a.clone()),
        )
        .unwrap();
    ledger
        .append(
            fixture.campaign_id.clone(),
            ConvergenceEvent::CandidateRecorded(candidate_b.clone()),
        )
        .unwrap();

    let verified = || {
        ConvergenceEvent::CandidateDispositionRecorded(CandidateDispositionRecord::new(
            candidate_a.id().clone(),
            CandidateDisposition::Verified,
            verification(&fixture, SESSION_B, "dispositions/source.json", b"source"),
        ))
    };
    assert!(
        ledger
            .append(fixture.campaign_id.clone(), verified())
            .is_err()
    );
    finalize_attempt(&mut ledger, &fixture, attempt_a_id).unwrap();

    let duplicate = || {
        ConvergenceEvent::CandidateDispositionRecorded(CandidateDispositionRecord::new(
            candidate_b.id().clone(),
            CandidateDisposition::Duplicate {
                canonical_candidate_id: candidate_a.id().clone(),
            },
            verification(&fixture, SESSION_B, "dispositions/target.json", b"target"),
        ))
    };
    assert!(
        ledger
            .append(fixture.campaign_id.clone(), duplicate())
            .is_err()
    );
    finalize_attempt(&mut ledger, &fixture, attempt_b_id).unwrap();
    ledger
        .append(fixture.campaign_id.clone(), duplicate())
        .unwrap();
}

#[test]
fn convergence_protocol_finalization_records_reject_unknown_fields() {
    let fixture = Fixture::new();
    let plan = CoveragePlanFinalizationRecord::new(fixture.epoch.id().clone());
    assert_eq!(plan.epoch_id(), fixture.epoch.id());
    let mut plan_value = serde_json::to_value(&plan).unwrap();
    plan_value["future"] = json!(true);
    assert!(serde_json::from_value::<CoveragePlanFinalizationRecord>(plan_value).is_err());

    let attempt_id = DiscoveryAttemptId::parse(ATTEMPT_A).unwrap();
    let finalization = DiscoveryAttemptFinalizationRecord::new(attempt_id.clone());
    assert_eq!(finalization.discovery_attempt_id(), &attempt_id);
    let mut attempt_value = serde_json::to_value(&finalization).unwrap();
    attempt_value["future"] = json!(true);
    assert!(serde_json::from_value::<DiscoveryAttemptFinalizationRecord>(attempt_value).is_err());
}
