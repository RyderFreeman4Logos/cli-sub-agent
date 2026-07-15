use chrono::{TimeZone, Utc};
use csa_process::ProviderTurnCompletion;
use serde_json::json;

use crate::convergence::{
    AdmittedModelIdentity, ArtifactEvidenceRef, CampaignId, CampaignRecord, CandidateDisposition,
    CandidateDispositionRecord, CandidateId, CandidateRecord, CandidateVerificationEvidence,
    ConvergenceEvent, ConvergenceLedger, CoverageCellRecord, CoverageDispositionRecord,
    CoveragePlanFinalizationRecord, CoverageRequirement, CoverageScope, CsaSessionId,
    DiscoveryAttemptFinalizationRecord, DiscoveryAttemptId, DiscoveryAttemptRecord,
    DiscoveryDirective, DiscoveryRunIntent, EpochRecord, GitObjectId, SemanticFindingIdentity,
    SemanticLens, SessionRelativeArtifactPath, Sha256Digest, VerificationIndependence,
    next_discovery_directive,
};

const CAMPAIGN_A: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const CAMPAIGN_B: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
const SESSION_A: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAX";
const ATTEMPT_A: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAY";
const ATTEMPT_B: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAZ";
const ATTEMPT_C: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB0";
const CANDIDATE_A: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB1";
const CANDIDATE_B: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB2";

fn digest(fill: char) -> Sha256Digest {
    Sha256Digest::parse(&format!("sha256:{}", fill.to_string().repeat(64))).unwrap()
}

fn oid(fill: char) -> GitObjectId {
    GitObjectId::parse(&fill.to_string().repeat(40)).unwrap()
}

fn campaign(id: &str) -> CampaignRecord {
    CampaignRecord::for_test(
        CampaignId::parse(id).unwrap(),
        Utc.with_ymd_and_hms(2026, 7, 14, 12, 0, 0).unwrap(),
        Some(digest('d')),
    )
}

fn coverage_cell(epoch: &EpochRecord, key: &str) -> CoverageCellRecord {
    CoverageCellRecord::new(
        epoch.id().clone(),
        CoverageScope::new("crate", key).unwrap(),
        SemanticLens::new("correctness").unwrap(),
    )
}

fn artifact(path: &str, bytes: &[u8]) -> ArtifactEvidenceRef {
    ArtifactEvidenceRef::new(
        CsaSessionId::parse(SESSION_A).unwrap(),
        SessionRelativeArtifactPath::new(path).unwrap(),
        Sha256Digest::compute(bytes),
    )
}

fn model() -> AdmittedModelIdentity {
    AdmittedModelIdentity::new("codex", "openai", "gpt-5.6", "high").unwrap()
}

fn open(ledger: &mut ConvergenceLedger, campaign: &CampaignRecord, epoch: &EpochRecord) {
    ledger
        .append(
            campaign.id().clone(),
            ConvergenceEvent::CampaignStarted(campaign.clone()),
        )
        .unwrap();
    ledger
        .append(
            campaign.id().clone(),
            ConvergenceEvent::EpochOpened(epoch.clone()),
        )
        .unwrap();
}

fn define(
    ledger: &mut ConvergenceLedger,
    campaign: &CampaignRecord,
    cell: &CoverageCellRecord,
    requirement: CoverageRequirement,
) {
    ledger
        .append(
            campaign.id().clone(),
            ConvergenceEvent::CoverageCellDefined(cell.clone()),
        )
        .unwrap();
    ledger
        .append(
            campaign.id().clone(),
            ConvergenceEvent::CoverageDispositionRecorded(
                CoverageDispositionRecord::new(
                    cell.id().clone(),
                    requirement,
                    "review_policy",
                    "The frozen review policy determines this requirement.",
                )
                .unwrap(),
            ),
        )
        .unwrap();
}

fn finalize_plan(ledger: &mut ConvergenceLedger, campaign: &CampaignRecord, epoch: &EpochRecord) {
    ledger
        .append(
            campaign.id().clone(),
            ConvergenceEvent::CoveragePlanFinalized(CoveragePlanFinalizationRecord::new(
                epoch.id().clone(),
            )),
        )
        .unwrap();
}

