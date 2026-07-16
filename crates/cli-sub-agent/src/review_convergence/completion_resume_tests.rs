use std::collections::VecDeque;

use chrono::{TimeZone, Utc};
use csa_process::ProviderTurnCompletion;
use csa_session::convergence::{
    AdmittedModelIdentity, ArtifactEvidenceRef, CampaignId, CampaignRecord, CandidateDisposition,
    CandidateDispositionRecord, CandidateId, CandidateRecord, CandidateVerificationEvidence,
    CommandAuthorityCatalogIdentity, CommandAuthorityPolicy, CommandAuthoritySnapshot,
    CommandAuthoritySource, ConvergenceEvent, ConvergenceLedger, CoverageCellRecord,
    CoverageDispositionRecord, CoveragePlanFinalizationRecord, CoverageRequirement, CoverageScope,
    CsaSessionId, DiscoveryAttemptFinalizationRecord, DiscoveryAttemptId, DiscoveryAttemptRecord,
    EpochRecord, GitObjectId, RepairBatchId, RepairBatchRecord, RootClusterId, RootClusterRecord,
    SemanticFindingIdentity, SemanticLens, SessionRelativeArtifactPath, Sha256Digest,
    VerificationIndependence,
};

use super::completion::{
    AuthorizedRepairBatch, CompletionAction as Action, CompletionBudget as Budget,
    CompletionError as Failure, CompletionEvent as Event,
    CompletionExecutionReservation as ExecutionReservation, CompletionOutcome,
    CompletionPhase as Phase, CompletionStart, ProviderTurnEvidence, ProviderTurnReconciliation,
    reconcile_provider_turns, run_to_attestation_from_start, start_completion,
};
use super::completion_provider_turn_tests::FakePorts;
use super::completion_tests::{clean_output, provider_reservation};
use super::completion_types::ClusteredCompletionClaim;

fn epoch(head: u8) -> EpochRecord {
    EpochRecord::new(
        GitObjectId::parse(&"11".repeat(20)).expect("base"),
        GitObjectId::parse(&format!("{head:02x}").repeat(20)).expect("head"),
        Sha256Digest::compute(&[head]),
    )
}

fn authority(model: &str) -> CommandAuthoritySnapshot {
    CommandAuthoritySnapshot::new(
        CommandAuthoritySource::direct("completion resume test").expect("source"),
        CommandAuthorityPolicy::new(false, Vec::new(), false, true).expect("policy"),
        CommandAuthorityCatalogIdentity::new("test catalog", "v1").expect("catalog"),
        vec![AdmittedModelIdentity::new("codex", "openai", model, "high").expect("model")],
    )
    .expect("authority")
}

fn artifact(label: &[u8]) -> ArtifactEvidenceRef {
    ArtifactEvidenceRef::new(
        CsaSessionId::generate(),
        SessionRelativeArtifactPath::new("output/review.json").expect("path"),
        Sha256Digest::compute(label),
    )
}

