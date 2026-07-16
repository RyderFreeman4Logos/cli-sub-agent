use std::{
    path::PathBuf,
    sync::{Arc, Barrier},
    thread,
};

use anyhow::anyhow;
use tempfile::tempdir;
use ulid::Ulid;

use crate::atomic_state_write::AtomicWriteFault;
use crate::convergence::{
    ArtifactEvidenceRef, CleanupConfirmation, CompletionActionId, ConvergenceEvent,
    ConvergenceLedger, ConvergenceLedgerStore, ProviderTurnExecutionId, WorkspaceLeaseIdentity,
    verify_merge_attestation,
};
use crate::convergence_attestation_tests::Fixture;

#[test]
fn pre_publish_failure_preserves_the_complete_unattested_prefix() {
    let fixture = Fixture::new();
    let temp = tempdir().unwrap();
    let store = ConvergenceLedgerStore::for_project_state_root(temp.path()).unwrap();
    store
        .append_batch(fixture.campaign_id.clone(), fixture.prefix_events.clone())
        .unwrap();
    let before = store.load().unwrap();
    let (review, attestation) = fixture.terminal_pair();

    assert!(
        store
            .publish_final_attestation_with_before_publish(
                fixture.campaign_id.clone(),
                review,
                attestation,
                |_| Err(anyhow!("injected before-publication failure")),
            )
            .is_err()
    );
    assert_eq!(store.load().unwrap(), before);
}

#[test]
fn verified_terminal_publication_reloads_artifacts_and_generation_before_commit() {
    let fixture = Fixture::new();
    let temp = tempdir().unwrap();
    let store = ConvergenceLedgerStore::for_project_state_root(temp.path()).unwrap();
    store
        .append_batch(fixture.campaign_id.clone(), fixture.prefix_events.clone())
        .unwrap();
    let reader = |reference: &ArtifactEvidenceRef| fixture.read_artifact(reference);

    let appended = store
        .publish_verified_final_attestation(
            fixture.campaign_id.clone(),
            fixture.gate.clone(),
            fixture.review.clone(),
            CleanupConfirmation::after_successful_cleanup(&fixture.cleanup_lease),
            &reader,
        )
        .unwrap();
    assert_eq!(appended.len(), 2);
    let ledger = store.load().unwrap();
    verify_merge_attestation(&ledger, &fixture.campaign_id, &reader).unwrap();
}

