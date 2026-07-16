use std::cell::RefCell;
use std::time::Duration;

use csa_session::convergence::{
    AdmittedModelIdentity, ArtifactEvidenceRef, CampaignId, CampaignRecord, CandidateId,
    CleanRoomReviewArtifactBindings, CleanRoomReviewRecord, CommandAuthorityCatalogIdentity,
    CommandAuthorityPolicy, CommandAuthoritySnapshot, CommandAuthoritySource, CompletionActionId,
    CompletionActionJournal, ConvergenceEvent, ConvergenceLedger, CsaSessionId, EpochId,
    EpochRecord, GitObjectId, LedgerEventId, ModelEvidence, ObservedToolEvidence,
    ProviderTurnExecutionId, RepairBatchId, RootClusterId, SessionRelativeArtifactPath,
    Sha256Digest,
};

use super::clean_room_v2::{CleanRoomReviewOutput, HostReviewArtifactStore, ReviewEnvelopeContext};
use super::completion::{
    AuthorizedRepairBatch, CompletionAction as Action, CompletionBudget as Budget,
    CompletionError as Failure, CompletionEvent as Event,
    CompletionExecutionReservation as ExecutionReservation, CompletionPhase as Phase,
    CompletionState as State, ProviderTurnEvidence, ProviderTurnReconciliation,
    reconcile_provider_turns, reduce_completion,
};
use super::discovery_contract::{CampaignSelection, DiscoveryFocus, TargetedDiscoveryFocus};
use super::engine::{DiscoveryRequest, FrozenWorkspace, LedgerPort, initialize_campaign};

fn frozen() -> FrozenWorkspace {
    FrozenWorkspace::new(
        "1111111111111111111111111111111111111111",
        "2222222222222222222222222222222222222222",
        Sha256Digest::compute(b"legacy prompt fixture"),
        true,
        true,
    )
    .expect("frozen fixture")
}

pub(super) fn epoch(head: u8) -> EpochRecord {
    EpochRecord::new(
        GitObjectId::parse(&"11".repeat(20)).expect("base"),
        GitObjectId::parse(&format!("{head:02x}").repeat(20)).expect("head"),
        Sha256Digest::compute(&[head]),
    )
}

pub(super) fn epoch_id(head: u8) -> EpochId {
    epoch(head).id().clone()
}

pub(super) fn provider_reservation(
    reserved_turns: u32,
) -> csa_session::convergence::ProviderTurnReservation {
    let mut journal = CompletionActionJournal::new(
        CampaignId::generate(),
        epoch_id(2),
        Sha256Digest::compute(b"completion provider turn test"),
    );
    let claim = journal
        .claim_next(0, CompletionActionId::generate())
        .expect("action claim");
    journal
        .reserve_provider_turn(&claim, ProviderTurnExecutionId::generate(), reserved_turns)
        .expect("provider reservation")
}

fn authority(model: &str) -> CommandAuthoritySnapshot {
    CommandAuthoritySnapshot::new(
        CommandAuthoritySource::direct("completion test").expect("source"),
        CommandAuthorityPolicy::new(false, Vec::new(), false, true).expect("policy"),
        CommandAuthorityCatalogIdentity::new("test catalog", "v1").expect("catalog"),
        vec![AdmittedModelIdentity::new("codex", "openai", model, "high").expect("model")],
    )
    .expect("authority")
}

pub(super) fn artifact(label: &[u8]) -> ArtifactEvidenceRef {
    ArtifactEvidenceRef::new(
        CsaSessionId::generate(),
        SessionRelativeArtifactPath::new("output/review.json").expect("path"),
        Sha256Digest::compute(label),
    )
}

fn clean_room_json(findings: serde_json::Value, questions: &[&str], unchecked: &[&str]) -> String {
    serde_json::json!({
        "findings": findings,
        "questions": questions,
        "unchecked_items": unchecked,
        "review_text": "Clean-room review completed.",
    })
    .to_string()
}

