use chrono::{DateTime, TimeZone, Utc};
use csa_process::ProviderTurnCompletion;
use serde_json::json;

use crate::convergence::{
    AdmittedModelIdentity, ArtifactEvidenceRef, CONVERGENCE_LEDGER_SCHEMA_VERSION, CampaignId,
    CampaignRecord, CandidateDisposition, CandidateDispositionRecord, CandidateId, CandidateRecord,
    CandidateVerificationEvidence, ConvergenceEvent, ConvergenceLedger, ConvergenceLedgerEntry,
    CoverageCellRecord, CoverageDispositionRecord, CoveragePlanFinalizationRecord,
    CoverageRequirement, CoverageScope, CsaSessionId, DiscoveryAttemptFinalizationRecord,
    DiscoveryAttemptId, DiscoveryAttemptRecord, EpochRecord, GitObjectId, LedgerEventId,
    SemanticFindingIdentity, SemanticLens, SessionRelativeArtifactPath, Sha256Digest,
    VerificationIndependence,
};

const CAMPAIGN: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const SESSION: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
const ATTEMPT: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAX";
const CANDIDATE_1: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAY";
const CANDIDATE_2: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAZ";
const CANDIDATE_3: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB0";
const MISSING_CANDIDATE: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB1";

fn at(second: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 7, 14, 12, 0, second).unwrap()
}

fn campaign_id() -> CampaignId {
    CampaignId::parse(CAMPAIGN).unwrap()
}

fn digest(fill: char) -> Sha256Digest {
    Sha256Digest::parse(&format!("sha256:{}", fill.to_string().repeat(64))).unwrap()
}

fn oid(fill: char) -> GitObjectId {
    GitObjectId::parse(&fill.to_string().repeat(40)).unwrap()
}

fn epoch(fill: char) -> EpochRecord {
    EpochRecord::new(
        oid(fill),
        oid(char::from_u32(u32::from(fill) + 1).unwrap()),
        digest(fill),
    )
}

fn cell(epoch: &EpochRecord, key: &str) -> CoverageCellRecord {
    CoverageCellRecord::new(
        epoch.id().clone(),
        CoverageScope::new("crate", key).unwrap(),
        SemanticLens::new("correctness").unwrap(),
    )
}

fn model() -> AdmittedModelIdentity {
    AdmittedModelIdentity::new("codex", "openai", "gpt-5.6", "high").unwrap()
}

fn artifact(path: &str, bytes: &[u8]) -> ArtifactEvidenceRef {
    ArtifactEvidenceRef::new(
        CsaSessionId::parse(SESSION).unwrap(),
        SessionRelativeArtifactPath::new(path).unwrap(),
        Sha256Digest::compute(bytes),
    )
}

fn verification(epoch: &EpochRecord, path: &str, bytes: &[u8]) -> CandidateVerificationEvidence {
    CandidateVerificationEvidence::new(
        epoch.id().clone(),
        model(),
        VerificationIndependence::degraded("the frozen test authority admits one executor")
            .unwrap(),
        artifact(path, bytes),
    )
}

fn attempt_for(
    id: DiscoveryAttemptId,
    epoch: &EpochRecord,
    cell: &CoverageCellRecord,
    completion: ProviderTurnCompletion,
    count: u32,
) -> DiscoveryAttemptRecord {
    DiscoveryAttemptRecord::new(
        id,
        epoch.id().clone(),
        cell.id().clone(),
        at(4),
        completion,
        model(),
        artifact("discovery/attempt.json", b"attempt"),
        8,
        count,
        count == 8,
        vec!["src/generated.rs".to_string()],
    )
    .unwrap()
}

fn identity(bug_class: &str) -> SemanticFindingIdentity {
    SemanticFindingIdentity::new(
        "state transitions must be checked",
        "unchecked state transition",
        "csa-session",
        bug_class,
    )
    .unwrap()
}

fn candidate_for(
    id: &str,
    attempt_id: &DiscoveryAttemptId,
    identity: SemanticFindingIdentity,
) -> CandidateRecord {
    CandidateRecord::new(
        CandidateId::parse(id).unwrap(),
        attempt_id.clone(),
        identity,
        artifact(&format!("candidates/{id}.json"), id.as_bytes()),
    )
}

fn entry(
    sequence: u64,
    campaign_id: &CampaignId,
    event: ConvergenceEvent,
) -> ConvergenceLedgerEntry {
    ConvergenceLedgerEntry::new(
        sequence,
        LedgerEventId::generate(),
        campaign_id.clone(),
        at(u32::try_from(sequence).unwrap()),
        event,
    )
}

