//! Validation and parsing for the durable completion action journal.

use std::collections::HashSet;

use super::{
    COMPLETION_ACTION_JOURNAL_SCHEMA_VERSION, CompletionActionJournal,
    CompletionActionJournalError, MAX_COMPLETION_ACTION_RECORDS,
    MAX_PROVIDER_TURN_EXECUTIONS_PER_ACTION, ProviderTurnExecutionState, schema_version,
};

impl CompletionActionJournal {
    /// Validate schema, identity, generation, policy, duplicate-ID, and collection bounds.
    pub fn validate(&self) -> Result<(), CompletionActionJournalError> {
        if self.schema_version != COMPLETION_ACTION_JOURNAL_SCHEMA_VERSION {
            return Err(CompletionActionJournalError::UnsupportedSchema(
                self.schema_version,
            ));
        }
        if self.actions.len() > MAX_COMPLETION_ACTION_RECORDS {
            return Err(CompletionActionJournalError::TooManyRecords {
                maximum: MAX_COMPLETION_ACTION_RECORDS,
            });
        }
        if u64::try_from(self.actions.len()).ok() != Some(self.generation) {
            return Err(CompletionActionJournalError::InvalidGenerationSequence);
        }
        let mut action_ids = HashSet::new();
        let mut execution_ids = HashSet::new();
        for (index, record) in self.actions.iter().enumerate() {
            if record.schema_version != COMPLETION_ACTION_JOURNAL_SCHEMA_VERSION {
                return Err(CompletionActionJournalError::MixedSchema);
            }
            let expected_generation = u64::try_from(index)
                .ok()
                .and_then(|value| value.checked_add(1))
                .ok_or(CompletionActionJournalError::GenerationOverflow)?;
            if record.claim.generation != expected_generation {
                return Err(CompletionActionJournalError::InvalidGenerationSequence);
            }
            if record.claim.campaign_id != self.campaign_id
                || record.claim.epoch_id != self.epoch_id
                || record.claim.policy_digest != self.policy_digest
            {
                return Err(CompletionActionJournalError::IdentityMismatch);
            }
            if !action_ids.insert(record.claim.action_id.clone()) {
                return Err(CompletionActionJournalError::DuplicateActionId(
                    record.claim.action_id.clone(),
                ));
            }
            if record.provider_turns.len() > MAX_PROVIDER_TURN_EXECUTIONS_PER_ACTION {
                return Err(
                    CompletionActionJournalError::TooManyProviderTurnExecutions {
                        maximum: MAX_PROVIDER_TURN_EXECUTIONS_PER_ACTION,
                    },
                );
            }
            for execution in &record.provider_turns {
                if execution.reserved_turns == 0
                    || execution.reserved_turns > MAX_PROVIDER_TURN_EXECUTIONS_PER_ACTION as u32
                {
                    return Err(CompletionActionJournalError::InvalidProviderTurnReservation);
                }
                if !execution_ids.insert(execution.execution_id.clone()) {
                    return Err(
                        CompletionActionJournalError::DuplicateProviderTurnExecutionId(
                            execution.execution_id.clone(),
                        ),
                    );
                }
                if let ProviderTurnExecutionState::Reconciled {
                    observed_turn_delta,
                } = execution.state
                    && (observed_turn_delta == 0 || observed_turn_delta > execution.reserved_turns)
                {
                    return Err(CompletionActionJournalError::InvalidProviderTurnReconciliation);
                }
            }
        }
        Ok(())
    }

    pub(crate) fn parse_current(bytes: &[u8]) -> anyhow::Result<Self> {
        let value: serde_json::Value = serde_json::from_slice(bytes)
            .map_err(|error| anyhow::anyhow!("completion action journal is not JSON: {error}"))?;
        let schema_version = schema_version(&value)?;
        if schema_version != COMPLETION_ACTION_JOURNAL_SCHEMA_VERSION {
            return Err(anyhow::anyhow!(
                CompletionActionJournalError::UnsupportedSchema(schema_version)
            ));
        }
        let journal: Self = serde_json::from_value(value)
            .map_err(|error| anyhow::anyhow!("completion action journal v2 is invalid: {error}"))?;
        journal.validate().map_err(anyhow::Error::from)?;
        Ok(journal)
    }
}
