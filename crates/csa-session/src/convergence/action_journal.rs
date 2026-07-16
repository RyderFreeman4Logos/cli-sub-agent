//! Durable, fenced journal for completion actions that can cause external effects.
//!
//! The convergence ledger records review evidence. This journal records the ownership and
//! outcome of every completion action before a caller performs an external effect. It is a v2
//! format from its first write; a v1 document can be inspected only as a read-only legacy
//! document and can never be resumed or overwritten.

use std::{fmt, str::FromStr};

use anyhow::Context as _;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use thiserror::Error;
use ulid::Ulid;

use super::{CampaignId, EpochId, Sha256Digest};

mod provider_turns;
mod validation;

pub use provider_turns::{
    MAX_PROVIDER_TURN_EXECUTIONS_PER_ACTION, ProviderTurnExecutionId, ProviderTurnExecutionRecord,
    ProviderTurnExecutionState, ProviderTurnReservation,
};

/// The only completion action journal schema this binary writes.
pub const COMPLETION_ACTION_JOURNAL_SCHEMA_VERSION: u32 = 2;
/// The former journal schema, which is accepted only for read-only inspection.
pub const LEGACY_COMPLETION_ACTION_JOURNAL_SCHEMA_VERSION: u32 = 1;
/// Hard cap for durable action records in one campaign/epoch journal.
pub const MAX_COMPLETION_ACTION_RECORDS: usize = 4_096;

/// Globally unique identifier for one externally observable completion action.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CompletionActionId(String);

impl CompletionActionId {
    /// Generate a new canonical action identifier.
    #[must_use]
    pub fn generate() -> Self {
        Self(Ulid::new().to_string())
    }

    /// Parse and canonicalize an action identifier.
    ///
    /// # Errors
    ///
    /// Returns an error when `value` is not a ULID.
    pub fn parse(value: &str) -> anyhow::Result<Self> {
        let ulid = Ulid::from_string(value).map_err(|error| {
            anyhow::anyhow!("invalid completion action id ULID '{value}': {error}")
        })?;
        Ok(Self(ulid.to_string()))
    }

    /// Return the canonical ULID text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CompletionActionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for CompletionActionId {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl Serialize for CompletionActionId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for CompletionActionId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(D::Error::custom)
    }
}

/// Fencing token returned by a successful durable action claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompletionActionClaim {
    campaign_id: CampaignId,
    epoch_id: EpochId,
    generation: u64,
    action_id: CompletionActionId,
    policy_digest: Sha256Digest,
}

impl CompletionActionClaim {
    fn new(
        campaign_id: CampaignId,
        epoch_id: EpochId,
        generation: u64,
        action_id: CompletionActionId,
        policy_digest: Sha256Digest,
    ) -> Self {
        Self {
            campaign_id,
            epoch_id,
            generation,
            action_id,
            policy_digest,
        }
    }

    /// Return the campaign authorized by this claim.
    #[must_use]
    pub fn campaign_id(&self) -> &CampaignId {
        &self.campaign_id
    }

    /// Return the exact epoch authorized by this claim.
    #[must_use]
    pub fn epoch_id(&self) -> &EpochId {
        &self.epoch_id
    }

    /// Return the monotonically increasing fencing generation.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Return the unique external action identifier.
    #[must_use]
    pub fn action_id(&self) -> &CompletionActionId {
        &self.action_id
    }

    /// Return the policy digest frozen when the action was claimed.
    #[must_use]
    pub fn policy_digest(&self) -> &Sha256Digest {
        &self.policy_digest
    }
}

/// Persisted lifecycle state for an external completion action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionActionState {
    /// The durable claim exists and external execution may have started.
    Started,
    /// The current claim holder durably reported completion.
    Finished,
    /// Recovery cannot prove that the started action did not take effect.
    Uncertain,
}

