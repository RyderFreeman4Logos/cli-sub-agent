use std::collections::VecDeque;

use csa_session::convergence::{CampaignId, CandidateId, RepairBatchId, RootClusterId};

use super::completion::{
    AuthorizedRepairBatch, CompletionAction as Action, CompletionBudget as Budget,
    CompletionEvent as Event, CompletionOutcome, run_to_attestation,
};
use super::completion_tests::{FakePorts, artifact, clean_output, epoch, epoch_id};
use super::discovery_contract::{CampaignSelection, DiscoveryFocus};

#[tokio::test]
async fn run_to_attestation_replays_a_fake_history() {
    let campaign = CampaignId::generate();
    let candidate = CandidateId::generate();
    let root = RootClusterId::generate();
    let batch = AuthorizedRepairBatch::new(root.clone(), RepairBatchId::generate());
    let gate_artifact = artifact(b"gates");
    let review = clean_output();
    let published_gate_artifact = gate_artifact.clone();
    let published_review_artifact = review.artifact().clone();
    let published_model_identity = review.model_identity().clone();
    let mut ports = FakePorts::new(VecDeque::from([
        Ok(Event::DiscoveryCompleted {
            focus: DiscoveryFocus::Broad,
            selection: CampaignSelection::Fresh,
            campaign_id: campaign.clone(),
            epoch: epoch(2),
            candidates: vec![candidate.clone()],
        }),
        Ok(Event::ClustersReady {
            campaign_id: campaign.clone(),
            epoch_id: epoch_id(2),
            verified_candidates: vec![candidate],
            root_clusters: vec![root],
            repair_batches: vec![batch.clone()],
        }),
        Ok(Event::RepairsCompleted {
            campaign_id: campaign.clone(),
            previous_epoch_id: epoch_id(2),
            completed_batches: vec![batch.repair_batch_id().clone()],
            new_epoch: epoch(3),
        }),
        Ok(Event::DiscoveryCompleted {
            focus: DiscoveryFocus::Broad,
            selection: CampaignSelection::Continue(campaign.clone()),
            campaign_id: campaign.clone(),
            epoch: epoch(3),
            candidates: Vec::new(),
        }),
        Ok(Event::FinalGatesPassed {
            campaign_id: campaign.clone(),
            epoch_id: epoch_id(3),
            artifact: gate_artifact,
        }),
        Ok(Event::CleanRoomCompleted {
            campaign_id: campaign.clone(),
            epoch_id: epoch_id(3),
            output: review,
        }),
        Ok(Event::FinalPairPublished {
            campaign_id: campaign.clone(),
            epoch_id: epoch_id(3),
            gate_artifact: published_gate_artifact,
            review_artifact: published_review_artifact,
            model_identity: published_model_identity,
        }),
    ]));
    let outcome = run_to_attestation(
        &mut ports,
        Budget::new(16, 8).unwrap(),
        epoch(2),
        CampaignSelection::Fresh,
    )
    .await
    .unwrap();
    assert!(
        matches!(outcome, CompletionOutcome::Attested { campaign_id, epoch: final_epoch } if campaign_id == campaign && final_epoch == epoch(3))
    );
    assert_eq!(ports.actions().len(), 7);
    assert!(matches!(
        ports.actions(),
        [
            Action::Discover { .. },
            Action::VerifyAndCluster { .. },
            Action::RunAuthorizedRepairs { .. },
            Action::Discover { .. },
            Action::RunFinalGates { .. },
            Action::RunFreshCleanRoom { .. },
            Action::PublishFinalPair { .. },
        ]
    ));
}
