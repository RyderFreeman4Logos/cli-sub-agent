use std::{collections::HashSet, fmt, str::FromStr};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use ulid::Ulid;

use super::{
    ArtifactEvidenceRef, CampaignId, CampaignRecord, CandidateDisposition,
    CandidateDispositionRecord, CandidateRecord, CleanRoomReviewRecord,
    CompletionAuthorizationRecord, CoverageCellRecord, CoverageDispositionRecord,
    CoveragePlanFinalizationRecord, CoverageRequirement, DiscoveryAttemptFinalizationRecord,
    DiscoveryAttemptId, DiscoveryAttemptRecord, EpochRecord, MergeAttestationRecord,
    RepairBatchRecord, RepairHandoffRecord, RootClusterRecord, Sha256Digest, hash_fields,
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
    /// One completion attempt was bound to an owned workspace lease before external work began.
    CompletionAuthorizationRecorded(CompletionAuthorizationRecord),
    /// A fresh clean-room review reported an exact zero-finding terminal result.
    FinalReviewRecorded(Box<CleanRoomReviewRecord>),
    /// The authoritative terminal bindings were sealed for merge.
    MergeAttestationRecorded(Box<MergeAttestationRecord>),
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

const BINDING_DOMAIN: &[u8] = b"csa-convergence-attestation-binding-v1\0";

pub(super) struct SetDigests {
    pub(super) coverage: Sha256Digest,
    pub(super) candidates: Sha256Digest,
    pub(super) dispositions: Sha256Digest,
    pub(super) clusters: Sha256Digest,
    pub(super) repairs: Sha256Digest,
}

pub(super) fn set_digests(
    entries: &[ConvergenceLedgerEntry],
    campaign_id: &CampaignId,
    epoch: &EpochRecord,
) -> Result<SetDigests> {
    let mut cells = HashSet::new();
    let mut required = HashSet::new();
    let mut attempted = HashSet::new();
    let mut attempts = HashSet::<DiscoveryAttemptId>::new();
    let mut finalized = 0_usize;
    let mut candidate_count = 0_usize;
    let mut coverage = Vec::new();
    let mut candidate_set = Vec::new();
    let mut dispositions = Vec::new();
    let mut clusters = Vec::new();
    let mut repairs = Vec::new();
    let mut plan_finalized = false;

    for entry in entries
        .iter()
        .filter(|entry| entry.campaign_id() == campaign_id)
    {
        match entry.event() {
            ConvergenceEvent::CoverageCellDefined(record) if record.epoch_id() == epoch.id() => {
                cells.insert(record.id().clone());
                coverage.push(record_digest("coverage_cell", record));
            }
            ConvergenceEvent::CoverageDispositionRecorded(record)
                if cells.contains(record.coverage_cell_id()) =>
            {
                if record.requirement() == CoverageRequirement::Required {
                    required.insert(record.coverage_cell_id().clone());
                }
                coverage.push(record_digest("coverage_disposition", record));
            }
            ConvergenceEvent::CoveragePlanFinalized(record) if record.epoch_id() == epoch.id() => {
                plan_finalized = true;
                coverage.push(record_digest("coverage_finalized", record));
            }
            ConvergenceEvent::DiscoveryAttemptRecorded(record)
                if record.epoch_id() == epoch.id() =>
            {
                if record.more_candidates_possible() || !record.unscanned_items().is_empty() {
                    bail!("current epoch discovery is incomplete");
                }
                attempts.insert(record.id().clone());
                attempted.insert(record.coverage_cell_id().clone());
            }
            ConvergenceEvent::DiscoveryAttemptFinalized(record)
                if attempts.contains(record.discovery_attempt_id()) =>
            {
                finalized += 1;
            }
            ConvergenceEvent::CandidateRecorded(record)
                if attempts.contains(record.discovery_attempt_id()) =>
            {
                candidate_count += 1;
                candidate_set.push(record_digest("candidate", record));
            }
            ConvergenceEvent::CandidateDispositionRecorded(record)
                if record.epoch_id() == epoch.id() =>
            {
                dispositions.push(record.clone());
            }
            ConvergenceEvent::RootClusterRecorded(record) if record.epoch_id() == epoch.id() => {
                clusters.push(record.clone());
            }
            ConvergenceEvent::RepairBatchRecorded(record) if record.epoch_id() == epoch.id() => {
                repairs.push(record.clone());
            }
            _ => {}
        }
    }
    let verified = dispositions
        .iter()
        .filter(|record| record.disposition() == &CandidateDisposition::Verified)
        .map(CandidateDispositionRecord::candidate_id)
        .collect::<HashSet<_>>();
    let clustered = clusters
        .iter()
        .flat_map(|record| record.candidate_ids())
        .collect::<HashSet<_>>();
    if !plan_finalized
        || !required.is_subset(&attempted)
        || attempts.len() != finalized
        || candidate_count != dispositions.len()
        || verified != clustered
    {
        bail!("current epoch coverage, candidate, or disposition set is incomplete");
    }
    coverage.sort_unstable_by(|a, b| a.as_str().cmp(b.as_str()));
    candidate_set.sort_unstable_by(|a, b| a.as_str().cmp(b.as_str()));
    Ok(SetDigests {
        coverage: bind("coverage_manifest", &digest_refs(&coverage)),
        candidates: bind("candidate_set", &digest_refs(&candidate_set)),
        dispositions: CandidateDispositionRecord::set_digest(&dispositions),
        clusters: RootClusterRecord::set_digest(&clusters),
        repairs: RepairBatchRecord::set_digest(&repairs),
    })
}

fn record_digest(label: &str, record: &impl serde::Serialize) -> Sha256Digest {
    let bytes = serde_json::to_vec(record).expect("validated records serialize");
    bind(label, &[Sha256Digest::compute(&bytes).as_str()])
}

pub(super) fn terminal_pair(
    entries: &[ConvergenceLedgerEntry],
) -> Result<(&CleanRoomReviewRecord, &MergeAttestationRecord)> {
    match entries {
        [.., review_entry, attestation_entry] => {
            match (review_entry.event(), attestation_entry.event()) {
                (
                    ConvergenceEvent::FinalReviewRecorded(review),
                    ConvergenceEvent::MergeAttestationRecorded(attestation),
                ) => Ok((review, attestation)),
                _ => bail!("ledger has no terminal attestation pair"),
            }
        }
        _ => bail!("ledger has no terminal attestation pair"),
    }
}

pub(super) fn artifact_refs<'a>(
    entries: &'a [ConvergenceLedgerEntry],
    campaign_id: &CampaignId,
) -> Vec<&'a ArtifactEvidenceRef> {
    entries
        .iter()
        .filter(|entry| entry.campaign_id() == campaign_id)
        .filter_map(|entry| match entry.event() {
            ConvergenceEvent::DiscoveryAttemptRecorded(record) => Some(record.artifact()),
            ConvergenceEvent::CandidateRecorded(record) => Some(record.artifact()),
            ConvergenceEvent::CandidateDispositionRecorded(record) => Some(record.artifact()),
            ConvergenceEvent::RepairHandoffRecorded(record) => Some(record.artifact()),
            ConvergenceEvent::FinalReviewRecorded(record) => Some(record.artifact()),
            ConvergenceEvent::MergeAttestationRecorded(record) => {
                Some(record.gate_evidence.artifact())
            }
            _ => None,
        })
        .collect()
}

pub(super) fn bind(label: &str, fields: &[&str]) -> Sha256Digest {
    let mut framed = Vec::with_capacity(fields.len() + 1);
    framed.push(label);
    framed.extend_from_slice(fields);
    hash_fields(BINDING_DOMAIN, &framed)
}

fn digest_refs(digests: &[Sha256Digest]) -> Vec<&str> {
    digests.iter().map(Sha256Digest::as_str).collect()
}

pub(super) fn require_schema(bytes: &[u8], expected: &str) -> Result<()> {
    let value: serde_json::Value = serde_json::from_slice(bytes).context("artifact is not JSON")?;
    if value.get("schema").and_then(serde_json::Value::as_str) != Some(expected) {
        bail!("artifact schema is not '{expected}'");
    }
    Ok(())
}