fn finding_json() -> serde_json::Value {
    serde_json::json!([{
        "semantic_identity": {
            "violated_invariant": "published state is atomic",
            "trigger_failure_mode": "second terminal write",
            "primary_component": "completion reducer",
            "bug_class": "duplicate publication"
        },
        "review_text": "Terminal publication can be repeated."
    }])
}

fn parse_output(raw: &str) -> anyhow::Result<CleanRoomReviewOutput> {
    let directory = tempfile::tempdir()?;
    let session_id = CsaSessionId::generate();
    let store = HostReviewArtifactStore::new(
        directory.path(),
        session_id,
        SessionRelativeArtifactPath::new("output")?,
    )?;
    let evidence = ModelEvidence::host_observed(
        AdmittedModelIdentity::new("codex", "openai", "gpt-5.6", "high")?,
        ObservedToolEvidence::new("codex", "test-version")?,
        None,
        ProviderTurnExecutionId::generate(),
    )?;
    let context = ReviewEnvelopeContext::new(
        CampaignId::generate(),
        epoch(2),
        artifact(b"clean-room-gate"),
        evidence,
    );
    store.publish(&context, raw)
}

pub(super) fn clean_output() -> CleanRoomReviewOutput {
    parse_output(&clean_room_json(serde_json::json!([]), &[], &[])).expect("clean output")
}

fn finding_output() -> CleanRoomReviewOutput {
    parse_output(&clean_room_json(finding_json(), &[], &[])).expect("finding output")
}

fn start(budget: Budget) -> (State, Action) {
    let transition = reduce_completion(
        &State::new(budget),
        Event::Started {
            epoch: epoch(2),
            selection: CampaignSelection::Fresh,
        },
    )
    .expect("start transition");
    (
        transition.state,
        transition.action.expect("discovery action"),
    )
}

fn full_budget() -> Budget {
    Budget::new(32, 16).unwrap()
}

fn discovery_event(
    action: &Action,
    campaign_id: CampaignId,
    candidates: Vec<CandidateId>,
) -> Event {
    let Action::Discover {
        focus,
        selection,
        epoch,
    } = action
    else {
        panic!("expected discovery action")
    };
    Event::DiscoveryCompleted {
        focus: focus.clone(),
        selection: selection.clone(),
        campaign_id,
        epoch: epoch.clone(),
        candidates,
    }
}

fn discovered_candidate() -> (State, CampaignId, CandidateId) {
    let (state, discovery) = start(full_budget());
    let campaign = CampaignId::generate();
    let candidate = CandidateId::generate();
    let transition = reduce_completion(
        &state,
        discovery_event(&discovery, campaign.clone(), vec![candidate.clone()]),
    )
    .unwrap();
    (transition.state, campaign, candidate)
}

fn clean_room_event(campaign_id: CampaignId, output: CleanRoomReviewOutput) -> Event {
    Event::CleanRoomCompleted {
        campaign_id,
        epoch_id: epoch_id(2),
        output,
    }
}

fn publication_event(action: &Action) -> Event {
    let Action::PublishFinalPair {
        campaign_id,
        epoch,
        gate_artifact,
        clean_room,
    } = action
    else {
        panic!("expected terminal publication action")
    };
    Event::FinalPairPublished {
        campaign_id: campaign_id.clone(),
        epoch_id: epoch.id().clone(),
        gate_artifact: gate_artifact.clone(),
        review_artifact: clean_room.artifact().clone(),
        model_evidence: clean_room.model_evidence().clone(),
    }
}

fn reach_clean_room() -> (State, Action, CampaignId) {
    let (state, discovery) = start(full_budget());
    let campaign = CampaignId::generate();
    let gates = reduce_completion(
        &state,
        discovery_event(&discovery, campaign.clone(), Vec::new()),
    )
    .expect("zero discovery");
    let clean_room = reduce_completion(
        &gates.state,
        Event::FinalGatesPassed {
            campaign_id: campaign.clone(),
            epoch_id: epoch_id(2),
            artifact: artifact(b"gates"),
        },
    )
    .expect("gates");
    (
        clean_room.state,
        clean_room.action.expect("clean room"),
        campaign,
    )
}