/// One durably recorded external completion action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompletionActionRecord {
    schema_version: u32,
    claim: CompletionActionClaim,
    state: CompletionActionState,
    started_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    #[serde(default)]
    provider_turns: Vec<ProviderTurnExecutionRecord>,
}

impl CompletionActionRecord {
    fn started(claim: CompletionActionClaim, now: DateTime<Utc>) -> Self {
        Self {
            schema_version: COMPLETION_ACTION_JOURNAL_SCHEMA_VERSION,
            claim,
            state: CompletionActionState::Started,
            started_at: now,
            updated_at: now,
            provider_turns: Vec::new(),
        }
    }

    /// Return the schema written for this action record.
    #[must_use]
    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Return the complete fenced action identity.
    #[must_use]
    pub fn claim(&self) -> &CompletionActionClaim {
        &self.claim
    }

    /// Return the durable lifecycle state.
    #[must_use]
    pub fn state(&self) -> CompletionActionState {
        self.state
    }

    /// Return when the action was first claimed.
    #[must_use]
    pub fn started_at(&self) -> &DateTime<Utc> {
        &self.started_at
    }

    /// Return when the lifecycle state was last changed.
    #[must_use]
    pub fn updated_at(&self) -> &DateTime<Utc> {
        &self.updated_at
    }

    /// Return the provider execution reservations owned by this action.
    #[must_use]
    pub fn provider_turns(&self) -> &[ProviderTurnExecutionRecord] {
        &self.provider_turns
    }
}

/// Versioned completion journal for one exact campaign/epoch/policy tuple.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompletionActionJournal {
    schema_version: u32,
    campaign_id: CampaignId,
    epoch_id: EpochId,
    generation: u64,
    policy_digest: Sha256Digest,
    actions: Vec<CompletionActionRecord>,
}

impl CompletionActionJournal {
    /// Create an empty v2 journal. New journals never emit the legacy v1 schema.
    #[must_use]
    pub fn new(campaign_id: CampaignId, epoch_id: EpochId, policy_digest: Sha256Digest) -> Self {
        Self {
            schema_version: COMPLETION_ACTION_JOURNAL_SCHEMA_VERSION,
            campaign_id,
            epoch_id,
            generation: 0,
            policy_digest,
            actions: Vec::new(),
        }
    }

    /// Return the journal schema version.
    #[must_use]
    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Return the campaign this journal fences.
    #[must_use]
    pub fn campaign_id(&self) -> &CampaignId {
        &self.campaign_id
    }

    /// Return the exact epoch this journal fences.
    #[must_use]
    pub fn epoch_id(&self) -> &EpochId {
        &self.epoch_id
    }

    /// Return the latest claimed generation; zero means no action has been claimed.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Return the policy digest frozen for every action in this journal.
    #[must_use]
    pub fn policy_digest(&self) -> &Sha256Digest {
        &self.policy_digest
    }

    /// Return the immutable ordered action records.
    #[must_use]
    pub fn actions(&self) -> &[CompletionActionRecord] {
        &self.actions
    }

    /// Whether it is safe to attest with respect to this journal.
    ///
    /// Any started or uncertain record blocks attestation. A missing journal is handled by the
    /// store separately so legacy non-completion workflows retain their existing behavior.
    #[must_use]
    pub fn permits_attestation(&self) -> bool {
        self.actions.iter().all(|record| {
            record.state == CompletionActionState::Finished
                && record.provider_turns.iter().all(|execution| {
                    matches!(
                        execution.state,
                        ProviderTurnExecutionState::ReleasedBeforeSend
                            | ProviderTurnExecutionState::Reconciled { .. }
                    )
                })
        })
    }