pub(super) fn clustered_claim(with_repairs: bool) -> (ConvergenceLedger, ClusteredCompletionClaim) {
    let campaign_id = CampaignId::generate();
    let epoch = epoch(2);
    let policy_digest = Sha256Digest::compute(b"clustered completion policy");
    let campaign = CampaignRecord::new(
        campaign_id.clone(),
        Utc.with_ymd_and_hms(2026, 7, 15, 12, 0, 0)
            .single()
            .expect("fixed fixture timestamp"),
        Some(policy_digest.clone()),
        authority("gpt-5.6"),
    );
    let model = campaign.command_authority().ordered_admitted()[0].clone();
    let cell = CoverageCellRecord::new(
        epoch.id().clone(),
        CoverageScope::new("crate", "completion").expect("coverage scope"),
        SemanticLens::new("correctness").expect("semantic lens"),
    );
    let attempt_id = DiscoveryAttemptId::generate();
    let discovery_session = CsaSessionId::generate();
    let discovery_artifact = ArtifactEvidenceRef::new(
        discovery_session.clone(),
        SessionRelativeArtifactPath::new("output/clustered-discovery.json")
            .expect("discovery artifact path"),
        Sha256Digest::compute(b"clustered discovery"),
    );
    let candidate_artifact = ArtifactEvidenceRef::new(
        discovery_session,
        SessionRelativeArtifactPath::new("output/clustered-candidate.json")
            .expect("candidate artifact path"),
        Sha256Digest::compute(b"clustered candidate"),
    );
    let candidate = CandidateRecord::new(
        CandidateId::generate(),
        attempt_id.clone(),
        SemanticFindingIdentity::new(
            "clustered resume preserves ledger identity",
            "resuming with an unrelated batch executes unapproved work",
            "completion reducer",
            "identity-splicing",
        )
        .expect("semantic identity"),
        candidate_artifact,
    );
    let attempt = DiscoveryAttemptRecord::new(
        attempt_id.clone(),
        epoch.id().clone(),
        cell.id().clone(),
        Utc.with_ymd_and_hms(2026, 7, 15, 12, 1, 0)
            .single()
            .expect("fixed fixture timestamp"),
        ProviderTurnCompletion::Natural,
        model.clone(),
        discovery_artifact,
        8,
        1,
        false,
        Vec::new(),
    )
    .expect("discovery attempt");
    let disposition = CandidateDispositionRecord::new(
        candidate.id().clone(),
        if with_repairs {
            CandidateDisposition::Verified
        } else {
            CandidateDisposition::RejectedWithEvidence
        },
        CandidateVerificationEvidence::new(
            epoch.id().clone(),
            model,
            VerificationIndependence::degraded("fixture has one admitted executor")
                .expect("independence"),
            artifact(b"clustered disposition"),
        ),
    );
    let mut events = vec![
        ConvergenceEvent::CampaignStarted(campaign),
        ConvergenceEvent::EpochOpened(epoch.clone()),
        ConvergenceEvent::CoverageCellDefined(cell.clone()),
        ConvergenceEvent::CoverageDispositionRecorded(
            CoverageDispositionRecord::new(
                cell.id().clone(),
                CoverageRequirement::Required,
                "review_policy",
                "The fixture requires correctness coverage.",
            )
            .expect("coverage disposition"),
        ),
        ConvergenceEvent::CoveragePlanFinalized(CoveragePlanFinalizationRecord::new(
            epoch.id().clone(),
        )),
        ConvergenceEvent::DiscoveryAttemptRecorded(attempt),
        ConvergenceEvent::CandidateRecorded(candidate.clone()),
        ConvergenceEvent::DiscoveryAttemptFinalized(DiscoveryAttemptFinalizationRecord::new(
            attempt_id,
        )),
        ConvergenceEvent::CandidateDispositionRecorded(disposition.clone()),
    ];
    let (root_cluster_ids, repair_batches) = if with_repairs {
        let disposition_set_digest =
            CandidateDispositionRecord::set_digest(std::slice::from_ref(&disposition));
        let cluster = RootClusterRecord::new(
            epoch.id().clone(),
            "resume only the ledger-authorized root cause",
            vec![candidate.id().clone()],
            disposition_set_digest.clone(),
        )
        .expect("root cluster");
        let batch = RepairBatchRecord::new(
            cluster.id().clone(),
            cluster.content_digest().clone(),
            epoch.id().clone(),
            vec![candidate.id().clone()],
            disposition_set_digest,
            vec!["repair only the durable root cause".to_string()],
            vec!["regress clustered resume".to_string()],
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )
        .expect("repair batch");
        let root_cluster_ids = vec![cluster.id().clone()];
        let repair_batches = vec![AuthorizedRepairBatch::new(
            cluster.id().clone(),
            batch.id().clone(),
        )];
        events.extend([
            ConvergenceEvent::RootClusterRecorded(cluster),
            ConvergenceEvent::RepairBatchRecorded(batch),
        ]);
        (root_cluster_ids, repair_batches)
    } else {
        (Vec::new(), Vec::new())
    };
    let mut ledger = ConvergenceLedger::empty();
    ledger
        .append_batch(campaign_id.clone(), events)
        .expect("valid clustered ledger");
    let ledger_generation = ledger.entries().last().map_or(
        0,
        csa_session::convergence::ConvergenceLedgerEntry::sequence,
    );
    (
        ledger,
        ClusteredCompletionClaim {
            campaign_id,
            epoch,
            candidate_ids: vec![candidate.id().clone()],
            root_cluster_ids,
            repair_batches,
            cycles: 7,
            provider_turns: 4,
            ledger_generation,
            policy_digest,
        },
    )
}

