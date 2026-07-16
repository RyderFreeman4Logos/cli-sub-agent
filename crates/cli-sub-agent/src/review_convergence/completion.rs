use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;

use csa_session::convergence::{CampaignId, CandidateId, EpochId, EpochRecord, RootClusterId};

pub(crate) use super::completion_resume::start_completion;
pub(crate) use super::completion_types::{
    AuthorizedRepairBatch, CompletionAction, CompletionBudget, CompletionError, CompletionEvent,
    CompletionExecutionReservation, CompletionOutcome, CompletionPhase, CompletionPortError,
    CompletionPortResult, CompletionStart, CompletionState, CompletionTransition,
    ProviderTurnAllowance, ProviderTurnEvidence, ProviderTurnReconciliation,
};
use super::discovery_contract::{CampaignSelection, DiscoveryFocus, TargetedDiscoveryFocus};

pub(super) const MAX_CLUSTERED_RESUME_SET_MEMBERS: usize = 1_000;

pub(crate) fn reduce_completion(
    state: &CompletionState,
    event: CompletionEvent,
) -> Result<CompletionTransition, CompletionError> {
    if state.phase == CompletionPhase::Attested {
        return Err(CompletionError::InvalidTransition(
            "attestation is terminal",
        ));
    }
    if state.phase == CompletionPhase::UsageIndeterminate {
        return Err(CompletionError::UsageIndeterminate);
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
            require_bounded_set(verified_candidates.len())?;
            validate_cluster_batches(&root_clusters, &repair_batches)?;
            next.pending_candidates.clear();
            next.clustered_candidates.clone_from(&verified_candidates);
            next.root_clusters.clone_from(&root_clusters);
            if repair_batches.is_empty() {
                let epoch = current_epoch(state)?.clone();
                next.phase = CompletionPhase::RunFinalGates;
                next.repair_batches.clear();
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
            next.clustered_candidates.clear();
            next.root_clusters.clear();
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
                model_evidence,
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
                || expected_review.model_evidence() != &model_evidence
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

pub(super) fn issue(
    state: CompletionState,
    action: CompletionAction,
) -> Result<CompletionTransition, CompletionError> {
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

pub(super) fn require_unique<'a>(
    values: impl Iterator<Item = &'a str>,
) -> Result<(), CompletionError> {
    let mut seen = HashSet::new();
    if values.into_iter().all(|value| seen.insert(value)) {
        Ok(())
    } else {
        Err(CompletionError::DuplicateIdentity)
    }
}

pub(super) fn require_bounded_set(length: usize) -> Result<(), CompletionError> {
    if length > MAX_CLUSTERED_RESUME_SET_MEMBERS {
        Err(CompletionError::CardinalityMismatch)
    } else {
        Ok(())
    }
}

pub(super) fn validate_cluster_batches(
    root_clusters: &[RootClusterId],
    repair_batches: &[AuthorizedRepairBatch],
) -> Result<(), CompletionError> {
    require_bounded_set(root_clusters.len())?;
    require_bounded_set(repair_batches.len())?;
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
    /// Durably reserve any provider turns before this action can send a provider request.
    ///
    /// The port decides whether the concrete operation is host-only or provider-backed. It must
    /// persist every returned provider reservation before `execute` begins external work.
    fn reserve_execution<'a>(
        &'a mut self,
        action: &'a CompletionAction,
        allowance: ProviderTurnAllowance,
    ) -> Pin<
        Box<dyn Future<Output = Result<CompletionExecutionReservation, CompletionPortError>> + 'a>,
    >;

    /// Execute an action after its reservation was durably recorded.
    ///
    /// The result always carries reconciliation evidence. A provider error, timeout, cancel, or
    /// incomplete result belongs in `CompletionPortResult::event`, never in an outer error that
    /// could discard already-incurred provider turns.
    fn execute<'a>(
        &'a mut self,
        action: &'a CompletionAction,
        reservation: &'a CompletionExecutionReservation,
    ) -> Pin<Box<dyn Future<Output = CompletionPortResult> + 'a>>;
}

fn remaining_provider_turns(
    state: &CompletionState,
) -> Result<ProviderTurnAllowance, CompletionError> {
    state
        .budget
        .max_provider_turns
        .checked_sub(state.provider_turns)
        .map(ProviderTurnAllowance::new)
        .ok_or(CompletionError::BudgetExhausted)
}

fn validate_reservation(
    reservation: &CompletionExecutionReservation,
    allowance: ProviderTurnAllowance,
) -> Result<(), CompletionError> {
    let CompletionExecutionReservation::Provider(reservation) = reservation else {
        return Ok(());
    };
    if reservation.reserved_turns() > allowance.remaining_turns() {
        return Err(CompletionError::BudgetExhausted);
    }
    Ok(())
}

pub(super) fn reconcile_provider_turns(
    state: &CompletionState,
    reservation: &CompletionExecutionReservation,
    reconciliation: &ProviderTurnReconciliation,
) -> Result<CompletionState, CompletionError> {
    let mut next = state.clone();
    match (reservation, reconciliation) {
        (CompletionExecutionReservation::HostOnly, ProviderTurnReconciliation::HostOnly) => {}
        (
            CompletionExecutionReservation::Provider(expected),
            ProviderTurnReconciliation::ReleasedBeforeSend { reservation },
        ) if expected == reservation => {}
        (
            CompletionExecutionReservation::Provider(expected),
            ProviderTurnReconciliation::Reconciled {
                reservation,
                host_observed_turn_delta,
                evidence,
            },
        ) if expected == reservation => {
            if *host_observed_turn_delta == 0
                || *host_observed_turn_delta > reservation.reserved_turns()
                || matches!(
                    evidence,
                    ProviderTurnEvidence::Transport(csa_process::ProviderTurnCompletion::Unknown)
                )
                || matches!(evidence, ProviderTurnEvidence::ConfirmedExecutionFallback)
                    && *host_observed_turn_delta != 1
            {
                return Err(CompletionError::InvalidProviderAccounting);
            }
            if next
                .reconciled_execution_ids
                .iter()
                .any(|execution_id| execution_id == reservation.execution_id())
            {
                return Err(CompletionError::DuplicateExecutionReconciliation);
            }
            next.provider_turns = next
                .provider_turns
                .checked_add(*host_observed_turn_delta)
                .ok_or(CompletionError::BudgetExhausted)?;
            next.reconciled_execution_ids
                .push(reservation.execution_id().clone());
            if next.provider_turns >= next.budget.max_provider_turns {
                return Err(CompletionError::BudgetExhausted);
            }
        }
        (
            CompletionExecutionReservation::Provider(expected),
            ProviderTurnReconciliation::UsageIndeterminate {
                reservation: Some(actual),
            },
        ) if expected == actual => {
            next.phase = CompletionPhase::UsageIndeterminate;
        }
        _ => return Err(CompletionError::InvalidProviderAccounting),
    }
    Ok(next)
}

async fn execute_completion_action<P: CompletionPorts>(
    state: &CompletionState,
    action: &CompletionAction,
    ports: &mut P,
) -> Result<CompletionTransition, CompletionError> {
    let allowance = remaining_provider_turns(state)?;
    let reservation = ports
        .reserve_execution(action, allowance)
        .await
        .map_err(|error| CompletionError::Port(error.to_string()))?;
    validate_reservation(&reservation, allowance)?;
    let result = ports.execute(action, &reservation).await;
    let accounted = reconcile_provider_turns(state, &reservation, &result.reconciliation)?;
    if accounted.phase == CompletionPhase::UsageIndeterminate {
        return Err(CompletionError::UsageIndeterminate);
    }
    let event = result
        .event
        .map_err(|error| CompletionError::Port(error.to_string()))?;
    reduce_completion(&accounted, event)
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
    execute_completion_action(state, action, ports).await
}

pub(crate) async fn run_to_attestation<P: CompletionPorts>(
    ports: &mut P,
    budget: CompletionBudget,
    initial_epoch: EpochRecord,
    selection: CampaignSelection,
) -> Result<CompletionOutcome, CompletionError> {
    run_to_attestation_from_start(
        ports,
        budget,
        CompletionStart::Fresh {
            initial_epoch,
            selection,
        },
    )
    .await
}

/// Drive either a fresh campaign or a ledger-validated clustered campaign to attestation.
pub(crate) async fn run_to_attestation_from_start<P: CompletionPorts>(
    ports: &mut P,
    budget: CompletionBudget,
    start: CompletionStart,
) -> Result<CompletionOutcome, CompletionError> {
    let mut transition = start_completion(budget, start)?;
    loop {
        let action = transition
            .action
            .as_ref()
            .ok_or(CompletionError::InvalidTransition(
                "nonterminal action is missing",
            ))?;
        transition = execute_completion_action(&transition.state, action, ports).await?;
        if transition.state.phase == CompletionPhase::Attested {
            let gate_artifact = transition
                .state
                .gate_artifact
                .clone()
                .ok_or(CompletionError::IdentityMismatch)?;
            let clean_room = transition
                .state
                .clean_room
                .as_ref()
                .ok_or(CompletionError::IdentityMismatch)?;
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
                gate_artifact,
                review_artifact: clean_room.artifact().clone(),
                model_evidence: clean_room.model_evidence().clone(),
            });
        }
    }
}
