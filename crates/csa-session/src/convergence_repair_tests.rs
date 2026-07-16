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
    authorize_consolidated_repairs,
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
const CLEAN_ATTEMPT: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB8";
const DISCOVERY_SESSION: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB4";
const CLEAN_DISCOVERY_SESSION: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB9";
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
        let clean_attempt_id = DiscoveryAttemptId::parse(CLEAN_ATTEMPT).unwrap();
        let clean_attempt = DiscoveryAttemptRecord::new(
            clean_attempt_id.clone(),
            epoch.id().clone(),
            cell.id().clone(),
            Utc.with_ymd_and_hms(2026, 7, 14, 12, 2, 0).unwrap(),
            ProviderTurnCompletion::Natural,
            AdmittedModelIdentity::new("codex", "openai", "gpt-5.6", "high").unwrap(),
            artifact(CLEAN_DISCOVERY_SESSION, "discovery/clean.json"),
            8,
            0,
            false,
            Vec::new(),
        )
        .unwrap();
        let events = vec![
            ConvergenceEvent::CampaignStarted(CampaignRecord::for_test(
                campaign_id.clone(),
                Utc.with_ymd_and_hms(2026, 7, 14, 12, 0, 0).unwrap(),
                Some(digest('f')),
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
            ConvergenceEvent::DiscoveryAttemptRecorded(clean_attempt),
            ConvergenceEvent::DiscoveryAttemptFinalized(DiscoveryAttemptFinalizationRecord::new(
                clean_attempt_id,
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
fn clustering_rejects_candidates_from_a_prior_epoch() {
    let mut fixture = RepairFixture::new();
    let later_epoch = EpochRecord::new(
        GitObjectId::parse(&"a".repeat(40)).expect("test base oid"),
        GitObjectId::parse(&"d".repeat(40)).expect("test head oid"),
        digest('e'),
    );
    let candidates = fixture
        .dispositions
        .iter()
        .map(|disposition| disposition.candidate_id().clone())
        .collect::<Vec<_>>();
    let disposition_set_digest = CandidateDispositionRecord::set_digest(&fixture.dispositions);
    let cluster = RootClusterRecord::new(
        later_epoch.id().clone(),
        "must not splice old candidate evidence into a new repair epoch",
        candidates.clone(),
        disposition_set_digest.clone(),
    )
    .expect("well-formed but cross-epoch root cluster");
    let batch = RepairBatchRecord::new(
        cluster.id().clone(),
        cluster.content_digest().clone(),
        later_epoch.id().clone(),
        candidates,
        disposition_set_digest,
        vec!["reject cross-epoch repair evidence".to_string()],
        vec!["regress epoch splicing".to_string()],
        Vec::new(),
        Vec::new(),
        Vec::new(),
    )
    .expect("well-formed but cross-epoch repair batch");

    let error = fixture
        .ledger
        .append_batch(
            fixture.campaign_id.clone(),
            vec![
                ConvergenceEvent::EpochOpened(later_epoch),
                ConvergenceEvent::RootClusterRecorded(cluster),
                ConvergenceEvent::RepairBatchRecorded(batch),
            ],
        )
        .expect_err("cross-epoch root evidence must not become durable");

    assert!(
        error
            .to_string()
            .contains("discovery attempt belongs to epoch")
    );
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

#[test]
fn complete_current_epoch_authorizes_one_consolidated_batch_per_root() {
    let mut fixture = RepairFixture::new();
    fixture
        .ledger
        .append_batch(
            fixture.campaign_id.clone(),
            fixture.records_for(&[0, 1], "shared root"),
        )
        .unwrap();

    let authorization =
        authorize_consolidated_repairs(&fixture.ledger, &fixture.campaign_id).unwrap();

    assert_eq!(authorization.epoch().id(), fixture.epoch.id());
    assert_eq!(authorization.batches().len(), 1);
    assert_eq!(
        authorization.batches()[0].corrections(),
        ["repair shared root"]
    );
}

#[test]
fn opening_changed_head_epoch_invalidates_prior_authorization() {
    let mut fixture = RepairFixture::new();
    fixture
        .ledger
        .append_batch(
            fixture.campaign_id.clone(),
            fixture.records_for(&[0, 1], "shared root"),
        )
        .unwrap();
    let changed_epoch = EpochRecord::new(
        fixture.epoch.base_oid().clone(),
        GitObjectId::parse(&"d".repeat(40)).unwrap(),
        digest('e'),
    );
    fixture
        .ledger
        .append(
            fixture.campaign_id.clone(),
            ConvergenceEvent::EpochOpened(changed_epoch),
        )
        .unwrap();

    let error = authorize_consolidated_repairs(&fixture.ledger, &fixture.campaign_id)
        .expect_err("a new epoch must invalidate every earlier authorization");

    assert!(error.to_string().contains("current epoch"));
}

#[test]
fn incomplete_wrong_campaign_and_workspace_evidence_fail_closed() {
    let fixture = RepairFixture::new();
    let before = fixture.ledger.clone();
    let error = authorize_consolidated_repairs(&fixture.ledger, &fixture.campaign_id)
        .expect_err("missing clusters and batches must not authorize repair");
    assert!(error.to_string().contains("no complete consolidated"));
    assert_eq!(fixture.ledger, before);

    let wrong_campaign = CampaignId::parse("01ARZ3NDEKTSV4RRFFQ69G5FBB").unwrap();
    assert!(authorize_consolidated_repairs(&fixture.ledger, &wrong_campaign).is_err());

    let mut complete = RepairFixture::new();
    complete
        .ledger
        .append_batch(
            complete.campaign_id.clone(),
            complete.records_for(&[0, 1], "shared root"),
        )
        .unwrap();
    let authorization =
        authorize_consolidated_repairs(&complete.ledger, &complete.campaign_id).unwrap();
    let changed = EpochRecord::new(
        complete.epoch.base_oid().clone(),
        GitObjectId::parse(&"d".repeat(40)).unwrap(),
        digest('e'),
    );
    assert!(authorization.validate_observed_epoch(&changed).is_err());
}

#[test]
fn multiple_roots_authorize_exactly_one_nonduplicable_handoff_each() {
    let mut fixture = RepairFixture::new();
    let mut records = fixture.records_for(&[0], "root a");
    records.extend(fixture.records_for(&[1], "root b"));
    fixture
        .ledger
        .append_batch(fixture.campaign_id.clone(), records)
        .unwrap();
    let authorization =
        authorize_consolidated_repairs(&fixture.ledger, &fixture.campaign_id).unwrap();
    assert_eq!(authorization.batches().len(), 2);

    let actual = AdmittedModelIdentity::new("codex", "openai", "gpt-5.6", "high").unwrap();
    let artifact = ArtifactEvidenceRef::new(
        CsaSessionId::parse("01ARZ3NDEKTSV4RRFFQ69G5FBC").unwrap(),
        SessionRelativeArtifactPath::new("output/repair-handoff.json").unwrap(),
        Sha256Digest::compute(b"one actual writer receipt"),
    );
    let handoffs = authorization
        .batches()
        .iter()
        .map(|batch| {
            authorization
                .handoff_for(batch.id(), actual.clone(), artifact.clone())
                .map(ConvergenceEvent::RepairHandoffRecorded)
        })
        .collect::<anyhow::Result<Vec<_>>>()
        .unwrap();
    fixture
        .ledger
        .append_batch(fixture.campaign_id.clone(), handoffs)
        .expect("one handoff per root must be accepted atomically");

    let duplicate = authorization
        .handoff_for(authorization.batches()[0].id(), actual, artifact)
        .unwrap();
    assert!(
        fixture
            .ledger
            .append(
                fixture.campaign_id.clone(),
                ConvergenceEvent::RepairHandoffRecorded(duplicate),
            )
            .is_err(),
        "a second writer handoff for one root batch must fail closed"
    );
}

#[test]
fn handoff_requires_actual_executor_membership_and_complete_evidence() {
    let mut fixture = RepairFixture::new();
    fixture
        .ledger
        .append_batch(
            fixture.campaign_id.clone(),
            fixture.records_for(&[0, 1], "shared root"),
        )
        .unwrap();
    let authorization =
        authorize_consolidated_repairs(&fixture.ledger, &fixture.campaign_id).unwrap();
    let batch = &authorization.batches()[0];
    let artifact = ArtifactEvidenceRef::new(
        CsaSessionId::parse("01ARZ3NDEKTSV4RRFFQ69G5FBA").unwrap(),
        SessionRelativeArtifactPath::new("output/repair-handoff.json").unwrap(),
        Sha256Digest::compute(b"actual writer receipt"),
    );
    let requested_but_not_actual =
        AdmittedModelIdentity::new("codex", "other", "unadmitted", "high").unwrap();

    let error = authorization
        .handoff_for(batch.id(), requested_but_not_actual, artifact.clone())
        .expect_err("requested routing is not actual executor evidence");
    assert!(error.to_string().contains("actual repair executor"));

    let actual = AdmittedModelIdentity::new("codex", "openai", "gpt-5.6", "high").unwrap();
    let handoff = authorization
        .handoff_for(batch.id(), actual.clone(), artifact)
        .unwrap();
    assert_eq!(handoff.actual_executor(), &actual);
    fixture
        .ledger
        .append(
            fixture.campaign_id.clone(),
            ConvergenceEvent::RepairHandoffRecorded(handoff),
        )
        .expect("actual executor evidence should become durable once");
    let error = authorize_consolidated_repairs(&fixture.ledger, &fixture.campaign_id)
        .expect_err("durable handoff consumes the current authorization");
    assert!(error.to_string().contains("already-used"));
}
