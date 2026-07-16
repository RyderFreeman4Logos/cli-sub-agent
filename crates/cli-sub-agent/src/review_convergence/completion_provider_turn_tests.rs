//! Provider-turn accounting contracts for the completion reducer.

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;

use csa_session::convergence::CampaignId;

use super::completion::{
    CompletionAction as Action, CompletionBudget as Budget, CompletionError as Failure,
    CompletionEvent as Event, CompletionExecutionReservation as ExecutionReservation,
    CompletionPhase as Phase, CompletionPortError as PortFailure,
    CompletionPortResult as PortResult, CompletionPorts, CompletionState as State,
    ProviderTurnAllowance, ProviderTurnEvidence, ProviderTurnReconciliation,
    reconcile_provider_turns, reduce_completion, run_targeted_discovery, run_to_attestation,
};
use super::completion_tests::{
    artifact, clean_output, epoch, epoch_id, provider_reservation, reach_targeted_discovery,
};
use super::discovery_contract::{CampaignSelection, DiscoveryFocus};

pub(super) struct FakeStep {
    pub(super) reservation: ExecutionReservation,
    pub(super) result: PortResult,
}

pub(super) struct FakePorts {
    steps: VecDeque<FakeStep>,
    actions: Vec<Action>,
}

impl FakePorts {
    pub(super) fn new(events: VecDeque<Result<Event, PortFailure>>) -> Self {
        Self {
            steps: events
                .into_iter()
                .map(|event| FakeStep {
                    reservation: ExecutionReservation::HostOnly,
                    result: PortResult {
                        event,
                        reconciliation: ProviderTurnReconciliation::HostOnly,
                    },
                })
                .collect(),
            actions: Vec::new(),
        }
    }

    pub(super) fn with_steps(steps: VecDeque<FakeStep>) -> Self {
        Self {
            steps,
            actions: Vec::new(),
        }
    }

    pub(super) fn actions(&self) -> &[Action] {
        &self.actions
    }
}

impl CompletionPorts for FakePorts {
    fn reserve_execution<'a>(
        &'a mut self,
        _action: &'a Action,
        _allowance: ProviderTurnAllowance,
    ) -> Pin<Box<dyn Future<Output = Result<ExecutionReservation, PortFailure>> + 'a>> {
        let reservation = self.steps.front().expect("fake step").reservation.clone();
        Box::pin(async move { Ok(reservation) })
    }

    fn execute<'a>(
        &'a mut self,
        action: &'a Action,
        _reservation: &'a ExecutionReservation,
    ) -> Pin<Box<dyn Future<Output = PortResult> + 'a>> {
        self.actions.push(action.clone());
        let result = self.steps.pop_front().expect("fake step").result;
        Box::pin(async move { result })
    }
}

#[test]
fn host_only_actions_count_cycles_but_zero_provider_turns() {
    let state = State::new(Budget::new(8, 4).unwrap());
    let started = reduce_completion(
        &state,
        Event::Started {
            epoch: epoch(2),
            selection: CampaignSelection::Fresh,
        },
    )
    .expect("start");
    let campaign = CampaignId::generate();
    let gates = reduce_completion(
        &started.state,
        Event::DiscoveryCompleted {
            focus: DiscoveryFocus::Broad,
            selection: CampaignSelection::Fresh,
            campaign_id: campaign.clone(),
            epoch: epoch(2),
            candidates: Vec::new(),
        },
    )
    .expect("final gates action");
    let accounted = reconcile_provider_turns(
        &gates.state,
        &ExecutionReservation::HostOnly,
        &ProviderTurnReconciliation::HostOnly,
    )
    .expect("host-only reconciliation");
    let transition = reduce_completion(
        &accounted,
        Event::FinalGatesPassed {
            campaign_id: campaign,
            epoch_id: epoch_id(2),
            artifact: artifact(b"host-only-gates"),
        },
    )
    .expect("host-only final gates event");

    assert_eq!(transition.state.cycles, 3);
    assert_eq!(transition.state.provider_turns, 0);
}

