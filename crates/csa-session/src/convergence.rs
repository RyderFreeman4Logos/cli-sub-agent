//! Immutable identity and snapshot primitives for convergence review campaigns.

use std::{fmt, str::FromStr};

use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use data_encoding::HEXLOWER;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use sha2::{Digest, Sha256};
use ulid::Ulid;

mod authority;
mod campaign;
mod discovery;
mod evidence;
mod finalization;
mod ledger;
mod provider_bundle;
mod secure_fs;
mod store;
mod validation;

pub use authority::{
    CommandAuthorityCatalogIdentity, CommandAuthorityPolicy, CommandAuthoritySnapshot,
    CommandAuthoritySource,
};
pub use discovery::{DiscoveryDirective, DiscoveryRunIntent, next_discovery_directive};

pub use evidence::{
    AdmittedModelIdentity, ArtifactEvidenceRef, CandidateDisposition, CandidateDispositionRecord,
    CandidateRecord, CoverageDispositionRecord, CoverageRequirement, DiscoveryAttemptRecord,
    SessionRelativeArtifactPath,
};

pub use finalization::{CoveragePlanFinalizationRecord, DiscoveryAttemptFinalizationRecord};

pub use ledger::{
    CONVERGENCE_LEDGER_SCHEMA_VERSION, ConvergenceEvent, ConvergenceLedger, ConvergenceLedgerEntry,
    LedgerEventId,
};
pub use provider_bundle::ProviderEvidenceBundle;

#[cfg(test)]
pub(crate) use store::MAX_LEDGER_BYTES;
pub use store::{ConvergenceAppendError, ConvergenceLedgerStore};

const EPOCH_DOMAIN: &[u8] = b"csa-convergence-epoch-v1\0";
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