    /// Claim the next external action with an optimistic generation compare-and-swap.
    ///
    /// # Errors
    ///
    /// Returns an error when the caller's generation is stale, the action ID was used before,
    /// the policy or schema is invalid, or the bounded journal cannot accept another record.
    pub fn claim_next(
        &mut self,
        expected_generation: u64,
        action_id: CompletionActionId,
    ) -> Result<CompletionActionClaim, CompletionActionJournalError> {
        self.validate()?;
        if expected_generation != self.generation {
            return Err(CompletionActionJournalError::FencingMismatch {
                expected_generation,
                actual_generation: self.generation,
            });
        }
        if self.actions.len() >= MAX_COMPLETION_ACTION_RECORDS {
            return Err(CompletionActionJournalError::TooManyRecords {
                maximum: MAX_COMPLETION_ACTION_RECORDS,
            });
        }
        if self
            .actions
            .iter()
            .any(|record| record.claim.action_id == action_id)
        {
            return Err(CompletionActionJournalError::DuplicateActionId(action_id));
        }
        let generation = self
            .generation
            .checked_add(1)
            .ok_or(CompletionActionJournalError::GenerationOverflow)?;
        let claim = CompletionActionClaim::new(
            self.campaign_id.clone(),
            self.epoch_id.clone(),
            generation,
            action_id,
            self.policy_digest.clone(),
        );
        self.actions
            .push(CompletionActionRecord::started(claim.clone(), Utc::now()));
        self.generation = generation;
        self.validate()?;
        Ok(claim)
    }

    /// Mark a current started action uncertain and atomically issue a newer fenced claim.
    ///
    /// Recovery never converts uncertainty into success. The old holder's completion is rejected
    /// because its action is durably marked uncertain and the returned claim has a newer
    /// generation.
    pub fn recover_and_claim(
        &mut self,
        previous: &CompletionActionClaim,
        next_action_id: CompletionActionId,
    ) -> Result<CompletionActionClaim, CompletionActionJournalError> {
        self.require_current_claim(previous)?;
        let record = self.record_for_current_claim_mut(previous)?;
        if record.state != CompletionActionState::Started {
            return Err(CompletionActionJournalError::InvalidStateTransition {
                from: record.state,
                to: CompletionActionState::Uncertain,
            });
        }
        record.state = CompletionActionState::Uncertain;
        record.updated_at = Utc::now();
        self.claim_next(previous.generation, next_action_id)
    }

    /// Mark a currently held action uncertain without issuing replacement work.
    pub fn mark_uncertain(
        &mut self,
        claim: &CompletionActionClaim,
    ) -> Result<(), CompletionActionJournalError> {
        self.require_current_claim(claim)?;
        let record = self.record_for_current_claim_mut(claim)?;
        if record.state != CompletionActionState::Started {
            return Err(CompletionActionJournalError::InvalidStateTransition {
                from: record.state,
                to: CompletionActionState::Uncertain,
            });
        }
        record.state = CompletionActionState::Uncertain;
        record.updated_at = Utc::now();
        self.validate()
    }

    /// Persist completion only when `claim` still owns the newest fenced generation.
    pub fn finish(
        &mut self,
        claim: &CompletionActionClaim,
    ) -> Result<(), CompletionActionJournalError> {
        self.require_current_claim(claim)?;
        let record = self.record_for_current_claim_mut(claim)?;
        if record.state != CompletionActionState::Started {
            return Err(CompletionActionJournalError::InvalidStateTransition {
                from: record.state,
                to: CompletionActionState::Finished,
            });
        }
        if record.provider_turns.iter().any(|execution| {
            matches!(
                execution.state,
                ProviderTurnExecutionState::Reserved
                    | ProviderTurnExecutionState::UsageIndeterminate
            )
        }) {
            return Err(CompletionActionJournalError::ProviderTurnUnresolved);
        }
        record.state = CompletionActionState::Finished;
        record.updated_at = Utc::now();
        self.validate()
    }

    fn require_current_claim(
        &self,
        claim: &CompletionActionClaim,
    ) -> Result<(), CompletionActionJournalError> {
        self.validate()?;
        if claim.campaign_id != self.campaign_id
            || claim.epoch_id != self.epoch_id
            || claim.policy_digest != self.policy_digest
        {
            return Err(CompletionActionJournalError::IdentityMismatch);
        }
        if claim.generation != self.generation {
            return Err(CompletionActionJournalError::FencingMismatch {
                expected_generation: claim.generation,
                actual_generation: self.generation,
            });
        }
        Ok(())
    }