fn ledger(events: Vec<ConvergenceEvent>) -> ConvergenceLedger {
    let campaign_id = campaign_id();
    let entries = events
        .into_iter()
        .enumerate()
        .map(|(index, event)| entry(u64::try_from(index).unwrap() + 1, &campaign_id, event))
        .collect::<Vec<_>>();
    serde_json::from_value(json!({
        "schema_version": CONVERGENCE_LEDGER_SCHEMA_VERSION,
        "entries": entries,
    }))
    .unwrap()
}

fn campaign_start() -> ConvergenceEvent {
    let id = campaign_id();
    ConvergenceEvent::CampaignStarted(CampaignRecord::for_test(id, at(0), None))
}

fn discovery_history(
    epoch: &EpochRecord,
    cell: &CoverageCellRecord,
    attempt: &DiscoveryAttemptRecord,
) -> Vec<ConvergenceEvent> {
    vec![
        campaign_start(),
        ConvergenceEvent::EpochOpened(epoch.clone()),
        ConvergenceEvent::CoverageCellDefined(cell.clone()),
        ConvergenceEvent::CoverageDispositionRecorded(
            CoverageDispositionRecord::new(
                cell.id().clone(),
                CoverageRequirement::Required,
                "review_policy",
                "The campaign policy requires correctness coverage.",
            )
            .unwrap(),
        ),
        ConvergenceEvent::CoveragePlanFinalized(CoveragePlanFinalizationRecord::new(
            epoch.id().clone(),
        )),
        ConvergenceEvent::DiscoveryAttemptRecorded(attempt.clone()),
    ]
}

#[test]
fn convergence_evidence_natural_zero_candidate_attempt_round_trips() {
    let epoch = epoch('a');
    let cell = cell(&epoch, "csa-session");
    let attempt = DiscoveryAttemptRecord::new(
        DiscoveryAttemptId::parse(ATTEMPT).unwrap(),
        epoch.id().clone(),
        cell.id().clone(),
        at(4),
        ProviderTurnCompletion::Natural,
        model(),
        artifact("discovery/zero.json", b"[]"),
        8,
        0,
        false,
        Vec::new(),
    )
    .unwrap();

    let round_trip: DiscoveryAttemptRecord =
        serde_json::from_value(serde_json::to_value(&attempt).unwrap()).unwrap();
    assert_eq!(round_trip, attempt);
    assert_eq!(round_trip.reported_candidate_count(), 0);
    assert_eq!(round_trip.completion(), ProviderTurnCompletion::Natural);
    assert!(round_trip.unscanned_items().is_empty());
    assert!(!round_trip.more_candidates_possible());

    let incomplete = attempt_for(
        DiscoveryAttemptId::generate(),
        &epoch,
        &cell,
        ProviderTurnCompletion::Incomplete,
        0,
    );
    assert_eq!(incomplete.completion(), ProviderTurnCompletion::Incomplete);
    assert_ne!(incomplete.completion(), ProviderTurnCompletion::Natural);

    let generated = CsaSessionId::generate();
    assert_eq!(generated.as_str().len(), 26);
    assert!(CsaSessionId::parse("not-a-session").is_err());
    assert!(DiscoveryAttemptId::parse(CANDIDATE_1).is_ok());
    assert!(CandidateId::parse(ATTEMPT).is_ok());
}

