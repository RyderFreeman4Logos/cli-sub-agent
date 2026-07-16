//! Immutable identity and snapshot primitives for convergence review campaigns.

use std::{fmt, str::FromStr};

use anyhow::{Result, bail};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use sha2::{Digest, Sha256};

mod action_journal;
mod action_journal_store;
mod attestation;
mod authority;
mod authorization;
mod campaign;
mod completion_authorization;
mod discovery;
mod evidence;
mod finalization;
mod identity;
mod ledger;
mod model_evidence;
mod provider_bundle;
mod repair;
mod repair_intent;
mod repair_intent_store;
mod secure_fs;
mod store;
mod terminal_publication;
mod validation;
mod validation_attestation;
mod verification_evidence;
pub use action_journal::{
    COMPLETION_ACTION_JOURNAL_SCHEMA_VERSION, CompletionActionClaim, CompletionActionId,
    CompletionActionJournal, CompletionActionJournalError, CompletionActionJournalRead,
    CompletionActionRecord, CompletionActionState, LEGACY_COMPLETION_ACTION_JOURNAL_SCHEMA_VERSION,
    LegacyCompletionActionJournal, MAX_COMPLETION_ACTION_RECORDS,
    MAX_PROVIDER_TURN_EXECUTIONS_PER_ACTION, ProviderTurnExecutionId, ProviderTurnExecutionRecord,
    ProviderTurnExecutionState, ProviderTurnReservation, parse_legacy_completion_action_journal,
};
pub use attestation::{
    AttestationArtifactReader, AttestationBindingDigests, CLEAN_ROOM_REVIEW_SCHEMA_ID,
    CleanRoomReviewArtifactBindings, CleanRoomReviewRecord, CleanupConfirmation,
    GATE_EVIDENCE_SCHEMA_ID, GateCommandResult, GateEvidenceRecord,
    LEGACY_CLEAN_ROOM_REVIEW_SCHEMA_ID, MERGE_ATTESTATION_SCHEMA_ID, MergeAttestationRecord,
    TerminalExecutionBinding,
};
pub use authority::{
    CommandAuthorityCatalogIdentity, CommandAuthorityPolicy, CommandAuthoritySnapshot,
    CommandAuthoritySource,
};
pub use authorization::{ConsolidatedRepairAuthorization, authorize_consolidated_repairs};
pub use completion_authorization::{CompletionAuthorizationRecord, WorkspaceLeaseIdentity};
pub use discovery::{DiscoveryDirective, DiscoveryRunIntent, next_discovery_directive};
pub use evidence::{
    AdmittedModelIdentity, ArtifactEvidenceRef, CandidateDisposition, CandidateRecord,
    CoverageDispositionRecord, CoverageRequirement, DiscoveryAttemptRecord,
    SessionRelativeArtifactPath,
};
pub use finalization::{CoveragePlanFinalizationRecord, DiscoveryAttemptFinalizationRecord};
pub use identity::{
    CampaignId, CampaignRecord, CandidateId, CsaSessionId, DiscoveryAttemptId, EpochId,
    EpochRecord, GitObjectId, Sha256Digest,
};
pub use validation_attestation::{
    compute_attestation_bindings, verify_merge_attestation, verify_terminal_artifact_pair,
};
pub use verification_evidence::{
    CandidateDispositionRecord, CandidateVerificationEvidence, VerificationIndependence,
};

pub use ledger::{
    CONVERGENCE_LEDGER_SCHEMA_VERSION, ConvergenceEvent, ConvergenceLedger, ConvergenceLedgerEntry,
    LedgerEventId,
};
pub use model_evidence::{
    IndependentlyVerifiedModel, ModelEvidence, ModelEvidenceConfidence, ModelEvidenceProvenance,
    ObservedToolEvidence,
};
pub use provider_bundle::ProviderEvidenceBundle;
pub use repair::{
    RepairBatchId, RepairBatchRecord, RepairHandoffId, RepairHandoffRecord, RootClusterId,
    RootClusterRecord,
};
pub use repair_intent::{
    MAX_REPAIR_INTENT_BATCHES, REPAIR_INTENT_SCHEMA_VERSION, RepairIntent, RepairIntentState,
};
pub use repair_intent_store::{RepairIntentRead, RepairIntentStoreError};

pub use action_journal_store::CompletionActionJournalStoreError;
#[cfg(test)]
pub(crate) use store::MAX_LEDGER_BYTES;
pub use store::{ConvergenceAppendError, ConvergenceLedgerStore};
pub use terminal_publication::FinalAttestationPublicationError;

const COVERAGE_CELL_DOMAIN: &[u8] = b"csa-convergence-coverage-cell-v1\0";
const STABLE_FINDING_DOMAIN: &[u8] = b"csa-convergence-stable-finding-v1\0";