    fn record_for_current_claim_mut(
        &mut self,
        claim: &CompletionActionClaim,
    ) -> Result<&mut CompletionActionRecord, CompletionActionJournalError> {
        self.actions
            .iter_mut()
            .find(|record| record.claim == *claim)
            .ok_or(CompletionActionJournalError::ClaimNotFound)
    }
}

/// Read-only summary of a legacy v1 completion action journal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyCompletionActionJournal {
    record_count: usize,
}

impl LegacyCompletionActionJournal {
    /// Return the number of legacy records inspected without granting mutation authority.
    #[must_use]
    pub fn record_count(&self) -> usize {
        self.record_count
    }
}

/// Result of reading an on-disk action journal without mutating it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionActionJournalRead {
    /// No action journal exists for the project.
    Missing,
    /// A v1 document exists and is intentionally read-only.
    LegacyV1(LegacyCompletionActionJournal),
    /// A validated v2 journal exists.
    Current(CompletionActionJournal),
}

/// Parse only the legacy v1 view of a journal.
///
/// This gives older callers a deterministic refusal when they encounter a v2 or mixed document
/// instead of silently attempting an unsafe resume.
pub fn parse_legacy_completion_action_journal(
    bytes: &[u8],
) -> Result<LegacyCompletionActionJournal, CompletionActionJournalError> {
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|error| CompletionActionJournalError::InvalidDocument(error.to_string()))?;
    let version = schema_version(&value)
        .map_err(|error| CompletionActionJournalError::InvalidDocument(error.to_string()))?;
    if version != LEGACY_COMPLETION_ACTION_JOURNAL_SCHEMA_VERSION {
        return Err(CompletionActionJournalError::UnsupportedLegacyReaderSchema(
            version,
        ));
    }
    let actions = value
        .get("actions")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            CompletionActionJournalError::InvalidDocument("v1 actions is missing".into())
        })?;
    if actions.iter().any(|action| {
        action
            .get("schema_version")
            .and_then(serde_json::Value::as_u64)
            != Some(u64::from(LEGACY_COMPLETION_ACTION_JOURNAL_SCHEMA_VERSION))
    }) {
        return Err(CompletionActionJournalError::MixedSchema);
    }
    Ok(LegacyCompletionActionJournal {
        record_count: actions.len(),
    })
}

pub(crate) fn read_journal(bytes: &[u8]) -> anyhow::Result<CompletionActionJournalRead> {
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|error| anyhow::anyhow!("completion action journal is not JSON: {error}"))?;
    match schema_version(&value)? {
        LEGACY_COMPLETION_ACTION_JOURNAL_SCHEMA_VERSION => {
            let legacy =
                parse_legacy_completion_action_journal(bytes).map_err(anyhow::Error::from)?;
            Ok(CompletionActionJournalRead::LegacyV1(legacy))
        }
        COMPLETION_ACTION_JOURNAL_SCHEMA_VERSION => Ok(CompletionActionJournalRead::Current(
            CompletionActionJournal::parse_current(bytes)?,
        )),
        version => Err(anyhow::anyhow!(
            CompletionActionJournalError::UnsupportedSchema(version)
        )),
    }
}

fn schema_version(value: &serde_json::Value) -> anyhow::Result<u32> {
    let version = value
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        .context(
            "completion action journal schema_version is missing or not an unsigned integer",
        )?;
    u32::try_from(version).context("completion action journal schema_version exceeds u32")
}

