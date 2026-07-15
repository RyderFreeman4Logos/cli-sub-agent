use super::*;

#[tokio::test]
async fn restart_recovers_legacy_partial_attempt_from_durable_artifact_and_ledger_only() {
    let store = MemoryStore::default();
    let mut planning_probe = ScriptedProbe::stable(3);
    let mut planning_runner = ScriptedRunner::default();
    run_discovery_observation(&input(), &mut planning_probe, &mut planning_runner, &store)
        .await
        .expect_err("provider failure leaves the durable plan prefix");

    let mut first_runner = ScriptedRunner::pages([page(
        "complete",
        8,
        false,
        &[],
        vec![candidate("recoverable")],
    )]);
    let output = first_runner
        .run(DiscoveryRequest::for_test(frozen()))
        .await
        .expect("scripted provider should publish a durable page envelope");
    let (campaign_id, epoch_id, cell_id) = {
        let ledger = store.ledger.borrow();
        let campaign_id = ledger.entries()[0].campaign_id().clone();
        let epoch_id = ledger
            .entries()
            .iter()
            .find_map(|entry| match entry.event() {
                ConvergenceEvent::EpochOpened(record) => Some(record.id().clone()),
                _ => None,
            })
            .expect("durable plan must contain an epoch");
        let cell_id = ledger
            .entries()
            .iter()
            .find_map(|entry| match entry.event() {
                ConvergenceEvent::CoverageCellDefined(record) => Some(record.id().clone()),
                _ => None,
            })
            .expect("durable plan must contain a coverage cell");
        (campaign_id, epoch_id, cell_id)
    };
    let attempt = DiscoveryAttemptRecord::new(
        DiscoveryAttemptId::generate(),
        epoch_id,
        cell_id,
        Utc::now(),
        output.completion,
        output.model_identity,
        output.artifact,
        PAGE_CANDIDATE_LIMIT,
        1,
        false,
        Vec::new(),
    )
    .expect("legacy attempt fixture should be valid");
    store
        .append_batch(
            campaign_id,
            vec![ConvergenceEvent::DiscoveryAttemptRecorded(attempt)],
        )
        .expect("legacy partial attempt should be durable");

    let durable_artifacts = std::mem::take(&mut first_runner.artifacts);
    let mut resumed_probe = ScriptedProbe::stable(3);
    let mut resumed_runner = ScriptedRunner::pages([page("complete", 8, false, &[], Vec::new())]);
    resumed_runner.artifacts = durable_artifacts;
    let summary =
        run_discovery_observation(&input(), &mut resumed_probe, &mut resumed_runner, &store)
            .await
            .expect("resume should recover the missing candidate from durable page evidence");

    assert_eq!(
        summary.provider_calls,
        usize::try_from(summary.coverage_cell_count).expect("cell count fits usize") + 1
    );
    assert_eq!(summary.candidates, 1);
    assert_eq!(
        resumed_runner.requests.len(),
        summary.coverage_cell_count as usize,
        "recovery must saturate the recovered cell and then cover every other manifest cell"
    );
    assert_eq!(
        resumed_runner.requests[0].intent,
        DiscoveryRunIntent::SaturationChallenge
    );
}

#[tokio::test]
async fn discovery_page_events_are_never_published_as_a_partial_page() {
    let store = MemoryStore::default();
    let mut probe = ScriptedProbe::stable(5);
    let mut runner = ScriptedRunner::pages([
        page(
            "complete",
            8,
            false,
            &[],
            vec![candidate("atomic-page-candidate")],
        ),
        page("complete", 8, false, &[], Vec::new()),
    ]);

    run_discovery_observation(&input(), &mut probe, &mut runner, &store)
        .await
        .expect("complete discovery should succeed");

    for ledger in store.published_snapshots.borrow().iter() {
        let attempts = ledger
            .entries()
            .iter()
            .filter(|entry| matches!(entry.event(), ConvergenceEvent::DiscoveryAttemptRecorded(_)))
            .count();
        let finalized = ledger
            .entries()
            .iter()
            .filter(|entry| {
                matches!(
                    entry.event(),
                    ConvergenceEvent::DiscoveryAttemptFinalized(_)
                )
            })
            .count();
        assert_eq!(
            attempts, finalized,
            "each persisted snapshot must include every attempt's page finalization"
        );
    }
}

#[test]
fn discovery_run_output_carries_the_preparsed_durable_page() {
    let raw = page("partial", 8, true, &["unscanned semantic lens"], Vec::new());
    let output = output(ProviderTurnCompletion::Natural, raw).expect("output should parse page");

    assert_eq!(output.page.unscanned_items, ["unscanned semantic lens"]);
}

#[test]
fn durable_page_envelope_rejects_a_parsed_page_that_disagrees_with_its_raw_response() {
    let frozen = frozen();
    let raw = page("partial", 8, true, &["raw-unscanned-item"], Vec::new());
    let parsed = parse_discovery_page(&raw).expect("raw page should parse");
    let artifact =
        encode_discovery_page_artifact(&raw, &parsed, &frozen.provider_evidence.identity)
            .expect("page envelope should serialize");
    let digest = Sha256Digest::compute(&artifact);
    assert!(
        decode_discovery_page_artifact(&artifact, &digest, &frozen.provider_evidence.identity)
            .is_ok()
    );

    let mut tampered: Value = serde_json::from_slice(&artifact).expect("envelope is JSON");
    tampered["parsed_page"]["unscanned_items"] = json!(["tampered-item"]);
    let tampered = serde_json::to_vec(&tampered).expect("tampered envelope should serialize");
    let tampered_digest = Sha256Digest::compute(&tampered);
    let error = decode_discovery_page_artifact(
        &tampered,
        &tampered_digest,
        &frozen.provider_evidence.identity,
    )
    .expect_err("raw and parsed envelope members must agree");
    assert!(
        error
            .to_string()
            .contains("parsed page does not match its raw response")
    );
}