pub(super) fn reach_targeted_discovery() -> (State, Action, CampaignId) {
    let (state, _, campaign) = reach_clean_room();
    let targeted =
        reduce_completion(&state, clean_room_event(campaign.clone(), finding_output())).unwrap();
    (
        targeted.state,
        targeted.action.expect("targeted discovery"),
        campaign,
    )
}

#[test]
fn broad_prompt_is_byte_for_byte_legacy_compatible() {
    let request = DiscoveryRequest::for_test(frozen());
    assert_eq!(request.focus, DiscoveryFocus::Broad);
    assert_eq!(request.campaign_selection, CampaignSelection::LegacyReuse);
    let prompt = super::runner::build_discovery_prompt(&request);
    assert_eq!(
        Sha256Digest::compute(prompt.as_bytes()).as_str(),
        "sha256:b7b9fd9b176b538294df2d3aba104c6b4ce3b9e2eb1e94f74e71ddec8495e09a"
    );
}

#[test]
fn completion_executes_exactly_one_authorized_batch_per_root_cluster() {
    let (state, campaign, candidate) = discovered_candidate();
    let clusters = vec![RootClusterId::generate(), RootClusterId::generate()];
    let batches = clusters
        .iter()
        .cloned()
        .map(|cluster| AuthorizedRepairBatch::new(cluster, RepairBatchId::generate()))
        .collect();
    let repairs = reduce_completion(
        &state,
        Event::ClustersReady {
            campaign_id: campaign,
            epoch_id: epoch_id(2),
            verified_candidates: vec![candidate],
            root_clusters: clusters,
            repair_batches: batches,
        },
    )
    .unwrap();
    let Action::RunAuthorizedRepairs { batches, .. } = repairs.action.unwrap() else {
        panic!("expected repairs")
    };
    assert_eq!(batches.len(), 2);
}

#[test]
fn completion_rejects_cluster_batch_cardinality_mismatch() {
    let (state, campaign, _) = discovered_candidate();
    let error = reduce_completion(
        &state,
        Event::ClustersReady {
            campaign_id: campaign,
            epoch_id: epoch_id(2),
            verified_candidates: Vec::new(),
            root_clusters: vec![RootClusterId::generate()],
            repair_batches: Vec::new(),
        },
    )
    .unwrap_err();
    assert!(matches!(error, Failure::CardinalityMismatch));
}

#[test]
fn completion_reopens_epoch_after_repair_and_rediscovers_broadly() {
    let (state, campaign, candidate) = discovered_candidate();
    let cluster = RootClusterId::generate();
    let batch = AuthorizedRepairBatch::new(cluster.clone(), RepairBatchId::generate());
    assert_eq!(batch.root_cluster_id(), &cluster);
    let repairs = reduce_completion(
        &state,
        Event::ClustersReady {
            campaign_id: campaign.clone(),
            epoch_id: epoch_id(2),
            verified_candidates: vec![candidate],
            root_clusters: vec![cluster],
            repair_batches: vec![batch.clone()],
        },
    )
    .unwrap();
    let reopened = reduce_completion(
        &repairs.state,
        Event::RepairsCompleted {
            campaign_id: campaign.clone(),
            previous_epoch_id: epoch_id(2),
            completed_batches: vec![batch.repair_batch_id().clone()],
            new_epoch: epoch(3),
        },
    )
    .unwrap();
    assert!(matches!(
        reopened.action,
        Some(Action::Discover {
            focus: DiscoveryFocus::Broad,
            selection: CampaignSelection::Continue(ref id),
            ref epoch,
        }) if id == &campaign && epoch == &self::epoch(3)
    ));
}