/// Fail-closed errors for journal parsing, claims, recovery, and completion.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CompletionActionJournalError {
    /// The document schema is unknown to the current implementation.
    #[error(
        "unsupported completion action journal schema version {0}; expected {COMPLETION_ACTION_JOURNAL_SCHEMA_VERSION}"
    )]
    UnsupportedSchema(u32),
    /// A v1-only reader encountered a newer schema and must not resume it.
    #[error("unsupported completion action journal schema version {0} for legacy v1 reader")]
    UnsupportedLegacyReaderSchema(u32),
    /// A document mixes records from more than one schema version.
    #[error("mixed completion action journal schemas are unsupported")]
    MixedSchema,
    /// The document cannot be safely parsed.
    #[error("invalid completion action journal: {0}")]
    InvalidDocument(String),
    /// A record does not bind the journal's campaign, epoch, or policy digest.
    #[error("completion action journal identity or policy digest mismatch")]
    IdentityMismatch,
    /// An action ID is reused, which would make recovery ambiguous.
    #[error("duplicate completion action id {0}")]
    DuplicateActionId(CompletionActionId),
    /// The journal collection crossed its bounded protocol limit.
    #[error("completion action journal exceeds its maximum of {maximum} records")]
    TooManyRecords {
        /// Configured hard upper bound.
        maximum: usize,
    },
    /// The next generation cannot be represented.
    #[error("completion action journal generation overflow")]
    GenerationOverflow,
    /// Entries are not a complete contiguous sequence from one through the current generation.
    #[error("completion action journal generation sequence is invalid")]
    InvalidGenerationSequence,
    /// The caller supplied a stale generation and therefore no longer owns the action.
    #[error(
        "completion action journal fencing mismatch: expected generation {expected_generation}, current generation is {actual_generation}"
    )]
    FencingMismatch {
        /// The generation supplied by the caller.
        expected_generation: u64,
        /// The generation durably held by the journal.
        actual_generation: u64,
    },
    /// A claim no longer identifies a persisted action record.
    #[error("completion action journal claim is not present")]
    ClaimNotFound,
    /// A state transition would make recovery or attestation ambiguous.
    #[error("completion action journal cannot transition from {from:?} to {to:?}")]
    InvalidStateTransition {
        /// Persisted state.
        from: CompletionActionState,
        /// Requested state.
        to: CompletionActionState,
    },
    /// A provider execution reservation was zero, unbounded, or did not match its journal row.
    #[error("provider turn reservation is invalid")]
    InvalidProviderTurnReservation,
    /// A provider execution ID was reused within the completion journal.
    #[error("duplicate provider turn execution id {0}")]
    DuplicateProviderTurnExecutionId(ProviderTurnExecutionId),
    /// A completion action reached the bounded provider-execution protocol limit.
    #[error("completion action exceeds its maximum of {maximum} provider turn executions")]
    TooManyProviderTurnExecutions {
        /// Configured hard upper bound.
        maximum: usize,
    },
    /// A provider turn was reconciled with zero or more turns than were reserved.
    #[error("provider turn reconciliation is not safely bounded by its reservation")]
    InvalidProviderTurnReconciliation,
    /// The reservation cannot be found under its claimed completion action.
    #[error("provider turn reservation is not present")]
    ProviderTurnReservationNotFound,
    /// Provider turn mutation requires the owning completion action to remain started.
    #[error("provider turn mutation requires a started completion action")]
    ProviderTurnActionNotStarted,
    /// A provider execution moved through an unsafe or conflicting recovery state.
    #[error("provider turn cannot transition from {from:?} to {to:?}")]
    InvalidProviderTurnStateTransition {
        /// Persisted provider execution state.
        from: ProviderTurnExecutionState,
        /// Requested provider execution state.
        to: ProviderTurnExecutionState,
    },
    /// A completion action was finished while a provider reservation remained unresolved.
    #[error("completion action contains unresolved provider turns")]
    ProviderTurnUnresolved,
    /// A v1 document is deliberately not eligible for mutation.
    #[error("legacy completion action journal schema version 1 is read-only")]
    LegacyReadOnly,
    /// A terminal attestation was requested while an action remains unresolved.
    #[error("completion action journal contains started or uncertain actions")]
    IncompleteForAttestation,
}
