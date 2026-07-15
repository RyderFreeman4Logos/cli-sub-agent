use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;

use csa_session::convergence::{CampaignId, CandidateId, EpochId, EpochRecord, RootClusterId};

pub(crate) use super::completion_types::{
    AuthorizedRepairBatch, CompletionAction, CompletionBudget, CompletionError, CompletionEvent,
    CompletionOutcome, CompletionPhase, CompletionPortError, CompletionState, CompletionTransition,
};
use super::discovery_contract::{CampaignSelection, DiscoveryFocus, TargetedDiscoveryFocus};

pub(crate) fn reduce_completion(
    state: &CompletionState,
    event: CompletionEvent,
) -> Result<CompletionTransition, CompletionError> {
    if state.phase == CompletionPhase::Attested {
        return Err(CompletionError::InvalidTransition(
            "attestation is terminal",
        ));
    }
    match event {
        CompletionEvent::DriftDetected => return Err(CompletionError::DriftDetected),
        CompletionEvent::CleanupUncertain => return Err(CompletionError::CleanupUncertain),
        CompletionEvent::ProviderUnavailable => return Err(CompletionError::ProviderUnavailable),
        CompletionEvent::IncompleteProviderOutput => {
            return Err(CompletionError::IncompleteProviderOutput);
        }
        CompletionEvent::MaxRoundsReached => return Err(CompletionError::MaxRoundsReached),
        _ => {}
    }
    let mut next = state.clone();
    next.cycles = next
        .cycles
        .checked_add(1)
        .ok_or(CompletionError::BudgetExhausted)?;
    if next.cycles > next.budget.max_cycles {
        return Err(CompletionError::BudgetExhausted);
    }
    let action = match (state.phase, event) {
        (CompletionPhase::Start, CompletionEvent::Started { epoch, selection }) => {
            if matches!(selection, CampaignSelection::LegacyReuse) {
                return Err(CompletionError::InvalidTransition(
                    "completion must start fresh or continue an exact campaign",
                ));
            }
            next.phase = CompletionPhase::Discover;
            next.epoch = Some(epoch.clone());
            next.discovery_focus = Some(DiscoveryFocus::Broad);
            next.campaign_selection = Some(selection.clone());
            CompletionAction::Discover {
                focus: DiscoveryFocus::Broad,
                selection,
                epoch,
            }
        }
        (
            CompletionPhase::Discover,
            CompletionEvent::DiscoveryCompleted {
                focus,
                selection,
                campaign_id,
                epoch,
                candidates,
            },
        ) => {
            require_discovery_identity(state, &focus, &selection, &campaign_id, &epoch)?;
            require_unique(candidates.iter().map(CandidateId::as_str))?;
            next.campaign_id = Some(campaign_id.clone());
            next.epoch = Some(epoch.clone());
            next.discovery_focus = None;
            next.campaign_selection = None;
            if candidates.is_empty() {
                next.pending_candidates.clear();
                next.phase = CompletionPhase::RunFinalGates;
                CompletionAction::RunFinalGates { campaign_id, epoch }
            } else {
                next.pending_candidates.clone_from(&candidates);
                next.phase = CompletionPhase::VerifyAndCluster;
                CompletionAction::VerifyAndCluster {
                    campaign_id,
                    epoch,
                    candidates,
                }
            }
        }
        (
            CompletionPhase::VerifyAndCluster,
            CompletionEvent::ClustersReady {
                campaign_id,
                epoch_id,
                verified_candidates,
                root_clusters,
                repair_batches,
            },
        ) => {
            require_current(state, &campaign_id, &epoch_id)?;
            if verified_candidates != state.pending_candidates {
                return Err(CompletionError::CardinalityMismatch);
            }
            validate_cluster_batches(&root_clusters, &repair_batches)?;
            next.pending_candidates.clear();
            if repair_batches.is_empty() {
                let epoch = current_epoch(state)?.clone();
                next.phase = CompletionPhase::RunFinalGates;
                CompletionAction::RunFinalGates { campaign_id, epoch }
            } else {
                let epoch = current_epoch(state)?.clone();
                next.phase = CompletionPhase::RunAuthorizedRepairs;
                next.repair_batches.clone_from(&repair_batches);
                CompletionAction::RunAuthorizedRepairs {
                    campaign_id,
                    epoch,
                    batches: repair_batches,
                }
            }
        }
        (
            CompletionPhase::RunAuthorizedRepairs,
            CompletionEvent::RepairsCompleted {
                campaign_id,
                previous_epoch_id,
                completed_batches,
                new_epoch,
            },
        ) => {
            require_current(state, &campaign_id, &previous_epoch_id)?;
            let expected = state
                .repair_batches
                .iter()
                .map(|batch| batch.repair_batch_id.clone())
                .collect::<Vec<_>>();
            if completed_batches != expected {
                return Err(CompletionError::CardinalityMismatch);
            }
            let previous = current_epoch(state)?;
            if new_epoch.id() == previous.id() || new_epoch.head_oid() == previous.head_oid() {
                return Err(CompletionError::EpochDidNotChange);
            }
            next.phase = CompletionPhase::Discover;
            next.epoch = Some(new_epoch.clone());
            next.repair_batches.clear();
            next.discovery_focus = Some(DiscoveryFocus::Broad);
            next.campaign_selection = Some(CampaignSelection::Continue(campaign_id.clone()));
            CompletionAction::Discover {
                focus: DiscoveryFocus::Broad,
                selection: CampaignSelection::Continue(campaign_id),
                epoch: new_epoch,
            }
        }
        (
            CompletionPhase::RunFinalGates,
            CompletionEvent::FinalGatesPassed {
                campaign_id,
                epoch_id,
                artifact,
            },
        ) => {
            require_current(state, &campaign_id, &epoch_id)?;
            let epoch = current_epoch(state)?.clone();
            next.phase = CompletionPhase::RunFreshCleanRoom;
            next.gate_artifact = Some(artifact.clone());
            CompletionAction::RunFreshCleanRoom {
                campaign_id,
                epoch,
                gate_artifact: artifact,
            }
        }
        (
            CompletionPhase::RunFreshCleanRoom,
            CompletionEvent::CleanRoomCompleted {
                campaign_id,
                epoch_id,
                output,
            },
        ) => {
            require_current(state, &campaign_id, &epoch_id)?;
            if !output.questions().is_empty() || !output.unchecked_items().is_empty() {
                return Err(CompletionError::BlockedCleanRoom);
            }
            let epoch = current_epoch(state)?.clone();
            if output.findings().is_empty() {
                next.phase = CompletionPhase::PublishFinalPair;
                next.clean_room = Some(output.clone());
                CompletionAction::PublishFinalPair {
                    campaign_id,
                    epoch,
                    gate_artifact: state
                        .gate_artifact
                        .clone()
                        .ok_or(CompletionError::InvalidTransition("missing gate evidence"))?,
                    clean_room: Box::new(output),
                }
            } else {
                let focus = DiscoveryFocus::Targeted(
                    TargetedDiscoveryFocus::from_review(&output)
                        .map_err(|_| CompletionError::BlockedCleanRoom)?,
                );
                next.phase = CompletionPhase::Discover;
                next.discovery_focus = Some(focus.clone());
                next.campaign_selection = Some(CampaignSelection::Fresh);
                next.gate_artifact = None;
                CompletionAction::Discover {
                    focus,
                    selection: CampaignSelection::Fresh,
                    epoch,
                }
            }
        }
        (
            CompletionPhase::PublishFinalPair,
            CompletionEvent::FinalPairPublished {
                campaign_id,
                epoch_id,
                gate_artifact,
                review_artifact,
                model_identity,
            },
        ) => {
            require_current(state, &campaign_id, &epoch_id)?;
            let expected_gate =
                state
                    .gate_artifact
                    .as_ref()
                    .ok_or(CompletionError::InvalidTransition(
                        "terminal publication lacks gate evidence",
                    ))?;
            let expected_review =
                state
                    .clean_room
                    .as_ref()
                    .ok_or(CompletionError::InvalidTransition(
                        "terminal publication lacks review evidence",
                    ))?;
            if expected_gate != &gate_artifact
                || expected_review.artifact() != &review_artifact
                || expected_review.model_identity() != &model_identity
            {
                return Err(CompletionError::IdentityMismatch);
            }
            next.phase = CompletionPhase::Attested;
            return Ok(CompletionTransition {
                state: next,
                action: None,
            });
        }
        _ => {
            return Err(CompletionError::InvalidTransition(
                "event does not match phase",
            ));
        }
    };
    issue(next, action)
}

