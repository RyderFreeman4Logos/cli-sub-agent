use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Barrier};

use tempfile::tempdir;

use crate::atomic_state_write::AtomicWriteFault;
use crate::convergence::{
    COMPLETION_ACTION_JOURNAL_SCHEMA_VERSION, CampaignId, CompletionActionId,
    CompletionActionJournal, CompletionActionJournalError, CompletionActionJournalRead,
    CompletionActionState, ConvergenceLedgerStore, EpochRecord, GitObjectId,
    MAX_COMPLETION_ACTION_RECORDS, ProviderTurnExecutionId, ProviderTurnExecutionState,
    RepairBatchId, RepairIntent, RepairIntentRead, RepairIntentState, Sha256Digest,
    parse_legacy_completion_action_journal,
};

fn epoch() -> EpochRecord {
    EpochRecord::new(
        GitObjectId::parse("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap(),
        GitObjectId::parse("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb").unwrap(),
        Sha256Digest::compute(b"completion action journal epoch"),
    )
}

fn epoch_id() -> crate::convergence::EpochId {
    epoch().id().clone()
}

fn journal() -> CompletionActionJournal {
    CompletionActionJournal::new(
        CampaignId::generate(),
        epoch_id(),
        Sha256Digest::compute(b"completion action policy"),
    )
}

fn store_at(root: &std::path::Path) -> ConvergenceLedgerStore {
    ConvergenceLedgerStore::for_project_state_root(root).unwrap()
}

fn action_journal_path(root: &std::path::Path) -> std::path::PathBuf {
    root.join("convergence/completion-actions.json")
}

fn initialize(store: &ConvergenceLedgerStore) -> CompletionActionJournal {
    let journal = journal();
    store
        .initialize_completion_action_journal(
            journal.campaign_id().clone(),
            journal.epoch_id().clone(),
            journal.policy_digest().clone(),
        )
        .unwrap()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecoveryState {
    Continue,
    Reconcile,
}

fn recovery_state(fault: AtomicWriteFault) -> RecoveryState {
    match fault {
        AtomicWriteFault::BeforeRename => RecoveryState::Continue,
        AtomicWriteFault::AfterRename
        | AtomicWriteFault::BeforeContainingDirectoryFsync
        | AtomicWriteFault::BeforeParentDirectoryFsync => RecoveryState::Reconcile,
    }
}

fn atomic_faults() -> [AtomicWriteFault; 4] {
    [
        AtomicWriteFault::BeforeRename,
        AtomicWriteFault::AfterRename,
        AtomicWriteFault::BeforeContainingDirectoryFsync,
        AtomicWriteFault::BeforeParentDirectoryFsync,
    ]
}

#[test]
fn action_journal_writes_v2_records_with_identity_generation_and_policy() {
    let mut journal = journal();
    let claim = journal
        .claim_next(0, CompletionActionId::generate())
        .unwrap();
    let record = &journal.actions()[0];

    assert_eq!(
        journal.schema_version(),
        COMPLETION_ACTION_JOURNAL_SCHEMA_VERSION
    );
    assert_eq!(
        record.schema_version(),
        COMPLETION_ACTION_JOURNAL_SCHEMA_VERSION
    );
    assert_eq!(record.claim(), &claim);
    assert_eq!(record.claim().campaign_id(), journal.campaign_id());
    assert_eq!(record.claim().epoch_id(), journal.epoch_id());
    assert_eq!(record.claim().policy_digest(), journal.policy_digest());
    assert_eq!(record.claim().generation(), 1);
    assert_eq!(record.state(), CompletionActionState::Started);
}

#[test]
fn action_journal_claim_cas_allows_exactly_one_concurrent_holder() {
    let temp = tempdir().unwrap();
    let store = store_at(temp.path());
    initialize(&store);
    let barrier = Arc::new(Barrier::new(3));
    let first_store = store.clone();
    let first_barrier = Arc::clone(&barrier);
    let second_store = store.clone();
    let second_barrier = Arc::clone(&barrier);

    let first = std::thread::spawn(move || {
        first_barrier.wait();
        first_store.claim_completion_action(0, CompletionActionId::generate())
    });
    let second = std::thread::spawn(move || {
        second_barrier.wait();
        second_store.claim_completion_action(0, CompletionActionId::generate())
    });
    barrier.wait();

    let outcomes = [first.join().unwrap(), second.join().unwrap()];
    assert_eq!(outcomes.iter().filter(|outcome| outcome.is_ok()).count(), 1);
    let rejected = outcomes
        .iter()
        .find_map(|outcome| outcome.as_ref().err())
        .unwrap();
    assert!(rejected.to_string().contains("fencing mismatch"));
    let CompletionActionJournalRead::Current(journal) =
        store.load_completion_action_journal().unwrap()
    else {
        panic!("v2 action journal should persist");
    };
    assert_eq!(journal.generation(), 1);
    assert_eq!(journal.actions().len(), 1);
}

#[test]
fn recovery_fences_a_stale_holder_and_uncertainty_blocks_attestation() {
    let temp = tempdir().unwrap();
    let store = store_at(temp.path());
    initialize(&store);
    let original = store
        .claim_completion_action(0, CompletionActionId::generate())
        .unwrap();
    let replacement = store
        .recover_completion_action(&original, CompletionActionId::generate())
        .unwrap();

    let stale_error = store.finish_completion_action(&original).unwrap_err();
    assert!(stale_error.to_string().contains("fencing mismatch"));
    store.finish_completion_action(&replacement).unwrap();
    let attestation_error = store
        .verify_completion_action_journal_attestable()
        .unwrap_err();
    assert!(
        attestation_error
            .to_string()
            .contains("started or uncertain actions")
    );

    let CompletionActionJournalRead::Current(journal) =
        store.load_completion_action_journal().unwrap()
    else {
        panic!("v2 action journal should persist");
    };
    assert_eq!(
        journal.actions()[0].state(),
        CompletionActionState::Uncertain
    );
    assert_eq!(
        journal.actions()[1].state(),
        CompletionActionState::Finished
    );
    assert!(!journal.permits_attestation());
}

#[test]
fn started_action_also_blocks_attestation_until_current_holder_finishes() {
    let temp = tempdir().unwrap();
    let store = store_at(temp.path());
    initialize(&store);
    let claim = store
        .claim_completion_action(0, CompletionActionId::generate())
        .unwrap();
    assert!(store.verify_completion_action_journal_attestable().is_err());
    store.finish_completion_action(&claim).unwrap();
    store.verify_completion_action_journal_attestable().unwrap();
}

#[test]
fn provider_turn_reservation_is_durable_and_reconciles_exactly_once() {
    let temp = tempdir().unwrap();
    let store = store_at(temp.path());
    initialize(&store);
    let claim = store
        .claim_completion_action(0, CompletionActionId::generate())
        .unwrap();
    let reservation = store
        .reserve_completion_provider_turn(&claim, ProviderTurnExecutionId::generate(), 2)
        .unwrap();

    let CompletionActionJournalRead::Current(journal) =
        store.load_completion_action_journal().unwrap()
    else {
        panic!("provider reservation must be durable");
    };
    assert_eq!(journal.actions()[0].provider_turns().len(), 1);
    assert_eq!(
        journal.actions()[0].provider_turns()[0].state(),
        ProviderTurnExecutionState::Reserved
    );
    assert!(store.finish_completion_action(&claim).is_err());

    store
        .reconcile_completion_provider_turn(&reservation, 2)
        .unwrap();
    store
        .reconcile_completion_provider_turn(&reservation, 2)
        .unwrap();
    assert!(
        store
            .reconcile_completion_provider_turn(&reservation, 1)
            .is_err()
    );
    store.finish_completion_action(&claim).unwrap();
    store.verify_completion_action_journal_attestable().unwrap();

    let CompletionActionJournalRead::Current(journal) =
        store.load_completion_action_journal().unwrap()
    else {
        panic!("provider reconciliation must remain durable");
    };
    assert_eq!(
        journal.actions()[0].provider_turns()[0].state(),
        ProviderTurnExecutionState::Reconciled {
            observed_turn_delta: 2
        }
    );
}

#[test]
fn provider_turn_crash_before_send_releases_but_crash_after_send_is_indeterminate() {
    let temp = tempdir().unwrap();
    let store = store_at(temp.path());
    initialize(&store);
    let first = store
        .claim_completion_action(0, CompletionActionId::generate())
        .unwrap();
    let before_send = store
        .reserve_completion_provider_turn(&first, ProviderTurnExecutionId::generate(), 1)
        .unwrap();
    store
        .release_completion_provider_turn_before_send(&before_send)
        .unwrap();
    store.finish_completion_action(&first).unwrap();

    let second = store
        .claim_completion_action(1, CompletionActionId::generate())
        .unwrap();
    let after_send = store
        .reserve_completion_provider_turn(&second, ProviderTurnExecutionId::generate(), 1)
        .unwrap();
    store
        .mark_completion_provider_turn_usage_indeterminate(&after_send)
        .unwrap();
    assert!(store.finish_completion_action(&second).is_err());
    assert!(store.verify_completion_action_journal_attestable().is_err());

    let CompletionActionJournalRead::Current(journal) =
        store.load_completion_action_journal().unwrap()
    else {
        panic!("indeterminate provider usage must remain durable");
    };
    assert_eq!(
        journal.actions()[0].provider_turns()[0].state(),
        ProviderTurnExecutionState::ReleasedBeforeSend
    );
    assert_eq!(
        journal.actions()[1].provider_turns()[0].state(),
        ProviderTurnExecutionState::UsageIndeterminate
    );
}

#[test]
fn fault_matrix_action_claim_allows_only_continue_or_non_attested_reconciliation() {
    for fault in atomic_faults() {
        let temp = tempdir().unwrap();
        let store = store_at(temp.path());
        initialize(&store);

        assert!(
            store
                .claim_completion_action_with_fault(0, CompletionActionId::generate(), fault)
                .is_err(),
            "fault injection must interrupt the action claim at {fault:?}"
        );
        let CompletionActionJournalRead::Current(journal) =
            store.load_completion_action_journal().unwrap()
        else {
            panic!("initialized action journal must remain readable after {fault:?}");
        };

        match recovery_state(fault) {
            RecoveryState::Continue => {
                assert!(journal.actions().is_empty());
                let claim = store
                    .claim_completion_action(0, CompletionActionId::generate())
                    .expect("pre-rename claim failure permits a new fenced claim");
                store.finish_completion_action(&claim).unwrap();
                store.verify_completion_action_journal_attestable().unwrap();
            }
            RecoveryState::Reconcile => {
                let [record] = journal.actions() else {
                    panic!("post-rename action claim must leave exactly one started record");
                };
                let claim = record.claim().clone();
                assert_eq!(journal.actions()[0].state(), CompletionActionState::Started);
                assert!(store.verify_completion_action_journal_attestable().is_err());
                store.mark_completion_action_uncertain(&claim).unwrap();
                assert!(store.verify_completion_action_journal_attestable().is_err());
            }
        }
    }
}

#[test]
fn fault_matrix_provider_reservation_never_discards_possible_usage() {
    for fault in atomic_faults() {
        let temp = tempdir().unwrap();
        let store = store_at(temp.path());
        initialize(&store);
        let claim = store
            .claim_completion_action(0, CompletionActionId::generate())
            .unwrap();

        assert!(
            store
                .reserve_completion_provider_turn_with_fault(
                    &claim,
                    ProviderTurnExecutionId::generate(),
                    1,
                    fault,
                )
                .is_err(),
            "fault injection must interrupt the provider reservation at {fault:?}"
        );
        let CompletionActionJournalRead::Current(journal) =
            store.load_completion_action_journal().unwrap()
        else {
            panic!("action journal must remain readable after {fault:?}");
        };

        match recovery_state(fault) {
            RecoveryState::Continue => {
                assert!(journal.actions()[0].provider_turns().is_empty());
                let reservation = store
                    .reserve_completion_provider_turn(
                        &claim,
                        ProviderTurnExecutionId::generate(),
                        1,
                    )
                    .expect("pre-rename reservation failure permits a fresh reservation");
                store
                    .release_completion_provider_turn_before_send(&reservation)
                    .unwrap();
                store.finish_completion_action(&claim).unwrap();
                store.verify_completion_action_journal_attestable().unwrap();
            }
            RecoveryState::Reconcile => {
                assert_eq!(
                    journal.actions()[0].provider_turns()[0].state(),
                    ProviderTurnExecutionState::Reserved
                );
                assert!(store.verify_completion_action_journal_attestable().is_err());
                store.mark_completion_action_uncertain(&claim).unwrap();
                assert!(store.verify_completion_action_journal_attestable().is_err());
            }
        }
    }
}

#[test]
fn fault_matrix_repair_intent_never_retries_or_attests_an_ambiguous_mutation() {
    for fault in atomic_faults() {
        let temp = tempdir().unwrap();
        let store = store_at(temp.path());
        initialize(&store);
        let claim = store
            .claim_completion_action(0, CompletionActionId::generate())
            .unwrap();
        let intent = RepairIntent::new(
            claim.clone(),
            epoch(),
            Sha256Digest::compute(b"fault-matrix repair batch set"),
            vec![RepairBatchId::generate()],
        )
        .unwrap();

        assert!(
            store
                .persist_repair_intent_with_fault(intent.clone(), fault)
                .is_err(),
            "fault injection must interrupt the repair intent at {fault:?}"
        );
        match (
            recovery_state(fault),
            store.load_repair_intent(&claim).unwrap(),
        ) {
            (RecoveryState::Continue, RepairIntentRead::Missing) => {
                store
                    .persist_repair_intent(intent)
                    .expect("pre-rename repair intent failure permits a new intent");
                assert!(store.verify_completion_action_journal_attestable().is_err());
            }
            (RecoveryState::Reconcile, RepairIntentRead::Current(current)) => {
                assert_eq!(current.state(), &RepairIntentState::Started);
                assert!(
                    store.persist_repair_intent(intent).is_err(),
                    "a visible repair intent must fence duplicate repair execution"
                );
                store.mark_repair_intent_uncertain(&claim).unwrap();
                store.mark_completion_action_uncertain(&claim).unwrap();
                assert!(matches!(
                    store.load_repair_intent(&claim).unwrap(),
                    RepairIntentRead::Current(intent)
                        if intent.state() == &RepairIntentState::Uncertain
                ));
                assert!(store.verify_completion_action_journal_attestable().is_err());
            }
            (expected, actual) => panic!(
                "fault {fault:?} expected recovery {expected:?}, found repair intent {actual:?}"
            ),
        }
    }
}

#[test]
fn duplicate_action_id_and_oversized_collection_fail_closed() {
    let mut journal = journal();
    let action_id = CompletionActionId::generate();
    journal.claim_next(0, action_id.clone()).unwrap();
    assert!(matches!(
        journal.claim_next(1, action_id),
        Err(CompletionActionJournalError::DuplicateActionId(_))
    ));

    let record = serde_json::to_value(&journal).unwrap()["actions"][0].clone();
    let mut oversized = serde_json::to_value(&journal).unwrap();
    oversized["generation"] = serde_json::json!(MAX_COMPLETION_ACTION_RECORDS + 1);
    oversized["actions"] = serde_json::Value::Array(
        std::iter::repeat_n(record, MAX_COMPLETION_ACTION_RECORDS + 1).collect(),
    );
    let error = CompletionActionJournal::parse_current(&serde_json::to_vec(&oversized).unwrap())
        .unwrap_err();
    assert!(error.to_string().contains("exceeds its maximum of"));
}

#[test]
fn unknown_v2_and_mixed_schemas_refuse_resume_deterministically() {
    let current = journal();
    let current_bytes = serde_json::to_vec(&current).unwrap();
    assert_eq!(
        parse_legacy_completion_action_journal(&current_bytes).unwrap_err(),
        CompletionActionJournalError::UnsupportedLegacyReaderSchema(
            COMPLETION_ACTION_JOURNAL_SCHEMA_VERSION
        )
    );

    let mixed = serde_json::json!({
        "schema_version": 1,
        "actions": [{"schema_version": 2}]
    });
    assert_eq!(
        parse_legacy_completion_action_journal(&serde_json::to_vec(&mixed).unwrap()).unwrap_err(),
        CompletionActionJournalError::MixedSchema
    );

    let temp = tempdir().unwrap();
    let path = action_journal_path(temp.path());
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::set_permissions(path.parent().unwrap(), fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(&path, br#"{"schema_version":999,"actions":[]}"#).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let error = store_at(temp.path())
        .load_completion_action_journal()
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("unsupported completion action journal schema version 999")
    );
}

#[test]
fn legacy_v1_journal_is_read_only_and_new_journals_never_write_it() {
    let temp = tempdir().unwrap();
    let path = action_journal_path(temp.path());
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::set_permissions(path.parent().unwrap(), fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(
        &path,
        br#"{"schema_version":1,"actions":[{"schema_version":1}]}"#,
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let store = store_at(temp.path());
    let CompletionActionJournalRead::LegacyV1(legacy) =
        store.load_completion_action_journal().unwrap()
    else {
        panic!("v1 journal must remain a read-only legacy document");
    };
    assert_eq!(legacy.record_count(), 1);
    let error = store
        .claim_completion_action(0, CompletionActionId::generate())
        .unwrap_err();
    assert!(error.to_string().contains("schema version 1 is read-only"));

    let clean = tempdir().unwrap();
    let clean_store = store_at(clean.path());
    let journal = initialize(&clean_store);
    let bytes = fs::read(action_journal_path(clean.path())).unwrap();
    assert!(
        bytes
            .windows(b"\"schema_version\": 2".len())
            .any(|window| { window == b"\"schema_version\": 2" })
    );
    assert_eq!(
        journal.schema_version(),
        COMPLETION_ACTION_JOURNAL_SCHEMA_VERSION
    );
}
