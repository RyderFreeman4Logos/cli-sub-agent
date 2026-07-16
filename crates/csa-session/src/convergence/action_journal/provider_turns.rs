//! Durable provider-turn reservations attached to fenced completion actions.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use ulid::Ulid;

use super::{
    CompletionActionClaim, CompletionActionJournal, CompletionActionJournalError,
    CompletionActionState,
};

/// Hard cap for provider-turn reservations attached to one completion action.
pub const MAX_PROVIDER_TURN_EXECUTIONS_PER_ACTION: usize = 1_000;

/// Globally unique identity for one reserved provider execution.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProviderTurnExecutionId(String);

impl ProviderTurnExecutionId {
    /// Generate a new canonical provider execution identifier.
    #[must_use]
    pub fn generate() -> Self {
        Self(Ulid::new().to_string())
    }

    /// Parse and canonicalize a provider execution identifier.
    ///
    /// # Errors
    ///
    /// Returns an error when `value` is not a ULID.
    pub fn parse(value: &str) -> anyhow::Result<Self> {
        let ulid = Ulid::from_string(value).map_err(|error| {
            anyhow::anyhow!("invalid provider execution id ULID '{value}': {error}")
        })?;
        Ok(Self(ulid.to_string()))
    }

    /// Return the canonical ULID text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProviderTurnExecutionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for ProviderTurnExecutionId {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl Serialize for ProviderTurnExecutionId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ProviderTurnExecutionId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(D::Error::custom)
    }
}

/// Persisted lifecycle of a reservation for one provider execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderTurnExecutionState {
    /// The reservation is durable, but the provider send has not been reconciled yet.
    Reserved,
    /// The provider was never sent, so its reservation was safely released.
    ReleasedBeforeSend,
    /// Host-observed provider turns were reconciled exactly once.
    Reconciled {
        /// Number of provider turns observed by the host for this execution.
        observed_turn_delta: u32,
    },
    /// Recovery cannot safely bound provider usage after the reservation was made.
    UsageIndeterminate,
}

/// Durable reservation for one provider execution.
///
/// A reservation is created under a fenced action claim before the provider can be sent. The
/// claim and execution ID make completion/recovery idempotent and unambiguous.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderTurnReservation {
    pub(super) claim: CompletionActionClaim,
    pub(super) execution_id: ProviderTurnExecutionId,
    pub(super) reserved_turns: u32,
}

impl ProviderTurnReservation {
    /// Return the fenced completion action that owns this reservation.
    #[must_use]
    pub fn claim(&self) -> &CompletionActionClaim {
        &self.claim
    }

    /// Return the stable identity for this provider execution.
    #[must_use]
    pub fn execution_id(&self) -> &ProviderTurnExecutionId {
        &self.execution_id
    }

    /// Return the durable upper bound reserved before sending the provider request.
    #[must_use]
    pub fn reserved_turns(&self) -> u32 {
        self.reserved_turns
    }
}

/// One provider execution reservation stored under a completion action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderTurnExecutionRecord {
    pub(super) execution_id: ProviderTurnExecutionId,
    pub(super) reserved_turns: u32,
    pub(super) state: ProviderTurnExecutionState,
}

impl ProviderTurnExecutionRecord {
    pub(super) fn reserved(execution_id: ProviderTurnExecutionId, reserved_turns: u32) -> Self {
        Self {
            execution_id,
            reserved_turns,
            state: ProviderTurnExecutionState::Reserved,
        }
    }

    /// Return the stable provider execution identity.
    #[must_use]
    pub fn execution_id(&self) -> &ProviderTurnExecutionId {
        &self.execution_id
    }

    /// Return the upper bound reserved before any provider send.
    #[must_use]
    pub fn reserved_turns(&self) -> u32 {
        self.reserved_turns
    }

    /// Return this reservation's durable reconciliation state.
    #[must_use]
    pub fn state(&self) -> ProviderTurnExecutionState {
        self.state
    }
}

