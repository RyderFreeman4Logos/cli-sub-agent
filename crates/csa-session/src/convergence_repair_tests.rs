use chrono::{TimeZone, Utc};
use csa_process::ProviderTurnCompletion;

use crate::convergence::{
    AdmittedModelIdentity, ArtifactEvidenceRef, CampaignId, CampaignRecord, CandidateDisposition,
    CandidateDispositionRecord, CandidateId, CandidateRecord, CandidateVerificationEvidence,
    ConvergenceEvent, ConvergenceLedger, CoverageCellRecord, CoverageDispositionRecord,
    CoveragePlanFinalizationRecord, CoverageRequirement, CoverageScope, CsaSessionId,
    DiscoveryAttemptFinalizationRecord, DiscoveryAttemptId, DiscoveryAttemptRecord, EpochRecord,
    GitObjectId, RepairBatchRecord, RootClusterRecord, SemanticFindingIdentity, SemanticLens,
    SessionRelativeArtifactPath, Sha256Digest, VerificationIndependence,
};

fn digest(fill: char) -> Sha256Digest {
    Sha256Digest::parse(&format!("sha256:{}", fill.to_string().repeat(64)))
        .expect("test digest should be valid")
}

fn epoch() -> EpochRecord {
    EpochRecord::new(
        GitObjectId::parse(&"a".repeat(40)).expect("test base oid"),
        GitObjectId::parse(&"b".repeat(40)).expect("test head oid"),
        digest('c'),
    )
}

fn candidate(value: &str) -> CandidateId {
    CandidateId::parse(value).expect("test candidate id")
}

#[test]
fn repair_records_digest_complete_sets_canonically() {
    let epoch = epoch();
    let candidates = vec![
        candidate("01ARZ3NDEKTSV4RRFFQ69G5FAV"),
        candidate("01ARZ3NDEKTSV4RRFFQ69G5FAW"),
    ];
    let cluster = RootClusterRecord::new(
        epoch.id().clone(),
        "every repair handoff must be complete",
        candidates.clone(),
        digest('d'),
    )
    .expect("cluster should be valid");
    let same_cluster = RootClusterRecord::new(
        epoch.id().clone(),
        "every repair handoff must be complete",
        candidates.iter().cloned().rev().collect(),
        digest('d'),
    )
    .expect("cluster should be stable");
    assert_eq!(
        cluster.candidate_set_digest(),
        same_cluster.candidate_set_digest()
    );
    assert_eq!(cluster.content_digest(), same_cluster.content_digest());
    let changed_disposition_cluster = RootClusterRecord::new(
        epoch.id().clone(),
        "every repair handoff must be complete",
        candidates.clone(),
        digest('e'),
    )
    .expect("changed disposition union should remain structurally valid");
    assert_ne!(
        cluster.content_digest(),
        changed_disposition_cluster.content_digest()
    );

    let batch = RepairBatchRecord::new(
        cluster.id().clone(),
        cluster.content_digest().clone(),
        epoch.id().clone(),
        candidates,
        digest('d'),
        vec!["validate the immutable handoff".to_string()],
        vec!["exercise a changed candidate union".to_string()],
        vec!["document repair authorization".to_string()],
        vec!["preserve current ledger readers".to_string()],
        vec!["audit sibling repair launches".to_string()],
    )
    .expect("batch should be valid");
    assert_ne!(cluster.candidate_set_digest(), batch.content_digest());

    assert!(
        RootClusterRecord::new(
            epoch.id().clone(),
            "every repair handoff must be complete",
            vec![candidate("01ARZ3NDEKTSV4RRFFQ69G5FAW")],
            digest('d'),
        )
        .expect("changed member should be valid")
        .candidate_set_digest()
            != cluster.candidate_set_digest()
    );
}

const CAMPAIGN: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB2";
const ATTEMPT: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB3";
const DISCOVERY_SESSION: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB4";
const VERIFIER_SESSION: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB5";
const CANDIDATE_A: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB6";
const CANDIDATE_B: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB7";

struct RepairFixture {
    ledger: ConvergenceLedger,
    campaign_id: CampaignId,
    epoch: EpochRecord,
    dispositions: Vec<CandidateDispositionRecord>,
}