fn issue(
    mut state: CompletionState,
    action: CompletionAction,
) -> Result<CompletionTransition, CompletionError> {
    if action.consumes_provider_action() {
        state.provider_actions = state
            .provider_actions
            .checked_add(1)
            .ok_or(CompletionError::BudgetExhausted)?;
        if state.provider_actions > state.budget.max_provider_actions {
            return Err(CompletionError::BudgetExhausted);
        }
    }
    Ok(CompletionTransition {
        state,
        action: Some(action),
    })
}

fn require_discovery_identity(
    state: &CompletionState,
    focus: &DiscoveryFocus,
    selection: &CampaignSelection,
    campaign_id: &CampaignId,
    epoch: &EpochRecord,
) -> Result<(), CompletionError> {
    if state.discovery_focus.as_ref() != Some(focus)
        || state.campaign_selection.as_ref() != Some(selection)
        || state.epoch.as_ref() != Some(epoch)
    {
        return Err(CompletionError::IdentityMismatch);
    }
    match selection {
        CampaignSelection::Continue(expected) if expected != campaign_id => {
            Err(CompletionError::IdentityMismatch)
        }
        CampaignSelection::Fresh if state.campaign_id.as_ref() == Some(campaign_id) => {
            Err(CompletionError::IdentityMismatch)
        }
        CampaignSelection::LegacyReuse => Err(CompletionError::InvalidTransition(
            "legacy campaign reuse is outside completion",
        )),
        CampaignSelection::Fresh | CampaignSelection::Continue(_) => Ok(()),
    }
}