#[test]
fn provider_turn_reconciliation_is_exactly_once_and_does_not_use_action_kind() {
    let state = State::new(Budget::new(8, 4).unwrap());
    let provider_reservation = provider_reservation(2);
    let reservation = ExecutionReservation::Provider(provider_reservation.clone());
    let reconciliation = ProviderTurnReconciliation::Reconciled {
        reservation: provider_reservation,
        host_observed_turn_delta: 2,
        evidence: ProviderTurnEvidence::Transport(csa_process::ProviderTurnCompletion::Natural),
    };
    let accounted = reconcile_provider_turns(&state, &reservation, &reconciliation)
        .expect("first reconciliation");

    assert_eq!(accounted.provider_turns, 2);
    assert_eq!(accounted.reconciled_execution_ids.len(), 1);
    assert_eq!(
        reconcile_provider_turns(&accounted, &reservation, &reconciliation),
        Err(Failure::DuplicateExecutionReconciliation)
    );
}

#[test]
fn retry_continuation_and_incomplete_provider_turns_use_the_reported_delta() {
    let state = State::new(Budget::new(8, 6).unwrap());
    let provider_reservation = provider_reservation(3);
    let accounted = reconcile_provider_turns(
        &state,
        &ExecutionReservation::Provider(provider_reservation.clone()),
        &ProviderTurnReconciliation::Reconciled {
            reservation: provider_reservation,
            host_observed_turn_delta: 3,
            evidence: ProviderTurnEvidence::Transport(
                csa_process::ProviderTurnCompletion::Incomplete,
            ),
        },
    )
    .expect("retry, continuation, and incomplete turns are bounded by the reservation");

    assert_eq!(accounted.provider_turns, 3);
}

#[test]
fn indeterminate_provider_usage_stops_the_reducer_before_attestation() {
    let state = State::new(Budget::new(8, 4).unwrap());
    let provider_reservation = provider_reservation(1);
    let accounted = reconcile_provider_turns(
        &state,
        &ExecutionReservation::Provider(provider_reservation.clone()),
        &ProviderTurnReconciliation::UsageIndeterminate {
            reservation: Some(provider_reservation),
        },
    )
    .expect("indeterminate state");

    assert_eq!(accounted.phase(), Phase::UsageIndeterminate);
    assert_eq!(
        reduce_completion(
            &accounted,
            Event::Started {
                epoch: epoch(2),
                selection: CampaignSelection::Fresh,
            },
        ),
        Err(Failure::UsageIndeterminate)
    );
}

#[test]
fn execution_metadata_adapter_records_transport_or_bounded_fallback_delta() {
    let natural_reservation = provider_reservation(1);
    let natural = csa_process::ExecutionResult {
        terminal_reason: Some("end_turn".to_owned()),
        ..Default::default()
    };
    assert!(matches!(
        ProviderTurnReconciliation::from_execution_result(natural_reservation, &natural),
        ProviderTurnReconciliation::Reconciled {
            host_observed_turn_delta: 1,
            evidence: ProviderTurnEvidence::Transport(csa_process::ProviderTurnCompletion::Natural),
            ..
        }
    ));

    let fallback = ProviderTurnReconciliation::from_execution_result(
        provider_reservation(1),
        &csa_process::ExecutionResult::default(),
    );
    assert!(matches!(
        fallback,
        ProviderTurnReconciliation::Reconciled {
            host_observed_turn_delta: 1,
            evidence: ProviderTurnEvidence::ConfirmedExecutionFallback,
            ..
        }
    ));
}

