use std::{
    collections::{HashMap, HashSet},
    fmt,
    str::FromStr,
};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use ulid::Ulid;

use super::{
    CampaignId, CampaignRecord, CandidateDisposition, CandidateDispositionRecord, CandidateId,
    CandidateRecord, CoverageCellId, CoverageCellRecord, CoverageDispositionRecord,
    DiscoveryAttemptId, DiscoveryAttemptRecord, EpochId, EpochRecord, StableFindingId,
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
    /// One discovery attempt produced completion and artifact evidence.
    DiscoveryAttemptRecorded(DiscoveryAttemptRecord),
    /// One candidate observation was reported by a prior discovery attempt.
    CandidateRecorded(CandidateRecord),
    /// One candidate received its immutable terminal disposition.
    CandidateDispositionRecorded(CandidateDispositionRecord),
    /// One defined coverage cell received its planning disposition.
    CoverageDispositionRecorded(CoverageDispositionRecord),
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
    #[cfg(test)]
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

    /// Validate schema compatibility and all history ordering and identity invariants.
    ///
    /// # Errors
    ///
    /// Returns an error for unsupported schemas, discontinuous or duplicated history,
    /// invalid campaign ordering, unknown epochs, duplicate campaign-scoped identities,
    /// or tampered deterministic records.
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != CONVERGENCE_LEDGER_SCHEMA_VERSION {
            bail!(
                "unsupported convergence ledger schema version {}; expected {}",
                self.schema_version,
                CONVERGENCE_LEDGER_SCHEMA_VERSION
            );
        }

        let mut event_ids = std::collections::HashSet::new();
        let mut campaigns: HashMap<CampaignId, CampaignState> = HashMap::new();

        for (index, entry) in self.entries.iter().enumerate() {
            let expected_sequence = u64::try_from(index)
                .context("convergence ledger contains more entries than u64 can address")?
                + 1;
            if entry.sequence != expected_sequence {
                bail!(
                    "noncontiguous convergence ledger sequence: expected {expected_sequence}, got {}",
                    entry.sequence
                );
            }
            if !event_ids.insert(entry.event_id.clone()) {
                bail!("duplicate convergence ledger event id {}", entry.event_id);
            }

            match &entry.event {
                ConvergenceEvent::CampaignStarted(record) => {
                    if record.id() != &entry.campaign_id {
                        bail!(
                            "campaign start record id {} does not match entry campaign id {}",
                            record.id(),
                            entry.campaign_id
                        );
                    }
                    if campaigns
                        .insert(entry.campaign_id.clone(), CampaignState::default())
                        .is_some()
                    {
                        bail!("duplicate campaign start for {}", entry.campaign_id);
                    }
                }
                ConvergenceEvent::EpochOpened(record) => {
                    let state = campaigns.get_mut(&entry.campaign_id).with_context(|| {
                        format!(
                            "epoch {} recorded before campaign {} started",
                            record.id(),
                            entry.campaign_id
                        )
                    })?;
                    record.validate().with_context(|| {
                        format!("invalid epoch record for campaign {}", entry.campaign_id)
                    })?;
                    if !state.epochs.insert(record.id().clone()) {
                        bail!(
                            "duplicate epoch {} in campaign {}",
                            record.id(),
                            entry.campaign_id
                        );
                    }
                }
                ConvergenceEvent::CoverageCellDefined(record) => {
                    let state = campaigns.get_mut(&entry.campaign_id).with_context(|| {
                        format!(
                            "coverage cell {} recorded before campaign {} started",
                            record.id(),
                            entry.campaign_id
                        )
                    })?;
                    record.validate().with_context(|| {
                        format!(
                            "invalid coverage cell record for campaign {}",
                            entry.campaign_id
                        )
                    })?;
                    if !state.epochs.contains(record.epoch_id()) {
                        bail!(
                            "coverage cell {} references unopened epoch {} in campaign {}",
                            record.id(),
                            record.epoch_id(),
                            entry.campaign_id
                        );
                    }
                    if state
                        .cells
                        .insert(record.id().clone(), record.epoch_id().clone())
                        .is_some()
                    {
                        bail!(
                            "duplicate coverage cell {} in campaign {}",
                            record.id(),
                            entry.campaign_id
                        );
                    }
                }
                ConvergenceEvent::DiscoveryAttemptRecorded(record) => {
                    let state = campaigns.get_mut(&entry.campaign_id).with_context(|| {
                        format!(
                            "discovery attempt {} recorded before campaign {} started",
                            record.id(),
                            entry.campaign_id
                        )
                    })?;
                    record.validate().with_context(|| {
                        format!(
                            "invalid discovery attempt {} in campaign {}",
                            record.id(),
                            entry.campaign_id
                        )
                    })?;
                    if !state.epochs.contains(record.epoch_id()) {
                        bail!(
                            "discovery attempt {} references unopened epoch {} in campaign {}",
                            record.id(),
                            record.epoch_id(),
                            entry.campaign_id
                        );
                    }
                    let cell_epoch = state.cells.get(record.coverage_cell_id()).with_context(|| {
                        format!(
                            "discovery attempt {} references undefined coverage cell {} in campaign {}",
                            record.id(),
                            record.coverage_cell_id(),
                            entry.campaign_id
                        )
                    })?;
                    if cell_epoch != record.epoch_id() {
                        bail!(
                            "discovery attempt {} epoch {} does not match coverage cell {} epoch {} in campaign {}",
                            record.id(),
                            record.epoch_id(),
                            record.coverage_cell_id(),
                            cell_epoch,
                            entry.campaign_id
                        );
                    }
                    if !state.attempts.insert(record.id().clone()) {
                        bail!(
                            "duplicate discovery attempt {} in campaign {}",
                            record.id(),
                            entry.campaign_id
                        );
                    }
                }
                ConvergenceEvent::CandidateRecorded(record) => {
                    let state = campaigns.get_mut(&entry.campaign_id).with_context(|| {
                        format!(
                            "candidate {} recorded before campaign {} started",
                            record.id(),
                            entry.campaign_id
                        )
                    })?;
                    record.validate().with_context(|| {
                        format!(
                            "invalid candidate {} in campaign {}",
                            record.id(),
                            entry.campaign_id
                        )
                    })?;
                    if !state.attempts.contains(record.discovery_attempt_id()) {
                        bail!(
                            "candidate {} references unknown discovery attempt {} in campaign {}",
                            record.id(),
                            record.discovery_attempt_id(),
                            entry.campaign_id
                        );
                    }
                    if state
                        .candidates
                        .insert(record.id().clone(), record.stable_finding_id().clone())
                        .is_some()
                    {
                        bail!(
                            "duplicate candidate {} in campaign {}",
                            record.id(),
                            entry.campaign_id
                        );
                    }
                }
                ConvergenceEvent::CandidateDispositionRecorded(record) => {
                    let state = campaigns.get_mut(&entry.campaign_id).with_context(|| {
                        format!(
                            "candidate disposition for {} recorded before campaign {} started",
                            record.candidate_id(),
                            entry.campaign_id
                        )
                    })?;
                    let source_stable = state
                        .candidates
                        .get(record.candidate_id())
                        .cloned()
                        .with_context(|| {
                            format!(
                                "candidate disposition references unknown candidate {} in campaign {}",
                                record.candidate_id(),
                                entry.campaign_id
                            )
                        })?;
                    if state.disposed_candidates.contains(record.candidate_id()) {
                        bail!(
                            "duplicate terminal disposition for candidate {} in campaign {}",
                            record.candidate_id(),
                            entry.campaign_id
                        );
                    }

                    match record.disposition() {
                        CandidateDisposition::Duplicate {
                            canonical_candidate_id,
                        } => {
                            if canonical_candidate_id == record.candidate_id() {
                                bail!(
                                    "candidate {} cannot duplicate itself in campaign {}",
                                    record.candidate_id(),
                                    entry.campaign_id
                                );
                            }
                            let canonical_stable =
                                state.candidates.get(canonical_candidate_id).with_context(|| {
                                    format!(
                                        "candidate {} duplicates missing canonical candidate {} in campaign {}",
                                        record.candidate_id(),
                                        canonical_candidate_id,
                                        entry.campaign_id
                                    )
                                })?;
                            if canonical_stable != &source_stable {
                                bail!(
                                    "candidate {} cannot duplicate candidate {} with a different stable finding id in campaign {}",
                                    record.candidate_id(),
                                    canonical_candidate_id,
                                    entry.campaign_id
                                );
                            }
                        }
                        CandidateDisposition::Superseded {
                            replacement_candidate_id,
                        } => {
                            if replacement_candidate_id == record.candidate_id() {
                                bail!(
                                    "candidate {} cannot supersede itself in campaign {}",
                                    record.candidate_id(),
                                    entry.campaign_id
                                );
                            }
                            if !state.candidates.contains_key(replacement_candidate_id) {
                                bail!(
                                    "candidate {} references missing superseding candidate {} in campaign {}",
                                    record.candidate_id(),
                                    replacement_candidate_id,
                                    entry.campaign_id
                                );
                            }
                        }
                        CandidateDisposition::Verified
                        | CandidateDisposition::RejectedWithEvidence
                        | CandidateDisposition::NeedsContractOrDocumentation
                        | CandidateDisposition::PreExistingOutsideDiffScope => {}
                    }
                    state
                        .disposed_candidates
                        .insert(record.candidate_id().clone());
                }
                ConvergenceEvent::CoverageDispositionRecorded(record) => {
                    let state = campaigns.get_mut(&entry.campaign_id).with_context(|| {
                        format!(
                            "coverage disposition for {} recorded before campaign {} started",
                            record.coverage_cell_id(),
                            entry.campaign_id
                        )
                    })?;
                    record.validate().with_context(|| {
                        format!(
                            "invalid coverage disposition for {} in campaign {}",
                            record.coverage_cell_id(),
                            entry.campaign_id
                        )
                    })?;
                    if !state.cells.contains_key(record.coverage_cell_id()) {
                        bail!(
                            "coverage disposition references undefined cell {} in campaign {}",
                            record.coverage_cell_id(),
                            entry.campaign_id
                        );
                    }
                    if !state
                        .disposed_cells
                        .insert(record.coverage_cell_id().clone())
                    {
                        bail!(
                            "duplicate coverage disposition for cell {} in campaign {}",
                            record.coverage_cell_id(),
                            entry.campaign_id
                        );
                    }
                }
            }
        }

        Ok(())
    }
}

#[derive(Default)]
struct CampaignState {
    epochs: HashSet<EpochId>,
    cells: HashMap<CoverageCellId, EpochId>,
    attempts: HashSet<DiscoveryAttemptId>,
    candidates: HashMap<CandidateId, StableFindingId>,
    disposed_candidates: HashSet<CandidateId>,
    disposed_cells: HashSet<CoverageCellId>,
}