impl RepairFixture {
    fn new() -> Self {
        let campaign_id = CampaignId::parse(CAMPAIGN).unwrap();
        let epoch = epoch();
        let cell = CoverageCellRecord::new(
            epoch.id().clone(),
            CoverageScope::new("crate", "csa-session").unwrap(),
            SemanticLens::new("correctness").unwrap(),
        );
        let attempt_id = DiscoveryAttemptId::parse(ATTEMPT).unwrap();
        let model = AdmittedModelIdentity::new("codex", "openai", "gpt-5.6", "high").unwrap();
        let artifact = |session_id: &str, path: &str| {
            ArtifactEvidenceRef::new(
                CsaSessionId::parse(session_id).unwrap(),
                SessionRelativeArtifactPath::new(path).unwrap(),
                Sha256Digest::compute(path.as_bytes()),
            )
        };
        let candidate_record = |id: &str, key: &str| {
            CandidateRecord::new(
                CandidateId::parse(id).unwrap(),
                attempt_id.clone(),
                SemanticFindingIdentity::new(
                    &format!("invariant {key}"),
                    &format!("failure {key}"),
                    "csa-session",
                    &format!("cause {key}"),
                )
                .unwrap(),
                artifact(DISCOVERY_SESSION, &format!("candidates/{key}.json")),
            )
        };
        let candidate_a = candidate_record(CANDIDATE_A, "a");
        let candidate_b = candidate_record(CANDIDATE_B, "b");
        let evidence = |path: &str| {
            CandidateVerificationEvidence::new(
                epoch.id().clone(),
                model.clone(),
                VerificationIndependence::degraded("the frozen test authority admits one executor")
                    .unwrap(),
                artifact(VERIFIER_SESSION, path),
            )
        };
        let dispositions = vec![
            CandidateDispositionRecord::new(
                candidate_a.id().clone(),
                CandidateDisposition::Verified,
                evidence("dispositions/a.json"),
            ),
            CandidateDispositionRecord::new(
                candidate_b.id().clone(),
                CandidateDisposition::NeedsContractOrDocumentation,
                evidence("dispositions/b.json"),
            ),
        ];
        let attempt = DiscoveryAttemptRecord::new(
            attempt_id.clone(),
            epoch.id().clone(),
            cell.id().clone(),
            Utc.with_ymd_and_hms(2026, 7, 14, 12, 1, 0).unwrap(),
            ProviderTurnCompletion::Unknown,
            model,
            artifact(DISCOVERY_SESSION, "discovery/attempt.json"),
            8,
            2,
            false,
            Vec::new(),
        )
        .unwrap();
        let events = vec![
            ConvergenceEvent::CampaignStarted(CampaignRecord::for_test(
                campaign_id.clone(),
                Utc.with_ymd_and_hms(2026, 7, 14, 12, 0, 0).unwrap(),
                None,
            )),
            ConvergenceEvent::EpochOpened(epoch.clone()),
            ConvergenceEvent::CoverageCellDefined(cell.clone()),
            ConvergenceEvent::CoverageDispositionRecorded(
                CoverageDispositionRecord::new(
                    cell.id().clone(),
                    CoverageRequirement::Required,
                    "review_policy",
                    "The frozen review policy requires this coverage cell.",
                )
                .unwrap(),
            ),
            ConvergenceEvent::CoveragePlanFinalized(CoveragePlanFinalizationRecord::new(
                epoch.id().clone(),
            )),
            ConvergenceEvent::DiscoveryAttemptRecorded(attempt),
            ConvergenceEvent::CandidateRecorded(candidate_a),
            ConvergenceEvent::CandidateRecorded(candidate_b),
            ConvergenceEvent::DiscoveryAttemptFinalized(DiscoveryAttemptFinalizationRecord::new(
                attempt_id,
            )),
            ConvergenceEvent::CandidateDispositionRecorded(dispositions[0].clone()),
            ConvergenceEvent::CandidateDispositionRecorded(dispositions[1].clone()),
        ];
        let mut ledger = ConvergenceLedger::empty();
        ledger.append_batch(campaign_id.clone(), events).unwrap();
        Self {
            ledger,
            campaign_id,
            epoch,
            dispositions,
        }
    }