fn planned(
    campaign: &CampaignRecord,
    epoch: &EpochRecord,
    cells: &[(&CoverageCellRecord, CoverageRequirement)],
) -> ConvergenceLedger {
    let mut ledger = ConvergenceLedger::empty();
    open(&mut ledger, campaign, epoch);
    for (cell, requirement) in cells {
        define(&mut ledger, campaign, cell, *requirement);
    }
    finalize_plan(&mut ledger, campaign, epoch);
    ledger
}

#[derive(Clone)]
struct AttemptSpec {
    id: &'static str,
    completion: ProviderTurnCompletion,
    limit: u32,
    count: u32,
    more: bool,
    unscanned: Vec<String>,
}

impl AttemptSpec {
    fn clean(id: &'static str) -> Self {
        Self {
            id,
            completion: ProviderTurnCompletion::Natural,
            limit: 2,
            count: 0,
            more: false,
            unscanned: Vec::new(),
        }
    }
}

fn attempt(
    epoch: &EpochRecord,
    cell: &CoverageCellRecord,
    spec: &AttemptSpec,
) -> DiscoveryAttemptRecord {
    DiscoveryAttemptRecord::new(
        DiscoveryAttemptId::parse(spec.id).unwrap(),
        epoch.id().clone(),
        cell.id().clone(),
        Utc.with_ymd_and_hms(2026, 7, 14, 12, 1, 0).unwrap(),
        spec.completion,
        model(),
        artifact(&format!("discovery/{}.json", spec.id), spec.id.as_bytes()),
        spec.limit,
        spec.count,
        spec.more,
        spec.unscanned.clone(),
    )
    .unwrap()
}

fn candidate(id: &str, attempt_id: &DiscoveryAttemptId) -> CandidateRecord {
    CandidateRecord::new(
        CandidateId::parse(id).unwrap(),
        attempt_id.clone(),
        SemanticFindingIdentity::new(
            "state transitions must be checked",
            "unchecked transition",
            "csa-session",
            "missing guard",
        )
        .unwrap(),
        artifact(&format!("candidates/{id}.json"), id.as_bytes()),
    )
}

fn append_attempt(
    ledger: &mut ConvergenceLedger,
    campaign: &CampaignRecord,
    epoch: &EpochRecord,
    cell: &CoverageCellRecord,
    spec: &AttemptSpec,
    candidate_ids: &[&str],
    finalize: bool,
) -> DiscoveryAttemptId {
    let record = attempt(epoch, cell, spec);
    let attempt_id = record.id().clone();
    ledger
        .append(
            campaign.id().clone(),
            ConvergenceEvent::DiscoveryAttemptRecorded(record),
        )
        .unwrap();
    for candidate_id in candidate_ids {
        ledger
            .append(
                campaign.id().clone(),
                ConvergenceEvent::CandidateRecorded(candidate(candidate_id, &attempt_id)),
            )
            .unwrap();
    }
    if finalize {
        ledger
            .append(
                campaign.id().clone(),
                ConvergenceEvent::DiscoveryAttemptFinalized(
                    DiscoveryAttemptFinalizationRecord::new(attempt_id.clone()),
                ),
            )
            .unwrap();
    }
    attempt_id
}

fn run_intent(directive: DiscoveryDirective) -> (CoverageCellRecord, u32, DiscoveryRunIntent) {
    match directive {
        DiscoveryDirective::RunDiscovery {
            cell,
            prior_finalized_attempt_count,
            intent,
        } => (cell, prior_finalized_attempt_count, intent),
        other => panic!("expected discovery: {other:?}"),
    }
}

