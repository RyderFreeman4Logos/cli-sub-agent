use super::*;

#[tokio::test]
async fn changed_command_authority_starts_a_fresh_campaign() {
    let store = MemoryStore::default();
    let mut first_probe = ScriptedProbe::stable(3);
    let mut first_runner = ScriptedRunner::pages([page("complete", 8, false, &[], Vec::new())]);
    let first = run_discovery_observation(&input(), &mut first_probe, &mut first_runner, &store)
        .await
        .unwrap();

    let changed = ObservationInput::new("main...HEAD", authority("gpt-5.5"));
    let mut changed_probe = ScriptedProbe::stable(3);
    let mut changed_runner = ScriptedRunner::pages([page("complete", 8, false, &[], Vec::new())]);
    let second =
        run_discovery_observation(&changed, &mut changed_probe, &mut changed_runner, &store)
            .await
            .unwrap();

    assert_ne!(first.campaign_id, second.campaign_id);
    assert_eq!(
        store
            .ledger
            .borrow()
            .entries()
            .iter()
            .filter(|entry| matches!(entry.event(), ConvergenceEvent::CampaignStarted(_)))
            .count(),
        2
    );
}

#[test]
fn frozen_execution_rejects_outside_identity_and_preserves_final_tuple() {
    let snapshot = authority("gpt-5.6");
    let admitted = super::super::runner::finalize_frozen_identity(
        &snapshot,
        "codex/openai/gpt-5.6/high",
        "codex",
    )
    .expect("the executed admitted tuple belongs to the snapshot");
    assert_eq!(admitted, snapshot.ordered_admitted()[0]);

    let error = super::super::runner::finalize_frozen_identity(
        &snapshot,
        "codex/openai/gpt-5.5/high",
        "codex",
    )
    .expect_err("an executor outside frozen authority must be blocked");
    assert!(
        error
            .to_string()
            .contains("outside frozen command authority")
    );
}