    fn records_for(&self, candidate_indexes: &[usize], root: &str) -> Vec<ConvergenceEvent> {
        let candidates = candidate_indexes
            .iter()
            .map(|index| self.dispositions[*index].candidate_id().clone())
            .collect::<Vec<_>>();
        let dispositions = candidate_indexes
            .iter()
            .map(|index| self.dispositions[*index].clone())
            .collect::<Vec<_>>();
        let disposition_set_digest = CandidateDispositionRecord::set_digest(&dispositions);
        let cluster = RootClusterRecord::new(
            self.epoch.id().clone(),
            root,
            candidates.clone(),
            disposition_set_digest.clone(),
        )
        .unwrap();
        let batch = RepairBatchRecord::new(
            cluster.id().clone(),
            cluster.content_digest().clone(),
            self.epoch.id().clone(),
            candidates,
            disposition_set_digest,
            vec![format!("repair {root}")],
            vec![format!("test {root}")],
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )
        .unwrap();
        vec![
            ConvergenceEvent::RootClusterRecorded(cluster),
            ConvergenceEvent::RepairBatchRecorded(batch),
        ]
    }
}

#[test]
fn clustering_rejects_an_omitted_verified_blocking_candidate() {
    let mut fixture = RepairFixture::new();
    let before = fixture.ledger.clone();
    let error = fixture
        .ledger
        .append_batch(
            fixture.campaign_id.clone(),
            fixture.records_for(&[0], "shared root"),
        )
        .expect_err("partial clustering must not become durable");

    assert!(
        error
            .to_string()
            .contains("does not cover every verified blocking candidate")
    );
    assert_eq!(fixture.ledger, before);
}

#[test]
fn clustering_rejects_duplicate_candidate_membership_across_roots() {
    let mut fixture = RepairFixture::new();
    let before = fixture.ledger.clone();
    let mut records = fixture.records_for(&[0], "first root");
    records.extend(fixture.records_for(&[0, 1], "overlapping root"));
    let error = fixture
        .ledger
        .append_batch(fixture.campaign_id.clone(), records)
        .expect_err("one candidate cannot belong to two roots");

    assert!(error.to_string().contains("overlaps an already clustered"));
    assert_eq!(fixture.ledger, before);
}

#[test]
fn clustering_requires_exactly_one_consolidated_batch_per_root() {
    let mut fixture = RepairFixture::new();
    let before = fixture.ledger.clone();
    let mut records = fixture.records_for(&[0, 1], "shared root");
    records.pop();
    let error = fixture
        .ledger
        .append_batch(fixture.campaign_id.clone(), records)
        .expect_err("a root without its consolidated batch must not become durable");

    assert!(
        error
            .to_string()
            .contains("exactly one consolidated repair batch per cluster")
    );
    assert_eq!(fixture.ledger, before);
}

#[test]
fn disposition_set_digest_is_order_stable_and_evidence_sensitive() {
    let fixture = RepairFixture::new();
    let reversed = fixture
        .dispositions
        .iter()
        .cloned()
        .rev()
        .collect::<Vec<_>>();
    assert_eq!(
        CandidateDispositionRecord::set_digest(&fixture.dispositions),
        CandidateDispositionRecord::set_digest(&reversed)
    );

    let changed = CandidateDispositionRecord::new(
        fixture.dispositions[0].candidate_id().clone(),
        CandidateDisposition::Verified,
        CandidateVerificationEvidence::new(
            fixture.epoch.id().clone(),
            AdmittedModelIdentity::new("codex", "openai", "gpt-5.6", "high").unwrap(),
            VerificationIndependence::degraded("the frozen test authority admits one executor")
                .unwrap(),
            ArtifactEvidenceRef::new(
                CsaSessionId::parse(VERIFIER_SESSION).unwrap(),
                SessionRelativeArtifactPath::new("dispositions/changed.json").unwrap(),
                Sha256Digest::compute(b"changed verifier evidence"),
            ),
        ),
    );
    let changed_set = vec![changed, fixture.dispositions[1].clone()];
    assert_ne!(
        CandidateDispositionRecord::set_digest(&fixture.dispositions),
        CandidateDispositionRecord::set_digest(&changed_set)
    );
}
