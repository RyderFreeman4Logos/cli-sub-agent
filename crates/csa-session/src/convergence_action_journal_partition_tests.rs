//! Epoch-partition lifecycle coverage for completion action journals.

use tempfile::tempdir;

use crate::convergence::{
    CompletionActionId, CompletionActionJournalRead, CompletionActionState, ConvergenceEvent,
    ConvergenceLedgerStore, EpochRecord, GitObjectId, Sha256Digest,
};
use crate::convergence_attestation_tests::Fixture;

#[test]
fn completed_epoch_journal_is_preserved_when_the_current_epoch_selects_a_new_partition() {
    let fixture = Fixture::new();
    let temp = tempdir().unwrap();
    let store = ConvergenceLedgerStore::for_project_state_root(temp.path()).unwrap();
    store
        .append_batch(fixture.campaign_id.clone(), fixture.prefix_events.clone())
        .unwrap();

    store
        .initialize_completion_action_journal(
            fixture.campaign_id.clone(),
            fixture.epoch.id().clone(),
            fixture.policy_digest.clone(),
        )
        .unwrap();
    let completed_action = store
        .claim_completion_action(0, CompletionActionId::generate())
        .unwrap();
    store.finish_completion_action(&completed_action).unwrap();

    let next_epoch = EpochRecord::new(
        GitObjectId::parse("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb").unwrap(),
        GitObjectId::parse("cccccccccccccccccccccccccccccccccccccccc").unwrap(),
        Sha256Digest::compute(b"second completion action journal epoch"),
    );
    store
        .append(
            fixture.campaign_id.clone(),
            ConvergenceEvent::EpochOpened(next_epoch.clone()),
        )
        .unwrap();

    let selected = store
        .initialize_completion_action_journal(
            fixture.campaign_id.clone(),
            next_epoch.id().clone(),
            fixture.policy_digest.clone(),
        )
        .unwrap();
    assert_eq!(selected.epoch_id(), next_epoch.id());
    assert!(selected.actions().is_empty());

    let CompletionActionJournalRead::Current(previous) = store
        .load_completion_action_journal_for(
            &fixture.campaign_id,
            fixture.epoch.id(),
            &fixture.policy_digest,
        )
        .unwrap()
    else {
        panic!("completed prior epoch journal must be retained in its own partition");
    };
    assert_eq!(previous.actions().len(), 1);
    assert_eq!(previous.actions()[0].claim(), &completed_action);
    assert_eq!(
        previous.actions()[0].state(),
        CompletionActionState::Finished
    );

    let CompletionActionJournalRead::Current(active) =
        store.load_completion_action_journal().unwrap()
    else {
        panic!("current epoch selector must resolve to its current partition");
    };
    assert_eq!(active.epoch_id(), next_epoch.id());
}

#[test]
fn unresolved_epoch_journal_cannot_be_replaced_by_a_later_epoch_partition() {
    let fixture = Fixture::new();
    let temp = tempdir().unwrap();
    let store = ConvergenceLedgerStore::for_project_state_root(temp.path()).unwrap();
    store
        .append_batch(fixture.campaign_id.clone(), fixture.prefix_events.clone())
        .unwrap();
    store
        .initialize_completion_action_journal(
            fixture.campaign_id.clone(),
            fixture.epoch.id().clone(),
            fixture.policy_digest.clone(),
        )
        .unwrap();
    store
        .claim_completion_action(0, CompletionActionId::generate())
        .unwrap();

    let next_epoch = EpochRecord::new(
        GitObjectId::parse("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb").unwrap(),
        GitObjectId::parse("cccccccccccccccccccccccccccccccccccccccc").unwrap(),
        Sha256Digest::compute(b"unresolved completion action journal epoch"),
    );
    store
        .append(
            fixture.campaign_id.clone(),
            ConvergenceEvent::EpochOpened(next_epoch.clone()),
        )
        .unwrap();

    let error = store
        .initialize_completion_action_journal(
            fixture.campaign_id.clone(),
            next_epoch.id().clone(),
            fixture.policy_digest.clone(),
        )
        .unwrap_err();
    assert!(error.to_string().contains("contains an unresolved action"));
    assert!(matches!(
        store
            .load_completion_action_journal_for(
                &fixture.campaign_id,
                next_epoch.id(),
                &fixture.policy_digest,
            )
            .unwrap(),
        CompletionActionJournalRead::Missing
    ));
}