#[test]
fn discovery_reducer_validates_ledger_campaign_epoch_and_snapshot_digests_first() {
    let campaign = campaign(CAMPAIGN_A);
    let epoch = EpochRecord::new(oid('a'), oid('b'), digest('c'));
    let expected = coverage_cell(&epoch, "csa-session");

    let mut invalid_json = serde_json::to_value(ConvergenceLedger::empty()).unwrap();
    invalid_json["schema_version"] = json!(999);
    let invalid: ConvergenceLedger = serde_json::from_value(invalid_json).unwrap();
    let error =
        next_discovery_directive(&invalid, &campaign, &epoch, std::slice::from_ref(&expected))
            .expect_err("invalid ledger");
    assert!(
        error
            .to_string()
            .contains("unsupported convergence ledger schema")
    );

    assert!(
        next_discovery_directive(
            &ConvergenceLedger::empty(),
            &campaign,
            &epoch,
            std::slice::from_ref(&expected),
        )
        .is_err()
    );

    let mut ledger = ConvergenceLedger::empty();
    open(&mut ledger, &campaign, &epoch);
    let mismatched_campaign = CampaignRecord::new(
        campaign.id().clone(),
        Utc.with_ymd_and_hms(2026, 7, 14, 12, 0, 1).unwrap(),
        campaign.policy_digest().cloned(),
        campaign.command_authority().clone(),
    );
    assert!(
        next_discovery_directive(
            &ledger,
            &mismatched_campaign,
            &epoch,
            std::slice::from_ref(&expected),
        )
        .is_err()
    );

    let other_epoch = EpochRecord::new(oid('f'), oid('1'), digest('2'));
    let other_cell = coverage_cell(&other_epoch, "csa-session");
    assert!(
        next_discovery_directive(
            &ledger,
            &campaign,
            &other_epoch,
            std::slice::from_ref(&other_cell),
        )
        .is_err()
    );

    let missing = CampaignRecord::new(
        campaign.id().clone(),
        *campaign.created_at(),
        None,
        campaign.command_authority().clone(),
    );
    let mut missing_ledger = ConvergenceLedger::empty();
    open(&mut missing_ledger, &missing, &epoch);
    assert!(
        next_discovery_directive(
            &missing_ledger,
            &missing,
            &epoch,
            std::slice::from_ref(&expected),
        )
        .is_err()
    );
}

#[test]
fn discovery_reducer_rejects_invalid_or_drifting_expected_manifests() {
    let campaign = campaign(CAMPAIGN_A);
    let epoch = EpochRecord::new(oid('a'), oid('b'), digest('c'));
    let first = coverage_cell(&epoch, "csa-session");
    let second = coverage_cell(&epoch, "csa-process");
    let mut ledger = ConvergenceLedger::empty();
    open(&mut ledger, &campaign, &epoch);

    assert!(next_discovery_directive(&ledger, &campaign, &epoch, &[]).is_err());
    assert!(
        next_discovery_directive(&ledger, &campaign, &epoch, &[first.clone(), first.clone()],)
            .is_err()
    );

    let wrong_epoch = EpochRecord::new(oid('3'), oid('4'), digest('5'));
    assert!(
        next_discovery_directive(
            &ledger,
            &campaign,
            &epoch,
            std::slice::from_ref(&coverage_cell(&wrong_epoch, "csa-session")),
        )
        .is_err()
    );

    define(
        &mut ledger,
        &campaign,
        &first,
        CoverageRequirement::Required,
    );
    define(
        &mut ledger,
        &campaign,
        &second,
        CoverageRequirement::Required,
    );
    assert!(
        next_discovery_directive(&ledger, &campaign, &epoch, std::slice::from_ref(&first),)
            .is_err()
    );

    let mut tampered = serde_json::to_value(&first).unwrap();
    tampered["scope"]["key"] = json!("different");
    let tampered: CoverageCellRecord = serde_json::from_value(tampered).unwrap();
    assert!(next_discovery_directive(&ledger, &campaign, &epoch, &[tampered, second]).is_err());
}

#[test]
fn discovery_reducer_defines_disposes_and_seals_manifest_in_caller_order() {
    let campaign = campaign(CAMPAIGN_A);
    let epoch = EpochRecord::new(oid('a'), oid('b'), digest('c'));
    let first = coverage_cell(&epoch, "csa-session");
    let second = coverage_cell(&epoch, "csa-process");
    let expected = [first.clone(), second.clone()];
    let mut ledger = ConvergenceLedger::empty();
    open(&mut ledger, &campaign, &epoch);

    assert_eq!(
        next_discovery_directive(&ledger, &campaign, &epoch, &expected).unwrap(),
        DiscoveryDirective::DefineCoverageCell {
            cell: first.clone()
        }
    );
    ledger
        .append(
            campaign.id().clone(),
            ConvergenceEvent::CoverageCellDefined(first.clone()),
        )
        .unwrap();
    assert_eq!(
        next_discovery_directive(&ledger, &campaign, &epoch, &expected).unwrap(),
        DiscoveryDirective::RecordCoverageDisposition {
            cell: first.clone()
        }
    );
    ledger
        .append(
            campaign.id().clone(),
            ConvergenceEvent::CoverageDispositionRecorded(
                CoverageDispositionRecord::new(
                    first.id().clone(),
                    CoverageRequirement::Required,
                    "review_policy",
                    "Required by the frozen policy.",
                )
                .unwrap(),
            ),
        )
        .unwrap();
    assert_eq!(
        next_discovery_directive(&ledger, &campaign, &epoch, &expected).unwrap(),
        DiscoveryDirective::DefineCoverageCell {
            cell: second.clone()
        }
    );
    define(
        &mut ledger,
        &campaign,
        &second,
        CoverageRequirement::NotApplicable,
    );
    assert_eq!(
        next_discovery_directive(&ledger, &campaign, &epoch, &expected).unwrap(),
        DiscoveryDirective::FinalizeCoveragePlan {
            record: CoveragePlanFinalizationRecord::new(epoch.id().clone())
        }
    );
}