#[test]
fn convergence_evidence_strict_attempt_inputs_and_json_are_rejected() {
    assert!(AdmittedModelIdentity::new(" ", "openai", "gpt", "high").is_err());
    assert!(AdmittedModelIdentity::new("codex", " ", "gpt", "high").is_err());
    assert!(AdmittedModelIdentity::new("codex", "openai", " ", "high").is_err());
    assert!(AdmittedModelIdentity::new("codex", "openai", "gpt", " ").is_err());

    for invalid in [
        "",
        " ",
        "/absolute",
        "a//b",
        "a/./b",
        "a/../b",
        "a/",
        " a/b",
    ] {
        assert!(
            SessionRelativeArtifactPath::new(invalid).is_err(),
            "path should be rejected: {invalid:?}"
        );
    }
    assert_eq!(
        SessionRelativeArtifactPath::new("output/evidence.json")
            .unwrap()
            .as_str(),
        "output/evidence.json"
    );

    let epoch = epoch('a');
    let cell = cell(&epoch, "csa-session");
    let build = |limit, count, items| {
        DiscoveryAttemptRecord::new(
            DiscoveryAttemptId::generate(),
            epoch.id().clone(),
            cell.id().clone(),
            at(4),
            ProviderTurnCompletion::Unknown,
            model(),
            artifact("discovery/attempt.json", b"attempt"),
            limit,
            count,
            false,
            items,
        )
    };
    assert!(build(0, 0, Vec::new()).is_err());
    assert!(build(2, 3, Vec::new()).is_err());
    assert!(build(2, 0, vec![" ".to_string()]).is_err());
    assert!(
        build(
            2,
            0,
            vec!["src/lib.rs".to_string(), " src/lib.rs ".to_string()]
        )
        .is_err()
    );

    let valid = attempt_for(
        DiscoveryAttemptId::parse(ATTEMPT).unwrap(),
        &epoch,
        &cell,
        ProviderTurnCompletion::Unknown,
        1,
    );
    let mut unknown = serde_json::to_value(&valid).unwrap();
    unknown["future"] = json!(true);
    assert!(serde_json::from_value::<DiscoveryAttemptRecord>(unknown).is_err());

    let mut missing_completion = serde_json::to_value(&valid).unwrap();
    missing_completion
        .as_object_mut()
        .unwrap()
        .remove("completion");
    assert!(serde_json::from_value::<DiscoveryAttemptRecord>(missing_completion).is_err());

    let mut invalid_session = serde_json::to_value(valid).unwrap();
    invalid_session["artifact"]["csa_session_id"] = json!("invalid");
    assert!(serde_json::from_value::<DiscoveryAttemptRecord>(invalid_session).is_err());
}

#[test]
fn convergence_evidence_attempt_requires_prior_matching_epoch_and_cell() {
    let epoch_a = epoch('a');
    let epoch_b = epoch('d');
    let cell_a = cell(&epoch_a, "a");
    let attempt_a = attempt_for(
        DiscoveryAttemptId::parse(ATTEMPT).unwrap(),
        &epoch_a,
        &cell_a,
        ProviderTurnCompletion::Natural,
        0,
    );

    let before_cell = ledger(vec![
        campaign_start(),
        ConvergenceEvent::EpochOpened(epoch_a.clone()),
        ConvergenceEvent::DiscoveryAttemptRecorded(attempt_a.clone()),
        ConvergenceEvent::CoverageCellDefined(cell_a.clone()),
    ]);
    assert!(before_cell.validate().is_err());

    let mismatch = DiscoveryAttemptRecord::new(
        DiscoveryAttemptId::generate(),
        epoch_b.id().clone(),
        cell_a.id().clone(),
        at(5),
        ProviderTurnCompletion::Natural,
        model(),
        artifact("discovery/mismatch.json", b"mismatch"),
        1,
        0,
        false,
        Vec::new(),
    )
    .unwrap();
    let mismatch = ledger(vec![
        campaign_start(),
        ConvergenceEvent::EpochOpened(epoch_a),
        ConvergenceEvent::EpochOpened(epoch_b),
        ConvergenceEvent::CoverageCellDefined(cell_a),
        ConvergenceEvent::DiscoveryAttemptRecorded(mismatch),
    ]);
    assert!(mismatch.validate().is_err());

    let mut duplicate_events = discovery_history(&epoch('a'), &cell(&epoch('a'), "a"), &attempt_a);
    duplicate_events.push(ConvergenceEvent::DiscoveryAttemptRecorded(attempt_a));
    let duplicate = ledger(duplicate_events);
    assert!(duplicate.validate().is_err());
}