#[test]
fn completion_skips_verification_for_zero_candidate_epoch() {
    let (state, discovery) = start(full_budget());
    let next = reduce_completion(
        &state,
        discovery_event(&discovery, CampaignId::generate(), Vec::new()),
    )
    .unwrap();
    assert!(matches!(next.action, Some(Action::RunFinalGates { .. })));
}

#[test]
fn completion_runs_final_gates_before_fresh_clean_room_review() {
    let (state, action, campaign) = reach_clean_room();
    assert_eq!(state.phase(), Phase::RunFreshCleanRoom);
    assert!(
        matches!(action, Action::RunFreshCleanRoom { campaign_id, .. } if campaign_id == campaign)
    );
}

#[test]
fn clean_room_findings_start_targeted_discovery_in_a_fresh_campaign() {
    let (state, action, campaign) = reach_targeted_discovery();
    assert!(matches!(
        &action,
        Action::Discover {
            focus: DiscoveryFocus::Targeted(_),
            selection: CampaignSelection::Fresh,
            ..
        }
    ));
    let error =
        reduce_completion(&state, discovery_event(&action, campaign, Vec::new())).unwrap_err();
    assert_eq!(error, Failure::IdentityMismatch);
}

#[test]
fn clean_room_questions_or_unchecked_items_block_without_targeted_campaign() {
    for output in [
        parse_output(&clean_room_json(serde_json::json!([]), &["question"], &[])).unwrap(),
        parse_output(&clean_room_json(serde_json::json!([]), &[], &["unchecked"])).unwrap(),
    ] {
        let (state, _, campaign) = reach_clean_room();
        let error = reduce_completion(&state, clean_room_event(campaign, output)).unwrap_err();
        assert!(matches!(error, Failure::BlockedCleanRoom));
    }
}

#[test]
fn completion_rejects_attestation_until_clean_room_reports_zero_zero_zero() {
    let (state, action, _) = reach_targeted_discovery();
    assert_ne!(state.phase(), Phase::Attested);
    assert!(!matches!(action, Action::PublishFinalPair { .. }));
}

#[test]
fn completion_publishes_only_one_atomic_terminal_action() {
    let (state, _, campaign) = reach_clean_room();
    let publish =
        reduce_completion(&state, clean_room_event(campaign.clone(), clean_output())).unwrap();
    assert!(matches!(
        publish.action.as_ref(),
        Some(Action::PublishFinalPair { .. })
    ));
    let action = publish.action.as_ref().expect("publication action");
    let Event::FinalPairPublished {
        campaign_id,
        epoch_id,
        review_artifact,
        model_evidence,
        ..
    } = publication_event(action)
    else {
        unreachable!()
    };
    assert_eq!(
        reduce_completion(
            &publish.state,
            Event::FinalPairPublished {
                campaign_id,
                epoch_id,
                gate_artifact: artifact(b"wrong-gates"),
                review_artifact,
                model_evidence,
            },
        )
        .unwrap_err(),
        Failure::IdentityMismatch
    );
    let terminal = reduce_completion(&publish.state, publication_event(action)).unwrap();
    assert_eq!(terminal.state.phase(), Phase::Attested);
    assert!(terminal.action.is_none());
    assert!(reduce_completion(&terminal.state, Event::MaxRoundsReached).is_err());
}