macro_rules! impl_validated_string {
    ($name:ident) => {
        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = anyhow::Error;

            fn from_str(value: &str) -> Result<Self> {
                Self::parse(value)
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::parse(&value).map_err(D::Error::custom)
            }
        }
    };
}

impl CampaignRecord {
    #[cfg(test)]
    pub(crate) fn for_test(
        id: CampaignId,
        created_at: chrono::DateTime<chrono::Utc>,
        policy_digest: Option<Sha256Digest>,
    ) -> Self {
        let authority = CommandAuthoritySnapshot::new(
            CommandAuthoritySource::direct("test fixture").expect("test source"),
            CommandAuthorityPolicy::new(false, Vec::new(), false, true).expect("test policy"),
            CommandAuthorityCatalogIdentity::new("test catalog", "v1").expect("test catalog"),
            vec![
                AdmittedModelIdentity::new("codex", "openai", "gpt-5.6", "high")
                    .expect("test identity"),
            ],
        )
        .expect("test authority");
        Self::new(id, created_at, policy_digest, authority)
    }

    /// Return the campaign creation timestamp.
    #[must_use]
    pub fn created_at(&self) -> &chrono::DateTime<chrono::Utc> {
        &self.created_at
    }
}

/// Normalized semantic partition covered by a discovery cell.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct CoverageScope {
    kind: String,
    key: String,
}

impl CoverageScope {
    /// Construct a scope from trimmed, nonblank kind and key fields.
    ///
    /// # Errors
    ///
    /// Returns an error when either normalized field is blank.
    pub fn new(kind: &str, key: &str) -> Result<Self> {
        Ok(Self {
            kind: normalize_nonblank("coverage scope kind", kind)?,
            key: normalize_nonblank("coverage scope key", key)?,
        })
    }

    /// Return the normalized scope kind.
    #[must_use]
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// Return the normalized scope key.
    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }
}

impl<'de> Deserialize<'de> for CoverageScope {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawCoverageScope {
            kind: String,
            key: String,
        }

        let raw = RawCoverageScope::deserialize(deserializer)?;
        Self::new(&raw.kind, &raw.key).map_err(D::Error::custom)
    }
}

/// Normalized, nonblank semantic review lens.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SemanticLens(String);

impl SemanticLens {
    /// Construct a semantic lens from a trimmed, nonblank value.
    ///
    /// # Errors
    ///
    /// Returns an error when the normalized lens is blank.
    pub fn new(value: &str) -> Result<Self> {
        normalize_nonblank("semantic lens", value).map(Self)
    }

    /// Parse a serialized semantic lens.
    ///
    /// # Errors
    ///
    /// Returns an error when the normalized lens is blank.
    pub fn parse(value: &str) -> Result<Self> {
        Self::new(value)
    }

    /// Return the normalized lens.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl_validated_string!(SemanticLens);

/// Deterministic identity of an epoch, scope, and semantic lens cell.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CoverageCellId(Sha256Digest);

impl CoverageCellId {
    /// Compute a coverage cell identity from its semantic inputs.
    #[must_use]
    pub fn compute(epoch_id: &EpochId, scope: &CoverageScope, lens: &SemanticLens) -> Self {
        Self(hash_fields(
            COVERAGE_CELL_DOMAIN,
            &[epoch_id.as_str(), scope.kind(), scope.key(), lens.as_str()],
        ))
    }

    /// Parse a serialized coverage cell ID.
    ///
    /// # Errors
    ///
    /// Returns an error unless `value` is a canonicalizable SHA-256 digest.
    pub fn parse(value: &str) -> Result<Self> {
        Sha256Digest::parse(value).map(Self)
    }

    /// Return the canonical serialized coverage cell ID.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl_validated_string!(CoverageCellId);

/// Immutable snapshot of a semantic discovery coverage cell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoverageCellRecord {
    id: CoverageCellId,
    epoch_id: EpochId,
    scope: CoverageScope,
    lens: SemanticLens,
}

impl CoverageCellRecord {
    /// Construct a coverage cell record and derive its identity.
    #[must_use]
    pub fn new(epoch_id: EpochId, scope: CoverageScope, lens: SemanticLens) -> Self {
        let id = CoverageCellId::compute(&epoch_id, &scope, &lens);
        Self {
            id,
            epoch_id,
            scope,
            lens,
        }
    }

    /// Return the stored cell identity.
    #[must_use]
    pub fn id(&self) -> &CoverageCellId {
        &self.id
    }

    /// Return the epoch containing this cell.
    #[must_use]
    pub fn epoch_id(&self) -> &EpochId {
        &self.epoch_id
    }