#[test]
fn convergence_evidence_candidates_require_attempt_and_detect_tampering() {
    let epoch = epoch('a');
    let cell = cell(&epoch, "csa-session");
    let attempt_id = DiscoveryAttemptId::parse(ATTEMPT).unwrap();
    let attempt = attempt_for(
        attempt_id.clone(),
        &epoch,
        &cell,
        ProviderTurnCompletion::Natural,
        2,
    );
    let candidate_1 = candidate_for(CANDIDATE_1, &attempt_id, identity("missing guard"));
    let candidate_2 = candidate_for(CANDIDATE_2, &attempt_id, identity("missing guard"));
    assert_eq!(
        candidate_1.stable_finding_id(),
        candidate_2.stable_finding_id()
    );
    assert_ne!(candidate_1.id(), candidate_2.id());

    let before_attempt = ledger(vec![
        campaign_start(),
        ConvergenceEvent::EpochOpened(epoch.clone()),
        ConvergenceEvent::CoverageCellDefined(cell.clone()),
        ConvergenceEvent::CandidateRecorded(candidate_1.clone()),
    ]);
    assert!(before_attempt.validate().is_err());

    let mut valid_events = discovery_history(&epoch, &cell, &attempt);
    valid_events.push(ConvergenceEvent::CandidateRecorded(candidate_1.clone()));
    valid_events.push(ConvergenceEvent::CandidateRecorded(candidate_2));
    ledger(valid_events)
        .validate()
        .expect("multiple observations may share a stable finding id");

    let mut tampered = serde_json::to_value(&candidate_1).unwrap();
    tampered["stable_finding_id"] = json!(digest('f').as_str());
    assert!(serde_json::from_value::<CandidateRecord>(tampered).is_err());

    let duplicate_id = candidate_for(CANDIDATE_1, &attempt_id, identity("different bug"));
    let mut duplicate_events = discovery_history(&epoch, &cell, &attempt);
    duplicate_events.push(ConvergenceEvent::CandidateRecorded(candidate_1));
    duplicate_events.push(ConvergenceEvent::CandidateRecorded(duplicate_id));
    assert!(ledger(duplicate_events).validate().is_err());
}

#[test]
fn convergence_evidence_duplicate_disposition_relations_are_validated() {
    let epoch = epoch('a');
    let cell = cell(&epoch, "csa-session");
    let attempt_id = DiscoveryAttemptId::parse(ATTEMPT).unwrap();
    let attempt = attempt_for(
        attempt_id.clone(),
        &epoch,
        &cell,
        ProviderTurnCompletion::Natural,
        3,
    );
    let canonical = candidate_for(CANDIDATE_1, &attempt_id, identity("missing guard"));
    let duplicate = candidate_for(CANDIDATE_2, &attempt_id, identity("missing guard"));
    let different = candidate_for(CANDIDATE_3, &attempt_id, identity("wrong branch"));
    let mut base = discovery_history(&epoch, &cell, &attempt);
    base.extend([
        ConvergenceEvent::CandidateRecorded(canonical.clone()),
        ConvergenceEvent::CandidateRecorded(duplicate.clone()),
        ConvergenceEvent::CandidateRecorded(different.clone()),
        ConvergenceEvent::DiscoveryAttemptFinalized(DiscoveryAttemptFinalizationRecord::new(
            attempt_id.clone(),
        )),
    ]);

    let accepted = CandidateDispositionRecord::new(
        duplicate.id().clone(),
        CandidateDisposition::Duplicate {
            canonical_candidate_id: canonical.id().clone(),
        },
        verification(&epoch, "dispositions/duplicate.json", b"duplicate"),
    );
    let mut accepted_events = base.clone();
    accepted_events.push(ConvergenceEvent::CandidateDispositionRecorded(
        accepted.clone(),
    ));
    ledger(accepted_events)
        .validate()
        .expect("same-stable-id duplicate relation must pass");

    let mismatched = CandidateDispositionRecord::new(
        different.id().clone(),
        CandidateDisposition::Duplicate {
            canonical_candidate_id: canonical.id().clone(),
        },
        verification(&epoch, "dispositions/mismatch.json", b"mismatch"),
    );
    let mut mismatched_events = base.clone();
    mismatched_events.push(ConvergenceEvent::CandidateDispositionRecorded(mismatched));
    assert!(ledger(mismatched_events).validate().is_err());

    for disposition in [
        CandidateDisposition::Duplicate {
            canonical_candidate_id: canonical.id().clone(),
        },
        CandidateDisposition::Superseded {
            replacement_candidate_id: canonical.id().clone(),
        },
    ] {
        let record = CandidateDispositionRecord::new(
            canonical.id().clone(),
            disposition,
            verification(&epoch, "dispositions/self.json", b"self"),
        );
        let mut events = base.clone();
        events.push(ConvergenceEvent::CandidateDispositionRecorded(record));
        assert!(ledger(events).validate().is_err());
    }

    let missing = CandidateId::parse(MISSING_CANDIDATE).unwrap();
    for disposition in [
        CandidateDisposition::Duplicate {
            canonical_candidate_id: missing.clone(),
        },
        CandidateDisposition::Superseded {
            replacement_candidate_id: missing.clone(),
        },
    ] {
        let record = CandidateDispositionRecord::new(
            canonical.id().clone(),
            disposition,
            verification(&epoch, "dispositions/missing.json", b"missing"),
        );
        let mut events = base.clone();
        events.push(ConvergenceEvent::CandidateDispositionRecorded(record));
        assert!(ledger(events).validate().is_err());
    }

    let mut twice = base;
    twice.push(ConvergenceEvent::CandidateDispositionRecorded(accepted));
    twice.push(ConvergenceEvent::CandidateDispositionRecorded(
        CandidateDispositionRecord::new(
            duplicate.id().clone(),
            CandidateDisposition::Verified,
            verification(&epoch, "dispositions/verified.json", b"verified"),
        ),
    ));
    assert!(ledger(twice).validate().is_err());
}