impl CompletionActionJournal {
    /// Persist an upper-bounded provider-turn reservation before sending the provider request.
    ///
    /// # Errors
    ///
    /// Returns an error when the action claim is stale, the execution ID is reused, or the
    /// reservation is zero, unbounded, or cannot be durably associated with a started action.
    pub fn reserve_provider_turn(
        &mut self,
        claim: &CompletionActionClaim,
        execution_id: ProviderTurnExecutionId,
        reserved_turns: u32,
    ) -> Result<ProviderTurnReservation, CompletionActionJournalError> {
        self.require_current_claim(claim)?;
        if reserved_turns == 0 || reserved_turns > MAX_PROVIDER_TURN_EXECUTIONS_PER_ACTION as u32 {
            return Err(CompletionActionJournalError::InvalidProviderTurnReservation);
        }
        if self
            .actions
            .iter()
            .flat_map(|record| record.provider_turns.iter())
            .any(|record| record.execution_id == execution_id)
        {
            return Err(
                CompletionActionJournalError::DuplicateProviderTurnExecutionId(execution_id),
            );
        }
        let record = self.record_for_current_claim_mut(claim)?;
        if record.state != CompletionActionState::Started {
            return Err(CompletionActionJournalError::ProviderTurnActionNotStarted);
        }
        if record.provider_turns.len() >= MAX_PROVIDER_TURN_EXECUTIONS_PER_ACTION {
            return Err(
                CompletionActionJournalError::TooManyProviderTurnExecutions {
                    maximum: MAX_PROVIDER_TURN_EXECUTIONS_PER_ACTION,
                },
            );
        }
        record
            .provider_turns
            .push(ProviderTurnExecutionRecord::reserved(
                execution_id.clone(),
                reserved_turns,
            ));
        self.validate()?;
        Ok(ProviderTurnReservation {
            claim: claim.clone(),
            execution_id,
            reserved_turns,
        })
    }

    /// Reconcile one provider execution exactly once with a host-observed turn delta.
    ///
    /// Repeating the same reconciliation is idempotent; a conflicting second completion is
    /// rejected, so recovery can never double-count turns.
    pub fn reconcile_provider_turn(
        &mut self,
        reservation: &ProviderTurnReservation,
        observed_turn_delta: u32,
    ) -> Result<(), CompletionActionJournalError> {
        let execution = self.provider_execution_mut(reservation)?;
        if observed_turn_delta == 0 || observed_turn_delta > execution.reserved_turns {
            return Err(CompletionActionJournalError::InvalidProviderTurnReconciliation);
        }
        match execution.state {
            ProviderTurnExecutionState::Reserved => {
                execution.state = ProviderTurnExecutionState::Reconciled {
                    observed_turn_delta,
                };
            }
            ProviderTurnExecutionState::Reconciled {
                observed_turn_delta: existing,
            } if existing == observed_turn_delta => return Ok(()),
            state => {
                return Err(
                    CompletionActionJournalError::InvalidProviderTurnStateTransition {
                        from: state,
                        to: ProviderTurnExecutionState::Reconciled {
                            observed_turn_delta,
                        },
                    },
                );
            }
        }
        self.validate()
    }

    /// Release a reservation only when the provider send definitely did not start.
    pub fn release_provider_turn_before_send(
        &mut self,
        reservation: &ProviderTurnReservation,
    ) -> Result<(), CompletionActionJournalError> {
        let execution = self.provider_execution_mut(reservation)?;
        match execution.state {
            ProviderTurnExecutionState::Reserved => {
                execution.state = ProviderTurnExecutionState::ReleasedBeforeSend;
            }
            ProviderTurnExecutionState::ReleasedBeforeSend => return Ok(()),
            state => {
                return Err(
                    CompletionActionJournalError::InvalidProviderTurnStateTransition {
                        from: state,
                        to: ProviderTurnExecutionState::ReleasedBeforeSend,
                    },
                );
            }
        }
        self.validate()
    }

    /// Fail closed after a possible provider send whose usage cannot be safely bounded.
    pub fn mark_provider_turn_usage_indeterminate(
        &mut self,
        reservation: &ProviderTurnReservation,
    ) -> Result<(), CompletionActionJournalError> {
        let execution = self.provider_execution_mut(reservation)?;
        match execution.state {
            ProviderTurnExecutionState::Reserved => {
                execution.state = ProviderTurnExecutionState::UsageIndeterminate;
            }
            ProviderTurnExecutionState::UsageIndeterminate => return Ok(()),
            state => {
                return Err(
                    CompletionActionJournalError::InvalidProviderTurnStateTransition {
                        from: state,
                        to: ProviderTurnExecutionState::UsageIndeterminate,
                    },
                );
            }
        }
        self.validate()
    }

    fn provider_execution_mut(
        &mut self,
        reservation: &ProviderTurnReservation,
    ) -> Result<&mut ProviderTurnExecutionRecord, CompletionActionJournalError> {
        self.require_current_claim(&reservation.claim)?;
        let record = self.record_for_current_claim_mut(&reservation.claim)?;
        if record.state != CompletionActionState::Started {
            return Err(CompletionActionJournalError::ProviderTurnActionNotStarted);
        }
        let execution = record
            .provider_turns
            .iter_mut()
            .find(|execution| execution.execution_id == reservation.execution_id)
            .ok_or(CompletionActionJournalError::ProviderTurnReservationNotFound)?;
        if execution.reserved_turns != reservation.reserved_turns {
            return Err(CompletionActionJournalError::InvalidProviderTurnReservation);
        }
        Ok(execution)
    }
}