#[test]
fn completion_never_maps_budget_exhaustion_or_max_rounds_to_attested() {
    let (state, discovery) = start(Budget::new(8, 1).unwrap());
    let transition = reduce_completion(
        &state,
        discovery_event(
            &discovery,
            CampaignId::generate(),
            vec![CandidateId::generate()],
        ),
    )
    .expect("events do not infer provider usage from action kind");
    assert_eq!(transition.state.provider_turns, 0);
    assert!(matches!(
        transition.action,
        Some(Action::VerifyAndCluster { .. })
    ));
    let reservation = provider_reservation(1);
    assert_eq!(
        reconcile_provider_turns(
            &state,
            &ExecutionReservation::Provider(reservation.clone()),
            &ProviderTurnReconciliation::Reconciled {
                reservation,
                host_observed_turn_delta: 1,
                evidence: ProviderTurnEvidence::Transport(
                    csa_process::ProviderTurnCompletion::Natural
                ),
            },
        ),
        Err(Failure::BudgetExhausted)
    );
    for (event, expected) in [
        (Event::MaxRoundsReached, Failure::MaxRoundsReached),
        (Event::DriftDetected, Failure::DriftDetected),
        (Event::CleanupUncertain, Failure::CleanupUncertain),
        (Event::ProviderUnavailable, Failure::ProviderUnavailable),
        (
            Event::IncompleteProviderOutput,
            Failure::IncompleteProviderOutput,
        ),
    ] {
        assert_eq!(reduce_completion(&state, event).unwrap_err(), expected);
    }
}

#[test]
fn targeted_prompt_contains_artifact_digest_and_semantic_identities_not_transcript() {
    let output = finding_output();
    assert!(
        output
            .artifact()
            .path()
            .as_str()
            .starts_with("output/clean-room-review-v2-")
    );
    assert_eq!(output.model_evidence().admitted_model().model(), "gpt-5.6");
    let focus = TargetedDiscoveryFocus::from_review(&output).expect("targeted focus");
    let stable_id = focus.semantic_finding_ids()[0].as_str().to_string();
    let digest = focus.artifact().digest().to_string();
    let mut request = DiscoveryRequest::for_test(frozen());
    request.focus = DiscoveryFocus::Targeted(focus);
    request.campaign_selection = CampaignSelection::Fresh;
    let prompt = super::runner::build_discovery_prompt(&request);
    assert!(prompt.contains(&digest));
    assert!(prompt.contains(&stable_id));
    assert!(prompt.contains("\"kind\":\"convergence_discovery_page\""));
    assert!(!prompt.contains("Terminal publication can be repeated"));
    assert!(!prompt.contains("A repeated event reaches"));
    let clean_room = super::discovery_prompt::build_clean_room_prompt(&epoch(2), output.artifact());
    assert!(clean_room.contains("\"findings\""));
    assert!(!clean_room.contains("\"model_identity\""));
    assert!(clean_room.contains(output.artifact().digest().as_str()));
}

#[test]
fn clean_room_parser_rejects_unknown_fields_prose_partial_or_ambiguous_json() {
    let valid = clean_room_json(finding_json(), &[], &[]);
    assert_eq!(parse_output(&valid).unwrap().findings().len(), 1);
    let mut unknown: serde_json::Value = serde_json::from_str(&valid).unwrap();
    unknown["unknown"] = serde_json::json!(true);
    for invalid in [
        unknown.to_string(),
        format!("prose {valid}"),
        valid[..valid.len() - 1].to_string(),
        format!("{valid}{valid}"),
    ] {
        assert!(parse_output(&invalid).is_err(), "accepted {invalid}");
    }
    let duplicate = clean_room_json(
        serde_json::json!([finding_json()[0], finding_json()[0]]),
        &[],
        &[],
    );
    assert!(parse_output(&duplicate).is_err());

    let mut forbidden_path: serde_json::Value = serde_json::from_str(&valid).unwrap();
    forbidden_path["findings"][0]["path"] = serde_json::json!("src/lib.rs");
    assert!(parse_output(&forbidden_path.to_string()).is_err());

    let mut control_text: serde_json::Value = serde_json::from_str(&valid).unwrap();
    control_text["findings"][0]["review_text"] = serde_json::json!("line\nbreak");
    assert!(parse_output(&control_text.to_string()).is_err());
}

struct FakeLedger(RefCell<ConvergenceLedger>);