#[test]
fn convergence_evidence_all_terminal_disposition_variants_round_trip() {
    let epoch = epoch('a');
    let candidate = CandidateId::parse(CANDIDATE_1).unwrap();
    let target = CandidateId::parse(CANDIDATE_2).unwrap();
    let dispositions = [
        CandidateDisposition::Verified,
        CandidateDisposition::Duplicate {
            canonical_candidate_id: target.clone(),
        },
        CandidateDisposition::RejectedWithEvidence,
        CandidateDisposition::NeedsContractOrDocumentation,
        CandidateDisposition::PreExistingOutsideDiffScope,
        CandidateDisposition::Superseded {
            replacement_candidate_id: target,
        },
    ];

    for disposition in dispositions {
        let record = CandidateDispositionRecord::new(
            candidate.clone(),
            disposition,
            verification(&epoch, "dispositions/evidence.json", b"evidence"),
        );
        let round_trip: CandidateDispositionRecord =
            serde_json::from_value(serde_json::to_value(&record).unwrap()).unwrap();
        assert_eq!(round_trip, record);
    }
}

#[test]
fn convergence_evidence_coverage_dispositions_are_strict_and_unique() {
    let epoch = epoch('a');
    let undefined_cell = cell(&epoch, "undefined");
    let cell = cell(&epoch, "csa-session");
    let required = CoverageDispositionRecord::new(
        cell.id().clone(),
        CoverageRequirement::Required,
        "review_policy",
        "The campaign policy requires correctness coverage.",
    )
    .unwrap();
    let not_applicable = CoverageDispositionRecord::new(
        cell.id().clone(),
        CoverageRequirement::NotApplicable,
        "generated_code_only",
        "The cell contains generated code outside review ownership.",
    )
    .unwrap();

    for record in [&required, &not_applicable] {
        let round_trip: CoverageDispositionRecord =
            serde_json::from_value(serde_json::to_value(record).unwrap()).unwrap();
        assert_eq!(&round_trip, record);
        ledger(vec![
            campaign_start(),
            ConvergenceEvent::EpochOpened(epoch.clone()),
            ConvergenceEvent::CoverageCellDefined(cell.clone()),
            ConvergenceEvent::CoverageDispositionRecorded(record.clone()),
        ])
        .validate()
        .expect("one disposition for a defined cell must pass");
    }

    assert!(
        CoverageDispositionRecord::new(
            cell.id().clone(),
            CoverageRequirement::Required,
            " ",
            "rationale"
        )
        .is_err()
    );
    assert!(
        CoverageDispositionRecord::new(
            cell.id().clone(),
            CoverageRequirement::Required,
            "Not Normalized",
            "rationale"
        )
        .is_err()
    );
    assert!(
        CoverageDispositionRecord::new(
            cell.id().clone(),
            CoverageRequirement::Required,
            "review_policy",
            " "
        )
        .is_err()
    );

    let duplicate = ledger(vec![
        campaign_start(),
        ConvergenceEvent::EpochOpened(epoch.clone()),
        ConvergenceEvent::CoverageCellDefined(cell.clone()),
        ConvergenceEvent::CoverageDispositionRecorded(required),
        ConvergenceEvent::CoverageDispositionRecorded(not_applicable),
    ]);
    assert!(duplicate.validate().is_err());

    let undefined = CoverageDispositionRecord::new(
        undefined_cell.id().clone(),
        CoverageRequirement::Required,
        "review_policy",
        "Required, but not defined in this history.",
    )
    .unwrap();
    let undefined = ledger(vec![
        campaign_start(),
        ConvergenceEvent::EpochOpened(epoch),
        ConvergenceEvent::CoverageDispositionRecorded(undefined),
    ]);
    assert!(undefined.validate().is_err());
}
