use std::{fmt, str::FromStr};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use ulid::Ulid;

use super::{
    CampaignId, CampaignRecord, CandidateDispositionRecord, CandidateRecord, CoverageCellRecord,
    CoverageDispositionRecord, CoveragePlanFinalizationRecord, DiscoveryAttemptFinalizationRecord,
    DiscoveryAttemptRecord, EpochRecord, RepairBatchRecord, RepairHandoffRecord, RootClusterRecord,
    validation,
};

/// Current on-disk schema version for convergence ledgers.
pub const CONVERGENCE_LEDGER_SCHEMA_VERSION: u32 = 1;

/// Validated, canonically encoded ULID identifying one ledger event.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LedgerEventId(String);

impl LedgerEventId {
    /// Generate a new ledger event identifier.
    #[must_use]
    pub fn generate() -> Self {
        Self(Ulid::new().to_string())
    }

    /// Parse and canonicalize a ledger event ULID.
    ///
    /// # Errors
    ///
    /// Returns an error when `value` is not a valid ULID.
    pub fn parse(value: &str) -> Result<Self> {
        let ulid = Ulid::from_string(value)
            .map_err(|error| anyhow::anyhow!("invalid ledger event id ULID '{value}': {error}"))?;
        Ok(Self(ulid.to_string()))
    }

    /// Return the canonical 26-character ULID.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for LedgerEventId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for LedgerEventId {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        Self::parse(value)
    }
}

impl Serialize for LedgerEventId {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for LedgerEventId {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(D::Error::custom)
    }
}

/// Immutable evidence carried by a convergence ledger entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "record",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ConvergenceEvent {
    /// A campaign was initialized with a frozen metadata snapshot.
    CampaignStarted(CampaignRecord),
    /// A deterministic discovery epoch was opened for the campaign.
    EpochOpened(EpochRecord),
    /// A deterministic semantic coverage cell was defined in an opened epoch.
    CoverageCellDefined(CoverageCellRecord),
    /// One defined coverage cell received its planning disposition.
    CoverageDispositionRecorded(CoverageDispositionRecord),
    /// The complete coverage plan for an epoch was sealed.
    CoveragePlanFinalized(CoveragePlanFinalizationRecord),
    /// One discovery attempt produced completion and artifact evidence.
    DiscoveryAttemptRecorded(DiscoveryAttemptRecord),
    /// One candidate observation was reported by a prior discovery attempt.
    CandidateRecorded(CandidateRecord),
    /// The complete candidate evidence for a discovery attempt was sealed.
    DiscoveryAttemptFinalized(DiscoveryAttemptFinalizationRecord),
    /// One candidate received its immutable terminal disposition.
    CandidateDispositionRecorded(CandidateDispositionRecord),
    /// Verified blocking candidates were clustered by a common root cause.
    RootClusterRecorded(RootClusterRecord),
    /// One root cluster received exactly one consolidated repair batch.
    RepairBatchRecorded(RepairBatchRecord),
    /// One validated repair batch received an immutable writer handoff.
    RepairHandoffRecorded(RepairHandoffRecord),
}

/// One ordered, immutable convergence history event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConvergenceLedgerEntry {
    sequence: u64,
    event_id: LedgerEventId,
    campaign_id: CampaignId,
    recorded_at: DateTime<Utc>,
    event: ConvergenceEvent,
}

impl ConvergenceLedgerEntry {
    pub(crate) fn new(
        sequence: u64,
        event_id: LedgerEventId,
        campaign_id: CampaignId,
        recorded_at: DateTime<Utc>,
        event: ConvergenceEvent,
    ) -> Self {
        Self {
            sequence,
            event_id,
            campaign_id,
            recorded_at,
            event,
        }
    }

    /// Return the one-based history sequence.
    #[must_use]
    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    /// Return the globally unique event identifier.
    #[must_use]
    pub fn event_id(&self) -> &LedgerEventId {
        &self.event_id
    }

    /// Return the campaign receiving this event.
    #[must_use]
    pub fn campaign_id(&self) -> &CampaignId {
        &self.campaign_id
    }

    /// Return when this event was recorded.
    #[must_use]
    pub fn recorded_at(&self) -> &DateTime<Utc> {
        &self.recorded_at
    }

    /// Return the immutable event evidence.
    #[must_use]
    pub fn event(&self) -> &ConvergenceEvent {
        &self.event
    }
}

/// Strict append-history ledger for convergence campaigns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConvergenceLedger {
    schema_version: u32,
    entries: Vec<ConvergenceLedgerEntry>,
}

impl Default for ConvergenceLedger {
    fn default() -> Self {
        Self::empty()
    }
}

impl ConvergenceLedger {
    /// Construct an empty ledger using the current schema version.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            schema_version: CONVERGENCE_LEDGER_SCHEMA_VERSION,
            entries: Vec::new(),
        }
    }

    /// Return the serialized ledger schema version.
    #[must_use]
    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Return the ordered history entries.
    #[must_use]
    pub fn entries(&self) -> &[ConvergenceLedgerEntry] {
        &self.entries
    }

    /// Append one event using a generated event ID, current timestamp, and next sequence.
    ///
    /// The append is transactional: a protocol-invalid event is removed before the error
    /// is returned, leaving the ledger exactly as it was before this call.
    ///
    /// # Errors
    ///
    /// Returns an error when the next sequence cannot be represented or when replaying the
    /// tentative history violates any ledger protocol invariant.
    pub fn append(&mut self, campaign_id: CampaignId, event: ConvergenceEvent) -> Result<()> {
        self.append_batch(campaign_id, vec![event])
    }

    /// Append a complete event batch only when the resulting history is valid.
    ///
    /// The batch is constructed and validated on a clone, so callers never observe a partial
    /// suffix when any event in the batch violates the convergence protocol.
    ///
    /// # Errors
    ///
    /// Returns an error when a sequence cannot be represented or the complete tentative history
    /// violates a ledger protocol invariant.
    pub fn append_batch(
        &mut self,
        campaign_id: CampaignId,
        events: Vec<ConvergenceEvent>,
    ) -> Result<()> {
        let mut next = self.clone();
        for event in events {
            next.append_unvalidated(campaign_id.clone(), event)?;
        }
        next.validate()?;
        *self = next;
        Ok(())
    }

    fn append_unvalidated(
        &mut self,
        campaign_id: CampaignId,
        event: ConvergenceEvent,
    ) -> Result<()> {
        let sequence = u64::try_from(self.entries.len())
            .context("convergence ledger contains more entries than u64 can address")?
            .checked_add(1)
            .context("convergence ledger sequence overflow")?;
        self.entries.push(ConvergenceLedgerEntry::new(
            sequence,
            LedgerEventId::generate(),
            campaign_id,
            Utc::now(),
            event,
        ));
        Ok(())
    }

    /// Validate schema compatibility and all history ordering and identity invariants.
    ///
    /// # Errors
    ///
    /// Returns an error for unsupported schemas, discontinuous or duplicated history,
    /// invalid event ordering, provenance mismatches, or incomplete finalization boundaries.
    pub fn validate(&self) -> Result<()> {
        validation::validate_ledger(self.schema_version, &self.entries)
    }
}