macro_rules! impl_generated_ulid_id {
    ($name:ident, $label:literal) => {
        impl $name {
            /// Generate a new identifier.
            #[must_use]
            pub fn generate() -> Self {
                Self(Ulid::new().to_string())
            }

            /// Parse and canonicalize a ULID identifier.
            ///
            /// # Errors
            ///
            /// Returns an error when `value` is not a valid ULID.
            pub fn parse(value: &str) -> Result<Self> {
                let ulid = Ulid::from_string(value).map_err(|error| {
                    anyhow::anyhow!("invalid {} ULID '{}': {}", $label, value, error)
                })?;
                Ok(Self(ulid.to_string()))
            }

            /// Return the canonical 26-character ULID.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl_validated_string!($name);
    };
}

/// Validated, canonically encoded ULID identifying a convergence campaign.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CampaignId(String);

impl CampaignId {
    /// Generate a new campaign identifier.
    #[must_use]
    pub fn generate() -> Self {
        Self(Ulid::new().to_string())
    }

    /// Parse and canonicalize a campaign ULID.
    ///
    /// # Errors
    ///
    /// Returns an error when `value` is not a valid ULID.
    pub fn parse(value: &str) -> Result<Self> {
        let ulid = Ulid::from_string(value)
            .map_err(|error| anyhow::anyhow!("invalid campaign id ULID '{value}': {error}"))?;
        Ok(Self(ulid.to_string()))
    }

    /// Return the canonical 26-character ULID.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl_validated_string!(CampaignId);

/// Validated, canonical ULID identifying one discovery attempt.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DiscoveryAttemptId(String);

impl_generated_ulid_id!(DiscoveryAttemptId, "discovery attempt id");

/// Validated, canonical ULID identifying one candidate observation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CandidateId(String);

impl_generated_ulid_id!(CandidateId, "candidate id");

/// Validated, canonical ULID identifying a CSA meta-session.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CsaSessionId(String);

impl CsaSessionId {
    /// Generate an identifier through the CSA session ID source contract.
    #[must_use]
    pub fn generate() -> Self {
        Self(crate::validate::new_session_id())
    }

    /// Parse and canonicalize a CSA meta-session ULID.
    ///
    /// # Errors
    ///
    /// Returns an error when `value` is not accepted by the CSA session ID validator.
    pub fn parse(value: &str) -> Result<Self> {
        crate::validate::validate_session_id(value)?;
        let ulid = Ulid::from_string(value)
            .map_err(|error| anyhow::anyhow!("invalid CSA session id ULID '{value}': {error}"))?;
        Ok(Self(ulid.to_string()))
    }

    /// Return the canonical 26-character ULID.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl_validated_string!(CsaSessionId);

/// Canonical SHA-256 digest encoded as `sha256:<64 lowercase hex>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Sha256Digest(String);

impl Sha256Digest {
    /// Compute the SHA-256 digest of arbitrary evidence bytes.
    #[must_use]
    pub fn compute(bytes: &[u8]) -> Self {
        Self::from_hash(&Sha256::digest(bytes))
    }

    /// Parse and canonicalize a prefixed SHA-256 digest.
    ///
    /// The hexadecimal payload is case-insensitive on input and lowercase on output.
    ///
    /// # Errors
    ///
    /// Returns an error for a missing prefix, a payload not exactly 64 characters long,
    /// or a non-hexadecimal payload.
    pub fn parse(value: &str) -> Result<Self> {
        let Some(hex) = value.strip_prefix("sha256:") else {
            bail!("sha256 digest must start with 'sha256:'");
        };
        if hex.len() != 64 {
            bail!(
                "sha256 digest payload must contain exactly 64 hex characters, got {}",
                hex.len()
            );
        }
        if !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            bail!("sha256 digest payload must contain only hexadecimal characters");
        }
        Ok(Self(format!("sha256:{}", hex.to_ascii_lowercase())))
    }

    fn from_hash(hash: &[u8]) -> Self {
        Self(format!("sha256:{}", HEXLOWER.encode(hash)))
    }

    /// Return the canonical prefixed digest.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl_validated_string!(Sha256Digest);

/// Canonical full Git object identifier.
///
/// Both 40-character SHA-1 and 64-character SHA-256 object IDs are accepted.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GitObjectId(String);

impl GitObjectId {
    /// Parse a full hexadecimal Git object identifier and canonicalize it to lowercase.
    ///
    /// # Errors
    ///
    /// Returns an error unless `value` is exactly 40 or 64 hexadecimal characters.
    pub fn parse(value: &str) -> Result<Self> {
        if !matches!(value.len(), 40 | 64) {
            bail!(
                "git object id must contain exactly 40 or 64 hex characters, got {}",
                value.len()
            );
        }
        if !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            bail!("git object id must contain only hexadecimal characters");
        }
        Ok(Self(value.to_ascii_lowercase()))
    }

    /// Return the canonical lowercase object ID.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl_validated_string!(GitObjectId);

/// Immutable metadata snapshot for a convergence campaign.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CampaignRecord {
    id: CampaignId,
    created_at: DateTime<Utc>,
    policy_digest: Option<Sha256Digest>,
    command_authority: CommandAuthoritySnapshot,
    command_authority_digest: Sha256Digest,
}

impl CampaignRecord {
    /// Construct a campaign metadata snapshot.
    #[must_use]
    pub fn new(
        id: CampaignId,
        created_at: DateTime<Utc>,
        policy_digest: Option<Sha256Digest>,
        command_authority: CommandAuthoritySnapshot,
    ) -> Self {
        let command_authority_digest = command_authority.digest();
        Self {
            id,
            created_at,
            policy_digest,
            command_authority,
            command_authority_digest,
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test(
        id: CampaignId,
        created_at: DateTime<Utc>,
        policy_digest: Option<Sha256Digest>,
    ) -> Self {
        let authority = CommandAuthoritySnapshot::new(
            CommandAuthoritySource::direct("test fixture").expect("test source"),
            CommandAuthorityPolicy::new(false, Vec::new(), false, true).expect("test policy"),
            CommandAuthorityCatalogIdentity::new("test catalog", "v1")
                .expect("test catalog identity"),
            vec![
                AdmittedModelIdentity::new("codex", "openai", "gpt-5.6", "high")
                    .expect("test admitted identity"),
            ],
        )
        .expect("test authority");
        Self::new(id, created_at, policy_digest, authority)
    }

    /// Return the campaign identifier.
    #[must_use]
    pub fn id(&self) -> &CampaignId {
        &self.id
    }

    /// Return the campaign creation timestamp.
    #[must_use]
    pub fn created_at(&self) -> &DateTime<Utc> {
        &self.created_at
    }

    /// Return the optional policy snapshot digest.
    #[must_use]
    pub fn policy_digest(&self) -> Option<&Sha256Digest> {
        self.policy_digest.as_ref()
    }

    /// Return the canonical command authority captured for this campaign.
    #[must_use]
    pub fn command_authority(&self) -> &CommandAuthoritySnapshot {
        &self.command_authority
    }

    /// Return the verified digest of the canonical command authority.
    #[must_use]
    pub fn command_authority_digest(&self) -> &Sha256Digest {
        &self.command_authority_digest
    }
}

/// Deterministic identity of a frozen convergence discovery epoch.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EpochId(Sha256Digest);

impl EpochId {
    /// Compute an epoch ID from canonical base, head, and diff identities.
    #[must_use]
    pub fn compute(
        base_oid: &GitObjectId,
        head_oid: &GitObjectId,
        diff_digest: &Sha256Digest,
    ) -> Self {
        Self(hash_fields(
            EPOCH_DOMAIN,
            &[base_oid.as_str(), head_oid.as_str(), diff_digest.as_str()],
        ))
    }

    /// Parse a serialized epoch ID.
    ///
    /// # Errors
    ///
    /// Returns an error unless `value` is a canonicalizable SHA-256 digest.
    pub fn parse(value: &str) -> Result<Self> {
        Sha256Digest::parse(value).map(Self)
    }

    /// Return the canonical serialized epoch ID.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl_validated_string!(EpochId);

/// Immutable snapshot of the inputs defining a discovery epoch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EpochRecord {
    id: EpochId,
    base_oid: GitObjectId,
    head_oid: GitObjectId,
    diff_digest: Sha256Digest,
}

impl EpochRecord {
    /// Construct an epoch record and derive its identity from its immutable inputs.
    #[must_use]
    pub fn new(base_oid: GitObjectId, head_oid: GitObjectId, diff_digest: Sha256Digest) -> Self {
        let id = EpochId::compute(&base_oid, &head_oid, &diff_digest);
        Self {
            id,
            base_oid,
            head_oid,
            diff_digest,
        }
    }

    /// Return the stored epoch identity.
    #[must_use]
    pub fn id(&self) -> &EpochId {
        &self.id
    }

    /// Return the frozen base object ID.
    #[must_use]
    pub fn base_oid(&self) -> &GitObjectId {
        &self.base_oid
    }

    /// Return the frozen head object ID.
    #[must_use]
    pub fn head_oid(&self) -> &GitObjectId {
        &self.head_oid
    }

    /// Return the digest of the canonical frozen diff.
    #[must_use]
    pub fn diff_digest(&self) -> &Sha256Digest {
        &self.diff_digest
    }

    /// Recompute the epoch identity from the stored immutable inputs.
    #[must_use]
    pub fn recompute_id(&self) -> EpochId {
        EpochId::compute(&self.base_oid, &self.head_oid, &self.diff_digest)
    }

    /// Verify that the stored epoch identity matches its inputs.
    ///
    /// # Errors
    ///
    /// Returns an error when the record identity was tampered with or mismatched.
    pub fn validate(&self) -> Result<()> {
        let expected = self.recompute_id();
        if self.id != expected {
            bail!(
                "epoch id mismatch: stored {}, recomputed {}",
                self.id,
                expected
            );
        }
        Ok(())
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
    if normalized.is_empty() {
        bail!("{field} must not be blank");
    }
    if normalized.contains('\0') {
        bail!("{field} must not contain NUL bytes");
    }
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