fn require_current(
    state: &CompletionState,
    campaign_id: &CampaignId,
    epoch_id: &EpochId,
) -> Result<(), CompletionError> {
    if state.campaign_id.as_ref() != Some(campaign_id)
        || state.epoch.as_ref().map(EpochRecord::id) != Some(epoch_id)
    {
        return Err(CompletionError::IdentityMismatch);
    }
    Ok(())
}

fn current_epoch(state: &CompletionState) -> Result<&EpochRecord, CompletionError> {
    state
        .epoch
        .as_ref()
        .ok_or(CompletionError::InvalidTransition("missing exact epoch"))
}

fn require_unique<'a>(values: impl Iterator<Item = &'a str>) -> Result<(), CompletionError> {
    let mut seen = HashSet::new();
    if values.into_iter().all(|value| seen.insert(value)) {
        Ok(())
    } else {
        Err(CompletionError::DuplicateIdentity)
    }
}

fn validate_cluster_batches(
    root_clusters: &[RootClusterId],
    repair_batches: &[AuthorizedRepairBatch],
) -> Result<(), CompletionError> {
    if root_clusters.len() != repair_batches.len() {
        return Err(CompletionError::CardinalityMismatch);
    }
    require_unique(root_clusters.iter().map(RootClusterId::as_str))?;
    require_unique(
        repair_batches
            .iter()
            .map(|batch| batch.repair_batch_id.as_str()),
    )?;
    let expected = root_clusters.iter().collect::<HashSet<_>>();
    let actual = repair_batches
        .iter()
        .map(|batch| &batch.root_cluster_id)
        .collect::<HashSet<_>>();
    if expected != actual {
        return Err(CompletionError::CardinalityMismatch);
    }
    Ok(())
}

pub(crate) trait CompletionPorts {
    fn execute<'a>(
        &'a mut self,
        action: &'a CompletionAction,
    ) -> Pin<Box<dyn Future<Output = Result<CompletionEvent, CompletionPortError>> + 'a>>;
}

pub(crate) async fn run_targeted_discovery<P: CompletionPorts>(
    state: &CompletionState,
    action: &CompletionAction,
    ports: &mut P,
) -> Result<CompletionTransition, CompletionError> {
    if !matches!(
        action,
        CompletionAction::Discover {
            focus: DiscoveryFocus::Targeted(_),
            selection: CampaignSelection::Fresh,
            ..
        }
    ) {
        return Err(CompletionError::InvalidTransition(
            "targeted discovery must use a fresh campaign",
        ));
    }
    let event = ports
        .execute(action)
        .await
        .map_err(|error| CompletionError::Port(error.to_string()))?;
    reduce_completion(state, event)
}

pub(crate) async fn run_to_attestation<P: CompletionPorts>(
    ports: &mut P,
    budget: CompletionBudget,
    initial_epoch: EpochRecord,
    selection: CampaignSelection,
) -> Result<CompletionOutcome, CompletionError> {
    let mut transition = reduce_completion(
        &CompletionState::new(budget),
        CompletionEvent::Started {
            epoch: initial_epoch,
            selection,
        },
    )?;
    loop {
        let action = transition
            .action
            .as_ref()
            .ok_or(CompletionError::InvalidTransition(
                "nonterminal action is missing",
            ))?;
        transition = if matches!(
            action,
            CompletionAction::Discover {
                focus: DiscoveryFocus::Targeted(_),
                ..
            }
        ) {
            run_targeted_discovery(&transition.state, action, ports).await?
        } else {
            let event = ports
                .execute(action)
                .await
                .map_err(|error| CompletionError::Port(error.to_string()))?;
            reduce_completion(&transition.state, event)?
        };
        if transition.state.phase == CompletionPhase::Attested {
            return Ok(CompletionOutcome::Attested {
                campaign_id: transition
                    .state
                    .campaign_id
                    .clone()
                    .ok_or(CompletionError::IdentityMismatch)?,
                epoch: transition
                    .state
                    .epoch
                    .clone()
                    .ok_or(CompletionError::IdentityMismatch)?,
            });
        }
    }
}