#[tokio::test]
async fn clustered_start_recovers_a_clean_campaign_without_rediscovery() {
    let (ledger, claim) = clustered_claim(false);
    let start = CompletionStart::clustered(&ledger, claim.clone()).expect("validated resume");
    let gate_artifact = artifact(b"clustered gates");
    let review = clean_output();
    let published_gate = gate_artifact.clone();
    let published_review = review.artifact().clone();
    let published_model = review.model_evidence().clone();
    let expected_gate = published_gate.clone();
    let expected_review = published_review.clone();
    let expected_model = published_model.clone();
    let mut ports = FakePorts::new(VecDeque::from([
        Ok(Event::FinalGatesPassed {
            campaign_id: claim.campaign_id.clone(),
            epoch_id: claim.epoch.id().clone(),
            artifact: gate_artifact,
        }),
        Ok(Event::CleanRoomCompleted {
            campaign_id: claim.campaign_id.clone(),
            epoch_id: claim.epoch.id().clone(),
            output: review,
        }),
        Ok(Event::FinalPairPublished {
            campaign_id: claim.campaign_id.clone(),
            epoch_id: claim.epoch.id().clone(),
            gate_artifact: published_gate,
            review_artifact: published_review,
            model_evidence: published_model,
        }),
    ]));

    let outcome = run_to_attestation_from_start(&mut ports, Budget::new(12, 8).unwrap(), start)
        .await
        .expect("clustered clean completion");

    assert!(matches!(
        outcome,
        CompletionOutcome::Attested { campaign_id, epoch, gate_artifact, review_artifact, model_evidence } if campaign_id == claim.campaign_id && epoch == claim.epoch && gate_artifact == expected_gate && review_artifact == expected_review && model_evidence == expected_model
    ));
    assert!(matches!(
        ports.actions(),
        [
            Action::RunFinalGates { .. },
            Action::RunFreshCleanRoom { .. },
            Action::PublishFinalPair { .. }
        ]
    ));
    assert!(ports.actions().iter().all(|action| !matches!(
        action,
        Action::Discover { .. } | Action::VerifyAndCluster { .. }
    )));
}

#[test]
fn clustered_start_dispatches_only_ledger_authorized_repairs_and_preserves_budget_usage() {
    let (ledger, claim) = clustered_claim(true);
    let start = CompletionStart::clustered(&ledger, claim.clone()).expect("validated resume");
    let transition = start_completion(Budget::new(12, 8).unwrap(), start).expect("first action");

    assert_eq!(transition.state.phase(), Phase::RunAuthorizedRepairs);
    assert_eq!(transition.state.cycles, claim.cycles);
    assert_eq!(transition.state.provider_turns, claim.provider_turns);
    assert_eq!(transition.state.clustered_candidates, claim.candidate_ids);
    assert_eq!(transition.state.root_clusters, claim.root_cluster_ids);
    assert_eq!(transition.state.repair_batches, claim.repair_batches);
    assert_eq!(
        transition.state.ledger_generation,
        Some(claim.ledger_generation)
    );
    assert_eq!(transition.state.policy_digest, Some(claim.policy_digest));
    assert!(matches!(
        transition.action,
        Some(Action::RunAuthorizedRepairs { batches, .. }) if batches == claim.repair_batches
    ));
}

#[test]
fn clustered_resume_provider_turn_usage_is_monotonic_after_reconciliation() {
    let (ledger, claim) = clustered_claim(true);
    let start = CompletionStart::clustered(&ledger, claim.clone()).expect("validated resume");
    let transition = start_completion(Budget::new(12, 8).unwrap(), start).expect("first action");
    let reservation = provider_reservation(1);
    let reconciled = reconcile_provider_turns(
        &transition.state,
        &ExecutionReservation::Provider(reservation.clone()),
        &ProviderTurnReconciliation::Reconciled {
            reservation,
            host_observed_turn_delta: 1,
            evidence: ProviderTurnEvidence::ConfirmedExecutionFallback,
        },
    )
    .expect("provider turn reconciliation");

    assert_eq!(reconciled.provider_turns, claim.provider_turns + 1);
}