#[test]
fn terminal_publication_rejects_readback_digest_and_schema_mismatches() {
    let fixture = Fixture::new();
    let temp = tempdir().unwrap();
    let store = ConvergenceLedgerStore::for_project_state_root(temp.path()).unwrap();
    store
        .append_batch(fixture.campaign_id.clone(), fixture.prefix_events.clone())
        .unwrap();
    let before = store.load().unwrap();
    let tampered_reader = |reference: &ArtifactEvidenceRef| {
        if reference == fixture.gate.artifact() {
            Ok(b"tampered".to_vec())
        } else {
            fixture.read_artifact(reference)
        }
    };
    assert!(
        store
            .publish_verified_final_attestation(
                fixture.campaign_id.clone(),
                fixture.gate.clone(),
                fixture.review.clone(),
                CleanupConfirmation::after_successful_cleanup(&fixture.cleanup_lease),
                &tampered_reader,
            )
            .is_err()
    );
    assert_eq!(store.load().unwrap(), before);

    let invalid = Fixture::with_review_bytes(br#"{"schema":"wrong/v1"}"#.to_vec());
    let temp = tempdir().unwrap();
    let store = ConvergenceLedgerStore::for_project_state_root(temp.path()).unwrap();
    store
        .append_batch(invalid.campaign_id.clone(), invalid.prefix_events.clone())
        .unwrap();
    let before = store.load().unwrap();
    let reader = |reference: &ArtifactEvidenceRef| invalid.read_artifact(reference);
    assert!(
        store
            .publish_verified_final_attestation(
                invalid.campaign_id.clone(),
                invalid.gate.clone(),
                invalid.review.clone(),
                CleanupConfirmation::after_successful_cleanup(&invalid.cleanup_lease),
                &reader,
            )
            .is_err()
    );
    assert_eq!(store.load().unwrap(), before);
}

#[test]
fn terminal_publication_rejects_unconfirmed_cleanup_and_unresolved_actions() {
    let fixture = Fixture::new();
    let temp = tempdir().unwrap();
    let store = ConvergenceLedgerStore::for_project_state_root(temp.path()).unwrap();
    store
        .append_batch(fixture.campaign_id.clone(), fixture.prefix_events.clone())
        .unwrap();
    let before = store.load().unwrap();
    let unconfirmed_lease = WorkspaceLeaseIdentity::new(
        fixture.campaign_id.clone(),
        fixture.epoch.clone(),
        2,
        PathBuf::from("/unconfirmed-cleanup-lease"),
        3,
        4,
        Ulid::new().to_string(),
    )
    .unwrap();
    let reader = |reference: &ArtifactEvidenceRef| fixture.read_artifact(reference);
    assert!(
        store
            .publish_verified_final_attestation(
                fixture.campaign_id.clone(),
                fixture.gate.clone(),
                fixture.review.clone(),
                CleanupConfirmation::after_successful_cleanup(&unconfirmed_lease),
                &reader,
            )
            .is_err()
    );
    assert_eq!(store.load().unwrap(), before);

    let journal = store
        .initialize_completion_action_journal(
            fixture.campaign_id.clone(),
            fixture.epoch.id().clone(),
            fixture.policy_digest.clone(),
        )
        .unwrap();
    let claim = store
        .claim_completion_action(journal.generation(), CompletionActionId::generate())
        .unwrap();
    assert!(
        store
            .publish_verified_final_attestation(
                fixture.campaign_id.clone(),
                fixture.gate.clone(),
                fixture.review.clone(),
                CleanupConfirmation::after_successful_cleanup(&fixture.cleanup_lease),
                &reader,
            )
            .is_err()
    );
    let reservation = store
        .reserve_completion_provider_turn(&claim, ProviderTurnExecutionId::generate(), 1)
        .unwrap();
    store
        .mark_completion_provider_turn_usage_indeterminate(&reservation)
        .unwrap();
    assert!(
        store
            .publish_verified_final_attestation(
                fixture.campaign_id.clone(),
                fixture.gate.clone(),
                fixture.review.clone(),
                CleanupConfirmation::after_successful_cleanup(&fixture.cleanup_lease),
                &reader,
            )
            .is_err()
    );
    assert_eq!(store.load().unwrap(), before);
}

#[test]
fn terminal_publication_generation_cas_allows_one_concurrent_publisher() {
    let fixture = Fixture::new();
    let temp = tempdir().unwrap();
    let store = Arc::new(ConvergenceLedgerStore::for_project_state_root(temp.path()).unwrap());
    store
        .append_batch(fixture.campaign_id.clone(), fixture.prefix_events.clone())
        .unwrap();
    let expected_generation = store.load().unwrap().generation();
    let (review_one, attestation_one) = fixture.terminal_pair();
    let (review_two, attestation_two) = fixture.terminal_pair();
    let barrier = Arc::new(Barrier::new(2));

    let first_store = Arc::clone(&store);
    let first_campaign = fixture.campaign_id.clone();
    let first_barrier = Arc::clone(&barrier);
    let first = thread::spawn(move || {
        first_barrier.wait();
        first_store.publish_final_attestation_at_generation(
            first_campaign,
            expected_generation,
            review_one,
            attestation_one,
        )
    });
    let second_store = Arc::clone(&store);
    let second_campaign = fixture.campaign_id.clone();
    let second = thread::spawn(move || {
        barrier.wait();
        second_store.publish_final_attestation_at_generation(
            second_campaign,
            expected_generation,
            review_two,
            attestation_two,
        )
    });

    let results = [first.join().unwrap(), second.join().unwrap()];
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    let ledger = store.load().unwrap();
    assert_eq!(ledger.entries().len(), fixture.prefix_events.len() + 2);
    verify_merge_attestation(
        &ledger,
        &fixture.campaign_id,
        &|reference: &ArtifactEvidenceRef| fixture.read_artifact(reference),
    )
    .unwrap();
}

#[test]
fn terminal_pair_faults_never_expose_a_partial_attestation() {
    for fault in [
        AtomicWriteFault::BeforeRename,
        AtomicWriteFault::AfterRename,
        AtomicWriteFault::BeforeContainingDirectoryFsync,
        AtomicWriteFault::BeforeParentDirectoryFsync,
    ] {
        let fixture = Fixture::new();
        let temp = tempdir().unwrap();
        let store = ConvergenceLedgerStore::for_project_state_root(temp.path()).unwrap();
        store
            .append_batch(fixture.campaign_id.clone(), fixture.prefix_events.clone())
            .unwrap();
        let before = store.load().unwrap();
        let (review, attestation) = fixture.terminal_pair();
        assert!(
            store
                .append_batch_with_fault(
                    fixture.campaign_id.clone(),
                    vec![
                        ConvergenceEvent::FinalReviewRecorded(Box::new(review)),
                        ConvergenceEvent::MergeAttestationRecorded(Box::new(attestation)),
                    ],
                    fault,
                )
                .is_err()
        );

        let after = store.load().unwrap();
        if after != before {
            assert_eq!(after.entries().len(), before.entries().len() + 2);
            verify_merge_attestation(
                &after,
                &fixture.campaign_id,
                &|reference: &ArtifactEvidenceRef| fixture.read_artifact(reference),
            )
            .unwrap();
        }
    }
}

#[test]
fn reader_never_interprets_a_partial_terminal_record_as_attested() {
    let fixture = Fixture::new();
    let terminal = fixture.terminal_ledger();
    let mut serialized = serde_json::to_value(terminal).unwrap();
    serialized["entries"].as_array_mut().unwrap().pop();
    let partial: ConvergenceLedger = serde_json::from_value(serialized).unwrap();

    assert!(
        verify_merge_attestation(
            &partial,
            &fixture.campaign_id,
            &|reference: &ArtifactEvidenceRef| { fixture.read_artifact(reference) }
        )
        .is_err()
    );
}
