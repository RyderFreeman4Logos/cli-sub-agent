use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Barrier};

use tempfile::tempdir;

use crate::convergence::{
    COMPLETION_ACTION_JOURNAL_SCHEMA_VERSION, CampaignId, CompletionActionId,
    CompletionActionJournal, CompletionActionJournalError, CompletionActionJournalRead,
    CompletionActionState, ConvergenceLedgerStore, EpochRecord, GitObjectId,
    MAX_COMPLETION_ACTION_RECORDS, Sha256Digest, parse_legacy_completion_action_journal,
};

fn epoch_id() -> crate::convergence::EpochId {
    EpochRecord::new(
        GitObjectId::parse("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap(),
        GitObjectId::parse("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb").unwrap(),
        Sha256Digest::compute(b"completion action journal epoch"),
    )
    .id()
    .clone()
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