#[tokio::test]
async fn provider_turn_delta_precisely_prevents_a_later_attestation() {
    let campaign = CampaignId::generate();
    let gate_artifact = artifact(b"gates");
    let review = clean_output();
    let discovery_reservation = provider_reservation(1);
    let clean_room_reservation = provider_reservation(2);
    let mut ports = FakePorts::with_steps(VecDeque::from([
        FakeStep {
            reservation: ExecutionReservation::Provider(discovery_reservation.clone()),
            result: PortResult {
                event: Ok(Event::DiscoveryCompleted {
                    focus: DiscoveryFocus::Broad,
                    selection: CampaignSelection::Fresh,
                    campaign_id: campaign.clone(),
                    epoch: epoch(2),
                    candidates: Vec::new(),
                }),
                reconciliation: ProviderTurnReconciliation::Reconciled {
                    reservation: discovery_reservation,
                    host_observed_turn_delta: 1,
                    evidence: ProviderTurnEvidence::Transport(
                        csa_process::ProviderTurnCompletion::Natural,
                    ),
                },
            },
        },
        FakeStep {
            reservation: ExecutionReservation::HostOnly,
            result: PortResult {
                event: Ok(Event::FinalGatesPassed {
                    campaign_id: campaign.clone(),
                    epoch_id: epoch_id(2),
                    artifact: gate_artifact,
                }),
                reconciliation: ProviderTurnReconciliation::HostOnly,
            },
        },
        FakeStep {
            reservation: ExecutionReservation::Provider(clean_room_reservation.clone()),
            result: PortResult {
                event: Ok(Event::CleanRoomCompleted {
                    campaign_id: campaign,
                    epoch_id: epoch_id(2),
                    output: review,
                }),
                reconciliation: ProviderTurnReconciliation::Reconciled {
                    reservation: clean_room_reservation,
                    host_observed_turn_delta: 2,
                    evidence: ProviderTurnEvidence::Transport(
                        csa_process::ProviderTurnCompletion::Incomplete,
                    ),
                },
            },
        },
    ]));

    let error = run_to_attestation(
        &mut ports,
        Budget::new(8, 3).unwrap(),
        epoch(2),
        CampaignSelection::Fresh,
    )
    .await
    .expect_err("exhausted provider turn budget must stop before publication");

    assert_eq!(error, Failure::BudgetExhausted);
    assert_eq!(ports.actions().len(), 3);
    assert!(
        ports
            .actions()
            .iter()
            .all(|action| !matches!(action, Action::PublishFinalPair { .. }))
    );
}

#[tokio::test]
async fn provider_failure_reports_its_reconciled_turns_before_returning_the_error() {
    let reservation = provider_reservation(2);
    let mut ports = FakePorts::with_steps(VecDeque::from([FakeStep {
        reservation: ExecutionReservation::Provider(reservation.clone()),
        result: PortResult {
            event: Err(PortFailure::new("provider timed out after retry")),
            reconciliation: ProviderTurnReconciliation::Reconciled {
                reservation,
                host_observed_turn_delta: 2,
                evidence: ProviderTurnEvidence::Transport(
                    csa_process::ProviderTurnCompletion::Incomplete,
                ),
            },
        },
    }]));

    let error = run_to_attestation(
        &mut ports,
        Budget::new(8, 4).unwrap(),
        epoch(2),
        CampaignSelection::Fresh,
    )
    .await
    .expect_err("provider failure must not advance the reducer");

    assert!(matches!(error, Failure::Port(message) if message.contains("timed out")));
    assert_eq!(ports.actions().len(), 1);
}

#[tokio::test]
async fn exhausted_budget_refuses_an_oversized_reservation_before_provider_execution() {
    let reservation = provider_reservation(2);
    let mut ports = FakePorts::with_steps(VecDeque::from([FakeStep {
        reservation: ExecutionReservation::Provider(reservation),
        result: PortResult {
            event: Err(PortFailure::new("must not execute")),
            reconciliation: ProviderTurnReconciliation::HostOnly,
        },
    }]));

    let error = run_to_attestation(
        &mut ports,
        Budget::new(8, 1).unwrap(),
        epoch(2),
        CampaignSelection::Fresh,
    )
    .await
    .expect_err("provider reservation cannot exceed the remaining budget");

    assert_eq!(error, Failure::BudgetExhausted);
    assert!(ports.actions().is_empty());
}

#[tokio::test]
async fn port_error_never_advances_reducer_state() {
    let (state, action, _) = reach_targeted_discovery();
    let before = state.clone();
    let mut ports = FakePorts {
        steps: VecDeque::from([FakeStep {
            reservation: ExecutionReservation::HostOnly,
            result: PortResult {
                event: Err(PortFailure::new("provider unavailable")),
                reconciliation: ProviderTurnReconciliation::HostOnly,
            },
        }]),
        actions: Vec::new(),
    };
    assert!(
        run_targeted_discovery(&state, &action, &mut ports)
            .await
            .is_err()
    );
    assert_eq!(before, state);
}