#[test]
fn discovery_reducer_fails_closed_for_finalized_empty_or_incomplete_plans() {
    let campaign = campaign(CAMPAIGN_A);
    let epoch = EpochRecord::new(oid('a'), oid('b'), digest('c'));
    let first = coverage_cell(&epoch, "csa-session");
    let second = coverage_cell(&epoch, "csa-process");

    let mut empty = ConvergenceLedger::empty();
    open(&mut empty, &campaign, &epoch);
    finalize_plan(&mut empty, &campaign, &epoch);
    assert!(
        next_discovery_directive(&empty, &campaign, &epoch, std::slice::from_ref(&first),).is_err()
    );

    let incomplete = planned(
        &campaign,
        &epoch,
        &[(&first, CoverageRequirement::Required)],
    );
    assert!(next_discovery_directive(&incomplete, &campaign, &epoch, &[first, second],).is_err());
}

#[test]
fn discovery_reducer_reconciles_candidates_then_finalizes_before_running() {
    let campaign = campaign(CAMPAIGN_A);
    let epoch = EpochRecord::new(oid('a'), oid('b'), digest('c'));
    let cell = coverage_cell(&epoch, "csa-session");
    let mut ledger = planned(&campaign, &epoch, &[(&cell, CoverageRequirement::Required)]);
    let spec = AttemptSpec {
        id: ATTEMPT_A,
        completion: ProviderTurnCompletion::Natural,
        limit: 3,
        count: 2,
        more: false,
        unscanned: Vec::new(),
    };
    let attempt_id = append_attempt(
        &mut ledger,
        &campaign,
        &epoch,
        &cell,
        &spec,
        &[CANDIDATE_A],
        false,
    );
    assert_eq!(
        next_discovery_directive(&ledger, &campaign, &epoch, std::slice::from_ref(&cell),).unwrap(),
        DiscoveryDirective::RecordMissingCandidates {
            attempt_id: attempt_id.clone(),
            missing_candidate_count: 1,
        }
    );
    ledger
        .append(
            campaign.id().clone(),
            ConvergenceEvent::CandidateRecorded(candidate(CANDIDATE_B, &attempt_id)),
        )
        .unwrap();
    assert_eq!(
        next_discovery_directive(&ledger, &campaign, &epoch, std::slice::from_ref(&cell),).unwrap(),
        DiscoveryDirective::FinalizeDiscoveryAttempt {
            record: DiscoveryAttemptFinalizationRecord::new(attempt_id),
        }
    );
}

#[test]
fn discovery_reducer_ignores_candidate_dispositions_for_saturation() {
    let campaign = campaign(CAMPAIGN_A);
    let epoch = EpochRecord::new(oid('a'), oid('b'), digest('c'));
    let cell = coverage_cell(&epoch, "csa-session");
    let mut ledger = planned(&campaign, &epoch, &[(&cell, CoverageRequirement::Required)]);
    let spec = AttemptSpec {
        id: ATTEMPT_A,
        completion: ProviderTurnCompletion::Natural,
        limit: 2,
        count: 1,
        more: false,
        unscanned: Vec::new(),
    };
    append_attempt(
        &mut ledger,
        &campaign,
        &epoch,
        &cell,
        &spec,
        &[CANDIDATE_A],
        true,
    );
    let before =
        next_discovery_directive(&ledger, &campaign, &epoch, std::slice::from_ref(&cell)).unwrap();
    assert_eq!(
        run_intent(before.clone()).2,
        DiscoveryRunIntent::SaturationChallenge
    );

    ledger
        .append(
            campaign.id().clone(),
            ConvergenceEvent::CandidateDispositionRecorded(CandidateDispositionRecord::new(
                CandidateId::parse(CANDIDATE_A).unwrap(),
                CandidateDisposition::Verified,
                CandidateVerificationEvidence::new(
                    epoch.id().clone(),
                    model(),
                    VerificationIndependence::degraded("one").unwrap(),
                    artifact("dispositions/a.json", b"verified"),
                ),
            )),
        )
        .unwrap();
    assert_eq!(
        next_discovery_directive(&ledger, &campaign, &epoch, std::slice::from_ref(&cell),).unwrap(),
        before
    );
}