    /// Return the cell coverage scope.
    #[must_use]
    pub fn scope(&self) -> &CoverageScope {
        &self.scope
    }

    /// Return the cell semantic lens.
    #[must_use]
    pub fn lens(&self) -> &SemanticLens {
        &self.lens
    }

    /// Recompute the cell identity from its semantic inputs.
    #[must_use]
    pub fn recompute_id(&self) -> CoverageCellId {
        CoverageCellId::compute(&self.epoch_id, &self.scope, &self.lens)
    }

    /// Verify that the stored cell identity matches its semantic inputs.
    ///
    /// # Errors
    ///
    /// Returns an error when the record identity was tampered with or mismatched.
    pub fn validate(&self) -> Result<()> {
        let expected = self.recompute_id();
        if self.id != expected {
            bail!(
                "coverage cell id mismatch: stored {}, recomputed {}",
                self.id,
                expected
            );
        }
        Ok(())
    }
}

/// Canonical semantic identity of a finding, independent of location evidence.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct SemanticFindingIdentity {
    violated_invariant: String,
    trigger_failure_mode: String,
    primary_component: String,
    bug_class: String,
}

impl SemanticFindingIdentity {
    /// Construct an identity from trimmed, nonblank semantic fields.
    ///
    /// Paths, spans, and anchors are deliberately absent and cannot affect the identity.
    ///
    /// # Errors
    ///
    /// Returns an error when any normalized semantic field is blank.
    pub fn new(
        violated_invariant: &str,
        trigger_failure_mode: &str,
        primary_component: &str,
        bug_class: &str,
    ) -> Result<Self> {
        Ok(Self {
            violated_invariant: normalize_nonblank(
                "finding violated invariant",
                violated_invariant,
            )?,
            trigger_failure_mode: normalize_nonblank(
                "finding trigger or failure mode",
                trigger_failure_mode,
            )?,
            primary_component: normalize_nonblank("finding primary component", primary_component)?,
            bug_class: normalize_nonblank("finding bug class", bug_class)?,
        })
    }

    /// Return the invariant the finding violates.
    #[must_use]
    pub fn violated_invariant(&self) -> &str {
        &self.violated_invariant
    }

    /// Return the trigger or failure mode that exposes the violation.
    #[must_use]
    pub fn trigger_failure_mode(&self) -> &str {
        &self.trigger_failure_mode
    }

    /// Return the primary component or symbol affected by the finding.
    #[must_use]
    pub fn primary_component(&self) -> &str {
        &self.primary_component
    }

    /// Return the canonical bug class.
    #[must_use]
    pub fn bug_class(&self) -> &str {
        &self.bug_class
    }
}

impl<'de> Deserialize<'de> for SemanticFindingIdentity {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawSemanticFindingIdentity {
            violated_invariant: String,
            trigger_failure_mode: String,
            primary_component: String,
            bug_class: String,
        }

        let raw = RawSemanticFindingIdentity::deserialize(deserializer)?;
        Self::new(
            &raw.violated_invariant,
            &raw.trigger_failure_mode,
            &raw.primary_component,
            &raw.bug_class,
        )
        .map_err(D::Error::custom)
    }
}

/// Stable finding ID computed only from canonical semantic finding fields.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StableFindingId(Sha256Digest);

impl StableFindingId {
    /// Compute a stable finding ID without location evidence.
    #[must_use]
    pub fn compute(identity: &SemanticFindingIdentity) -> Self {
        Self(hash_fields(
            STABLE_FINDING_DOMAIN,
            &[
                identity.violated_invariant(),
                identity.trigger_failure_mode(),
                identity.primary_component(),
                identity.bug_class(),
            ],
        ))
    }

    /// Parse a serialized stable finding ID.
    ///
    /// # Errors
    ///
    /// Returns an error unless `value` is a canonicalizable SHA-256 digest.
    pub fn parse(value: &str) -> Result<Self> {
        Sha256Digest::parse(value).map(Self)
    }

    /// Return the canonical serialized stable finding ID.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl_validated_string!(StableFindingId);

fn normalize_nonblank(field: &str, value: &str) -> Result<String> {
    let normalized = value.trim();
    anyhow::ensure!(!normalized.is_empty(), "{field} must not be blank");
    anyhow::ensure!(
        !normalized.contains('\0'),
        "{field} must not contain NUL bytes"
    );
    Ok(normalized.to_string())
}

pub(crate) fn hash_fields(domain: &[u8], fields: &[&str]) -> Sha256Digest {
    let mut hasher = Sha256::new();
    hasher.update(domain);
    for field in fields {
        hasher.update((field.len() as u64).to_be_bytes());
        hasher.update(field.as_bytes());
    }
    Sha256Digest::from_hash(&hasher.finalize())
}