impl LedgerPort for FakeLedger {
    fn load(&self) -> anyhow::Result<ConvergenceLedger> {
        Ok(self.0.borrow().clone())
    }

    fn append_batch(
        &self,
        campaign_id: CampaignId,
        events: Vec<ConvergenceEvent>,
    ) -> anyhow::Result<()> {
        self.0.borrow_mut().append_batch(campaign_id, events)
    }
}

fn matching_ledger(
    policy: &Sha256Digest,
    command_authority: &CommandAuthoritySnapshot,
) -> (ConvergenceLedger, CampaignRecord) {
    let campaign = CampaignRecord::new(
        CampaignId::generate(),
        chrono::Utc::now(),
        Some(policy.clone()),
        command_authority.clone(),
    );
    let mut ledger = ConvergenceLedger::empty();
    ledger
        .append(
            campaign.id().clone(),
            ConvergenceEvent::CampaignStarted(campaign.clone()),
        )
        .unwrap();
    (ledger, campaign)
}

#[test]
fn fresh_selection_never_reuses_matching_campaign() {
    let policy = Sha256Digest::compute(b"policy");
    let command_authority = authority("gpt-5.6");
    let (ledger, existing) = matching_ledger(&policy, &command_authority);
    let store = FakeLedger(RefCell::new(ledger));
    let mut persistence = Duration::ZERO;
    let (fresh, _) = initialize_campaign(
        &store,
        &epoch(2),
        &policy,
        &command_authority,
        &CampaignSelection::Fresh,
        &mut persistence,
    )
    .unwrap();
    assert_ne!(fresh.id(), existing.id());
}

fn terminal_ledger(
    policy: &Sha256Digest,
    command_authority: &CommandAuthoritySnapshot,
) -> (ConvergenceLedger, CampaignRecord) {
    let (ledger, campaign) = matching_ledger(policy, command_authority);
    let model_evidence = ModelEvidence::host_observed(
        command_authority.ordered_admitted()[0].clone(),
        ObservedToolEvidence::new("codex", "test-version").unwrap(),
        None,
        ProviderTurnExecutionId::generate(),
    )
    .unwrap();
    let review = CleanRoomReviewRecord::new(
        campaign.id().clone(),
        &epoch(2),
        model_evidence,
        CleanRoomReviewArtifactBindings::new(artifact(b"terminal-gate"), artifact(b"terminal")),
        0,
        0,
        0,
    )
    .unwrap();
    let mut value = serde_json::to_value(ledger).unwrap();
    let mut entry = value["entries"][0].clone();
    entry["sequence"] = serde_json::json!(2);
    entry["event_id"] = serde_json::to_value(LedgerEventId::generate()).unwrap();
    entry["event"] =
        serde_json::to_value(ConvergenceEvent::FinalReviewRecorded(Box::new(review))).unwrap();
    value["entries"].as_array_mut().unwrap().push(entry);
    (serde_json::from_value(value).unwrap(), campaign)
}

#[test]
fn continue_selection_rejects_wrong_or_terminal_campaign() {
    let policy = Sha256Digest::compute(b"policy");
    let command_authority = authority("gpt-5.6");
    let (ledger, _) = matching_ledger(&policy, &command_authority);
    let store = FakeLedger(RefCell::new(ledger));
    let mut persistence = Duration::ZERO;
    assert!(
        initialize_campaign(
            &store,
            &epoch(2),
            &policy,
            &command_authority,
            &CampaignSelection::Continue(CampaignId::generate()),
            &mut persistence,
        )
        .is_err()
    );

    let (ledger, terminal) = terminal_ledger(&policy, &command_authority);
    let store = FakeLedger(RefCell::new(ledger));
    persistence = Duration::ZERO;
    assert!(
        initialize_campaign(
            &store,
            &epoch(2),
            &policy,
            &command_authority,
            &CampaignSelection::Continue(terminal.id().clone()),
            &mut persistence,
        )
        .is_err()
    );
}