#[test]
fn discovery_reducer_classifies_each_continuation_signal_independently() {
    let campaign = campaign(CAMPAIGN_A);
    let epoch = EpochRecord::new(oid('a'), oid('b'), digest('c'));
    let cell = coverage_cell(&epoch, "csa-session");
    let cases = [
        AttemptSpec {
            id: ATTEMPT_A,
            completion: ProviderTurnCompletion::TerminalNonNatural,
            limit: 2,
            count: 0,
            more: false,
            unscanned: Vec::new(),
        },
        AttemptSpec {
            id: ATTEMPT_A,
            completion: ProviderTurnCompletion::Natural,
            limit: 1,
            count: 1,
            more: false,
            unscanned: Vec::new(),
        },
        AttemptSpec {
            id: ATTEMPT_A,
            completion: ProviderTurnCompletion::Natural,
            limit: 2,
            count: 0,
            more: true,
            unscanned: Vec::new(),
        },
        AttemptSpec {
            id: ATTEMPT_A,
            completion: ProviderTurnCompletion::Natural,
            limit: 2,
            count: 0,
            more: false,
            unscanned: vec!["src/unscanned.rs".to_string()],
        },
    ];

    for spec in cases {
        let mut ledger = planned(&campaign, &epoch, &[(&cell, CoverageRequirement::Required)]);
        let candidate_ids = if spec.count == 0 {
            Vec::new()
        } else {
            vec![CANDIDATE_A]
        };
        append_attempt(
            &mut ledger,
            &campaign,
            &epoch,
            &cell,
            &spec,
            &candidate_ids,
            true,
        );
        let (directed_cell, prior_count, intent) = run_intent(
            next_discovery_directive(&ledger, &campaign, &epoch, std::slice::from_ref(&cell))
                .unwrap(),
        );
        assert_eq!(directed_cell, cell);
        assert_eq!(prior_count, 1);
        assert_eq!(intent, DiscoveryRunIntent::Continuation);
    }
}

#[test]
fn discovery_reducer_uses_latest_finalized_page_and_requires_zero_new_challenge() {
    let campaign = campaign(CAMPAIGN_A);
    let epoch = EpochRecord::new(oid('a'), oid('b'), digest('c'));
    let cell = coverage_cell(&epoch, "csa-session");
    let mut ledger = planned(&campaign, &epoch, &[(&cell, CoverageRequirement::Required)]);

    let (_, prior_count, intent) = run_intent(
        next_discovery_directive(&ledger, &campaign, &epoch, std::slice::from_ref(&cell)).unwrap(),
    );
    assert_eq!(prior_count, 0);
    assert_eq!(intent, DiscoveryRunIntent::Initial);

    append_attempt(
        &mut ledger,
        &campaign,
        &epoch,
        &cell,
        &AttemptSpec::clean(ATTEMPT_A),
        &[],
        true,
    );
    assert_eq!(
        next_discovery_directive(&ledger, &campaign, &epoch, std::slice::from_ref(&cell),).unwrap(),
        DiscoveryDirective::DiscoveryEvidenceComplete {
            epoch_id: epoch.id().clone()
        }
    );

    let producing = AttemptSpec {
        id: ATTEMPT_B,
        completion: ProviderTurnCompletion::Natural,
        limit: 2,
        count: 1,
        more: false,
        unscanned: Vec::new(),
    };
    append_attempt(
        &mut ledger,
        &campaign,
        &epoch,
        &cell,
        &producing,
        &[CANDIDATE_A],
        true,
    );
    let (_, prior_count, intent) = run_intent(
        next_discovery_directive(&ledger, &campaign, &epoch, std::slice::from_ref(&cell)).unwrap(),
    );
    assert_eq!(prior_count, 2);
    assert_eq!(intent, DiscoveryRunIntent::SaturationChallenge);

    append_attempt(
        &mut ledger,
        &campaign,
        &epoch,
        &cell,
        &AttemptSpec::clean(ATTEMPT_C),
        &[],
        true,
    );
    assert_eq!(
        next_discovery_directive(&ledger, &campaign, &epoch, std::slice::from_ref(&cell),).unwrap(),
        DiscoveryDirective::DiscoveryEvidenceComplete {
            epoch_id: epoch.id().clone()
        }
    );
}

