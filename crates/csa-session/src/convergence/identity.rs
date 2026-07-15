//! Immutable campaign and epoch identities shared by convergence protocols.

use std::{fmt, str::FromStr};

use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use data_encoding::HEXLOWER;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use sha2::{Digest, Sha256};
use ulid::Ulid;

use super::CommandAuthoritySnapshot;

const EPOCH_DOMAIN: &[u8] = b"csa-convergence-epoch-v1\0";

macro_rules! validated_string {
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

macro_rules! generated_ulid_id {
    ($name:ident, $label:literal) => {
        impl $name {
            /// Generate a new identifier.
            #[must_use]
            pub fn generate() -> Self {
                Self(Ulid::new().to_string())
            }
            /// Parse and canonicalize a ULID identifier.
            pub fn parse(value: &str) -> Result<Self> {
                let ulid = Ulid::from_string(value).map_err(|error| {
                    anyhow::anyhow!("invalid {} ULID '{}': {}", $label, value, error)
                })?;
                Ok(Self(ulid.to_string()))
            }
            /// Return the canonical ULID.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
        validated_string!($name);
    };
}

/// Validated ULID identifying a convergence campaign.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CampaignId(String);

impl CampaignId {
    /// Generate a new campaign identifier.
    #[must_use]
    pub fn generate() -> Self {
        Self(Ulid::new().to_string())
    }
    /// Parse and canonicalize a campaign ULID.
    pub fn parse(value: &str) -> Result<Self> {
        let ulid = Ulid::from_string(value)
            .map_err(|error| anyhow::anyhow!("invalid campaign id ULID '{value}': {error}"))?;
        Ok(Self(ulid.to_string()))
    }
    /// Return the canonical ULID.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
validated_string!(CampaignId);

/// Validated ULID identifying one discovery attempt.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DiscoveryAttemptId(String);
generated_ulid_id!(DiscoveryAttemptId, "discovery attempt id");

/// Validated ULID identifying one candidate observation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CandidateId(String);
generated_ulid_id!(CandidateId, "candidate id");

/// Validated ULID identifying a CSA meta-session.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CsaSessionId(String);
impl CsaSessionId {
    /// Generate an identifier through the CSA session ID contract.
    #[must_use]
    pub fn generate() -> Self {
        Self(crate::validate::new_session_id())
    }
    /// Parse and canonicalize a CSA meta-session ULID.
    pub fn parse(value: &str) -> Result<Self> {
        crate::validate::validate_session_id(value)?;
        let ulid = Ulid::from_string(value)
            .map_err(|error| anyhow::anyhow!("invalid CSA session id ULID '{value}': {error}"))?;
        Ok(Self(ulid.to_string()))
    }
    /// Return the canonical ULID.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
validated_string!(CsaSessionId);

/// Canonical SHA-256 digest encoded as `sha256:<64 lowercase hex>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Sha256Digest(String);
impl Sha256Digest {
    /// Compute a digest from bytes.
    #[must_use]
    pub fn compute(bytes: &[u8]) -> Self {
        Self::from_hash(&Sha256::digest(bytes))
    }
    /// Parse and canonicalize a prefixed digest.
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
    pub(super) fn from_hash(hash: &[u8]) -> Self {
        Self(format!("sha256:{}", HEXLOWER.encode(hash)))
    }
    /// Return the canonical prefixed digest.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
validated_string!(Sha256Digest);

/// Canonical full Git object identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GitObjectId(String);
impl GitObjectId {
    /// Parse a full SHA-1 or SHA-256 Git object identifier.
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
validated_string!(GitObjectId);

/// Immutable metadata snapshot for a convergence campaign.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CampaignRecord {
    pub(super) id: CampaignId,
    pub(super) created_at: DateTime<Utc>,
    pub(super) policy_digest: Option<Sha256Digest>,
    pub(super) command_authority: CommandAuthoritySnapshot,
    pub(super) command_authority_digest: Sha256Digest,
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
    /// Return the campaign identifier.
    #[must_use]
    pub fn id(&self) -> &CampaignId {
        &self.id
    }
    /// Return the optional policy snapshot digest.
    #[must_use]
    pub fn policy_digest(&self) -> Option<&Sha256Digest> {
        self.policy_digest.as_ref()
    }
    /// Return the captured command authority.
    #[must_use]
    pub fn command_authority(&self) -> &CommandAuthoritySnapshot {
        &self.command_authority
    }
    /// Return the command authority digest.
    #[must_use]
    pub fn command_authority_digest(&self) -> &Sha256Digest {
        &self.command_authority_digest
    }
}

/// Deterministic identity of a frozen convergence epoch.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EpochId(Sha256Digest);
impl EpochId {
    /// Compute an epoch ID from canonical tuple inputs.
    #[must_use]
    pub fn compute(
        base_oid: &GitObjectId,
        head_oid: &GitObjectId,
        diff_digest: &Sha256Digest,
    ) -> Self {
        Self(super::hash_fields(
            EPOCH_DOMAIN,
            &[base_oid.as_str(), head_oid.as_str(), diff_digest.as_str()],
        ))
    }
    /// Parse a serialized epoch ID.
    pub fn parse(value: &str) -> Result<Self> {
        Sha256Digest::parse(value).map(Self)
    }
    /// Return the canonical serialized epoch ID.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}
validated_string!(EpochId);

/// Immutable tuple defining a discovery epoch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EpochRecord {
    id: EpochId,
    base_oid: GitObjectId,
    head_oid: GitObjectId,
    diff_digest: Sha256Digest,
}
impl EpochRecord {
    /// Construct an epoch and derive its identity.
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
    /// Return the epoch identity.
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
    /// Return the frozen diff digest.
    #[must_use]
    pub fn diff_digest(&self) -> &Sha256Digest {
        &self.diff_digest
    }
    /// Recompute the epoch identity.
    #[must_use]
    pub fn recompute_id(&self) -> EpochId {
        EpochId::compute(&self.base_oid, &self.head_oid, &self.diff_digest)
    }
    /// Validate the derived epoch identity.
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