#[test]
fn clustered_start_rejects_identity_set_policy_and_generation_mismatches() {
    let (ledger, claim) = clustered_claim(true);

    let mut cross_campaign = claim.clone();
    cross_campaign.campaign_id = CampaignId::generate();
    assert_eq!(
        CompletionStart::clustered(&ledger, cross_campaign),
        Err(Failure::IdentityMismatch)
    );

    let mut cross_epoch = claim.clone();
    cross_epoch.epoch = epoch(3);
    assert_eq!(
        CompletionStart::clustered(&ledger, cross_epoch),
        Err(Failure::IdentityMismatch)
    );

    let mut unknown_candidate = claim.clone();
    unknown_candidate.candidate_ids[0] = CandidateId::generate();
    assert_eq!(
        CompletionStart::clustered(&ledger, unknown_candidate),
        Err(Failure::IdentityMismatch)
    );

    let mut unknown_cluster = claim.clone();
    let unknown_root = RootClusterId::generate();
    unknown_cluster.root_cluster_ids[0] = unknown_root.clone();
    unknown_cluster.repair_batches[0] = AuthorizedRepairBatch::new(
        unknown_root,
        unknown_cluster.repair_batches[0].repair_batch_id().clone(),
    );
    assert_eq!(
        CompletionStart::clustered(&ledger, unknown_cluster),
        Err(Failure::IdentityMismatch)
    );

    let mut unknown_batch = claim.clone();
    unknown_batch.repair_batches[0] = AuthorizedRepairBatch::new(
        unknown_batch.repair_batches[0].root_cluster_id().clone(),
        RepairBatchId::generate(),
    );
    assert_eq!(
        CompletionStart::clustered(&ledger, unknown_batch),
        Err(Failure::IdentityMismatch)
    );

    let mut duplicate_candidate = claim.clone();
    duplicate_candidate
        .candidate_ids
        .push(duplicate_candidate.candidate_ids[0].clone());
    assert_eq!(
        CompletionStart::clustered(&ledger, duplicate_candidate),
        Err(Failure::DuplicateIdentity)
    );

    let mut duplicate_batch = claim.clone();
    let unclaimed_root = RootClusterId::generate();
    duplicate_batch
        .root_cluster_ids
        .push(unclaimed_root.clone());
    duplicate_batch
        .repair_batches
        .push(AuthorizedRepairBatch::new(
            unclaimed_root,
            duplicate_batch.repair_batches[0].repair_batch_id().clone(),
        ));
    assert_eq!(
        CompletionStart::clustered(&ledger, duplicate_batch),
        Err(Failure::DuplicateIdentity)
    );

    let mut duplicate_root = claim.clone();
    duplicate_root
        .root_cluster_ids
        .push(duplicate_root.root_cluster_ids[0].clone());
    duplicate_root
        .repair_batches
        .push(AuthorizedRepairBatch::new(
            duplicate_root.root_cluster_ids[0].clone(),
            RepairBatchId::generate(),
        ));
    assert_eq!(
        CompletionStart::clustered(&ledger, duplicate_root),
        Err(Failure::DuplicateIdentity)
    );

    let mut cardinality = claim.clone();
    cardinality.root_cluster_ids.clear();
    assert_eq!(
        CompletionStart::clustered(&ledger, cardinality),
        Err(Failure::CardinalityMismatch)
    );

    let mut oversized = claim.clone();
    oversized.candidate_ids = vec![claim.candidate_ids[0].clone(); 1_001];
    assert_eq!(
        CompletionStart::clustered(&ledger, oversized),
        Err(Failure::CardinalityMismatch)
    );

    let mut oversized_roots = claim.clone();
    oversized_roots.root_cluster_ids = vec![claim.root_cluster_ids[0].clone(); 1_001];
    assert_eq!(
        CompletionStart::clustered(&ledger, oversized_roots),
        Err(Failure::CardinalityMismatch)
    );

    let mut oversized_batches = claim.clone();
    oversized_batches.repair_batches = vec![claim.repair_batches[0].clone(); 1_001];
    assert_eq!(
        CompletionStart::clustered(&ledger, oversized_batches),
        Err(Failure::CardinalityMismatch)
    );

    let mut policy_mismatch = claim.clone();
    policy_mismatch.policy_digest = Sha256Digest::compute(b"different policy");
    assert_eq!(
        CompletionStart::clustered(&ledger, policy_mismatch),
        Err(Failure::PolicyDigestMismatch)
    );

    let mut stale_generation = claim;
    stale_generation.ledger_generation = stale_generation
        .ledger_generation
        .checked_sub(1)
        .expect("fixture ledger has entries");
    assert_eq!(
        CompletionStart::clustered(&ledger, stale_generation),
        Err(Failure::StaleLedgerGeneration)
    );
}