#[test]
fn discovery_reducer_skips_not_applicable_and_ignores_other_campaigns_and_epochs() {
    let campaign_a = campaign(CAMPAIGN_A);
    let campaign_b = campaign(CAMPAIGN_B);
    let epoch = EpochRecord::new(oid('a'), oid('b'), digest('c'));
    let cell = coverage_cell(&epoch, "csa-session");

    let not_applicable = planned(
        &campaign_a,
        &epoch,
        &[(&cell, CoverageRequirement::NotApplicable)],
    );
    assert_eq!(
        next_discovery_directive(
            &not_applicable,
            &campaign_a,
            &epoch,
            std::slice::from_ref(&cell),
        )
        .unwrap(),
        DiscoveryDirective::DiscoveryEvidenceComplete {
            epoch_id: epoch.id().clone()
        }
    );

    let mut ledger = planned(
        &campaign_a,
        &epoch,
        &[(&cell, CoverageRequirement::Required)],
    );
    open(&mut ledger, &campaign_b, &epoch);
    define(
        &mut ledger,
        &campaign_b,
        &cell,
        CoverageRequirement::Required,
    );
    finalize_plan(&mut ledger, &campaign_b, &epoch);
    append_attempt(
        &mut ledger,
        &campaign_b,
        &epoch,
        &cell,
        &AttemptSpec::clean(ATTEMPT_A),
        &[],
        true,
    );

    let other_epoch = EpochRecord::new(oid('6'), oid('7'), digest('8'));
    let other_cell = coverage_cell(&other_epoch, "csa-session");
    ledger
        .append(
            campaign_a.id().clone(),
            ConvergenceEvent::EpochOpened(other_epoch.clone()),
        )
        .unwrap();
    define(
        &mut ledger,
        &campaign_a,
        &other_cell,
        CoverageRequirement::Required,
    );
    finalize_plan(&mut ledger, &campaign_a, &other_epoch);
    append_attempt(
        &mut ledger,
        &campaign_a,
        &other_epoch,
        &other_cell,
        &AttemptSpec::clean(ATTEMPT_B),
        &[],
        true,
    );

    let (directed_cell, prior_count, intent) = run_intent(
        next_discovery_directive(&ledger, &campaign_a, &epoch, std::slice::from_ref(&cell))
            .unwrap(),
    );
    assert_eq!(directed_cell, cell);
    assert_eq!(prior_count, 0);
    assert_eq!(intent, DiscoveryRunIntent::Initial);
}

#[test]
fn discovery_completion_is_typed_evidence_not_a_review_verdict() {
    let campaign = campaign(CAMPAIGN_A);
    let epoch = EpochRecord::new(oid('a'), oid('b'), digest('c'));
    let cell = coverage_cell(&epoch, "csa-session");
    let ledger = planned(
        &campaign,
        &epoch,
        &[(&cell, CoverageRequirement::NotApplicable)],
    );

    let directive =
        next_discovery_directive(&ledger, &campaign, &epoch, std::slice::from_ref(&cell)).unwrap();
    assert_eq!(
        directive,
        DiscoveryDirective::DiscoveryEvidenceComplete {
            epoch_id: epoch.id().clone()
        }
    );
    let debug = format!("{directive:?}");
    assert!(!debug.contains("Pass"));
    assert!(!debug.contains("Clean"));

    let _: fn(
        &ConvergenceLedger,
        &CampaignRecord,
        &EpochRecord,
        &[CoverageCellRecord],
    ) -> anyhow::Result<crate::DiscoveryDirective> = crate::next_discovery_directive;
}
