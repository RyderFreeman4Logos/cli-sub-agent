use std::collections::HashSet;

use anyhow::{Result, bail};
use serde::{Deserialize, Deserializer, Serialize, de::Error as _};

use super::{AdmittedModelIdentity, Sha256Digest, normalize_nonblank};

const COMMAND_AUTHORITY_DOMAIN: &[u8] = b"csa-command-authority-v1\0";

/// Origin of the command-level model authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CommandAuthoritySource {
    /// A named tier selected from a specific configuration surface.
    Tier { name: String, origin: String },
    /// An explicit tool or model selection from a specific configuration surface.
    Direct { origin: String },
    /// A default model resolved from a specific configuration surface.
    DefaultModel { origin: String },
}

impl CommandAuthoritySource {
    /// Construct a named tier source.
    ///
    /// # Errors
    /// Returns an error when the tier name or origin is blank or contains NUL.
    pub fn tier(name: &str, origin: &str) -> Result<Self> {
        Ok(Self::Tier {
            name: normalize_nonblank("command authority tier name", name)?,
            origin: normalize_nonblank("command authority source origin", origin)?,
        })
    }

    /// Construct an explicit direct-selection source.
    ///
    /// # Errors
    /// Returns an error when the origin is blank or contains NUL.
    pub fn direct(origin: &str) -> Result<Self> {
        Ok(Self::Direct {
            origin: normalize_nonblank("command authority source origin", origin)?,
        })
    }

    /// Construct a default-model source.
    ///
    /// # Errors
    /// Returns an error when the origin is blank or contains NUL.
    pub fn default_model(origin: &str) -> Result<Self> {
        Ok(Self::DefaultModel {
            origin: normalize_nonblank("command authority source origin", origin)?,
        })
    }

    /// Return the configuration origin that selected this authority.
    #[must_use]
    pub fn origin(&self) -> &str {
        match self {
            Self::Tier { origin, .. } | Self::Direct { origin } | Self::DefaultModel { origin } => {
                origin
            }
        }
    }

    /// Return the display tier name when this authority came from a tier.
    #[must_use]
    pub fn tier_name(&self) -> Option<&str> {
        match self {
            Self::Tier { name, .. } => Some(name),
            Self::Direct { .. } | Self::DefaultModel { .. } => None,
        }
    }
}

impl<'de> Deserialize<'de> for CommandAuthoritySource {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
        enum RawSource {
            Tier { name: String, origin: String },
            Direct { origin: String },
            DefaultModel { origin: String },
        }

        match RawSource::deserialize(deserializer)? {
            RawSource::Tier { name, origin } => Self::tier(&name, &origin),
            RawSource::Direct { origin } => Self::direct(&origin),
            RawSource::DefaultModel { origin } => Self::default_model(&origin),
        }
        .map_err(D::Error::custom)
    }
}

/// Frozen fallback and command-line authority flags.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommandAuthorityPolicy {
    fallback_enabled: bool,
    preference_order: Vec<String>,
    force_ignore: bool,
    no_failover: bool,
}

impl CommandAuthorityPolicy {
    /// Construct the frozen command policy.
    ///
    /// # Errors
    /// Returns an error when an order entry is blank, contains NUL, or is duplicated.
    pub fn new(
        fallback_enabled: bool,
        preference_order: Vec<String>,
        force_ignore: bool,
        no_failover: bool,
    ) -> Result<Self> {
        let mut seen = HashSet::new();
        let mut normalized_order = Vec::with_capacity(preference_order.len());
        for entry in preference_order {
            let normalized = normalize_nonblank("command authority preference entry", &entry)?;
            if !seen.insert(normalized.clone()) {
                bail!("command authority preference order contains duplicate '{normalized}'");
            }
            normalized_order.push(normalized);
        }
        Ok(Self {
            fallback_enabled,
            preference_order: normalized_order,
            force_ignore,
            no_failover,
        })
    }

    /// Return whether identity fallback was authorized.
    #[must_use]
    pub fn fallback_enabled(&self) -> bool {
        self.fallback_enabled
    }

    /// Return the exact configured preference order.
    #[must_use]
    pub fn preference_order(&self) -> &[String] {
        &self.preference_order
    }

    /// Return whether force-ignore authority was active.
    #[must_use]
    pub fn force_ignore(&self) -> bool {
        self.force_ignore
    }

    /// Return whether failover was explicitly prohibited.
    #[must_use]
    pub fn no_failover(&self) -> bool {
        self.no_failover
    }
}

impl<'de> Deserialize<'de> for CommandAuthorityPolicy {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawPolicy {
            fallback_enabled: bool,
            preference_order: Vec<String>,
            force_ignore: bool,
            no_failover: bool,
        }

        let raw = RawPolicy::deserialize(deserializer)?;
        Self::new(
            raw.fallback_enabled,
            raw.preference_order,
            raw.force_ignore,
            raw.no_failover,
        )
        .map_err(D::Error::custom)
    }
}

/// Source and version identity of the captured effective model catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommandAuthorityCatalogIdentity {
    source: String,
    version: String,
}

impl CommandAuthorityCatalogIdentity {
    /// Construct a catalog identity.
    ///
    /// # Errors
    /// Returns an error when either field is blank or contains NUL.
    pub fn new(source: &str, version: &str) -> Result<Self> {
        Ok(Self {
            source: normalize_nonblank("command authority catalog source", source)?,
            version: normalize_nonblank("command authority catalog version", version)?,
        })
    }

    /// Return the catalog source identity.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Return the catalog version identity.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }
}

impl<'de> Deserialize<'de> for CommandAuthorityCatalogIdentity {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawCatalogIdentity {
            source: String,
            version: String,
        }

        let raw = RawCatalogIdentity::deserialize(deserializer)?;
        Self::new(&raw.source, &raw.version).map_err(D::Error::custom)
    }
}

/// Canonical immutable execution authority captured once for an entire command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommandAuthoritySnapshot {
    source: CommandAuthoritySource,
    policy: CommandAuthorityPolicy,
    catalog: CommandAuthorityCatalogIdentity,
    ordered_admitted: Vec<AdmittedModelIdentity>,
}

impl CommandAuthoritySnapshot {
    /// Construct and validate a frozen command authority snapshot.
    ///
    /// # Errors
    /// Returns an error when no executor is admitted or one identity appears more than once.
    pub fn new(
        source: CommandAuthoritySource,
        policy: CommandAuthorityPolicy,
        catalog: CommandAuthorityCatalogIdentity,
        ordered_admitted: Vec<AdmittedModelIdentity>,
    ) -> Result<Self> {
        if ordered_admitted.is_empty() {
            bail!("command authority must admit at least one executor identity");
        }
        for (index, identity) in ordered_admitted.iter().enumerate() {
            if ordered_admitted[..index].contains(identity) {
                bail!(
                    "command authority contains duplicate admitted identity '{}/{}/{}/{}'",
                    identity.tool(),
                    identity.provider(),
                    identity.model(),
                    identity.reasoning()
                );
            }
        }
        Ok(Self {
            source,
            policy,
            catalog,
            ordered_admitted,
        })
    }

    /// Compute the domain-separated digest of the canonical field serialization.
    #[must_use]
    pub fn digest(&self) -> Sha256Digest {
        Sha256Digest::compute(&self.canonical_bytes())
    }

    /// Return the authority selection source.
    #[must_use]
    pub fn source(&self) -> &CommandAuthoritySource {
        &self.source
    }

    /// Return the frozen fallback and command-line policy.
    #[must_use]
    pub fn policy(&self) -> &CommandAuthorityPolicy {
        &self.policy
    }

    /// Return the captured catalog identity.
    #[must_use]
    pub fn catalog(&self) -> &CommandAuthorityCatalogIdentity {
        &self.catalog
    }

    /// Return admitted executor identities in exact fallback order.
    #[must_use]
    pub fn ordered_admitted(&self) -> &[AdmittedModelIdentity] {
        &self.ordered_admitted
    }

    /// Return whether the exact executor identity belongs to this authority.
    #[must_use]
    pub fn contains(&self, identity: &AdmittedModelIdentity) -> bool {
        self.ordered_admitted.contains(identity)
    }

    fn canonical_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(COMMAND_AUTHORITY_DOMAIN);
        match &self.source {
            CommandAuthoritySource::Tier { name, origin } => {
                append_field(&mut bytes, b"tier");
                append_field(&mut bytes, name.as_bytes());
                append_field(&mut bytes, origin.as_bytes());
            }
            CommandAuthoritySource::Direct { origin } => {
                append_field(&mut bytes, b"direct");
                append_field(&mut bytes, origin.as_bytes());
            }
            CommandAuthoritySource::DefaultModel { origin } => {
                append_field(&mut bytes, b"default_model");
                append_field(&mut bytes, origin.as_bytes());
            }
        }
        append_bool(&mut bytes, self.policy.fallback_enabled);
        append_count(&mut bytes, self.policy.preference_order.len());
        for preference in &self.policy.preference_order {
            append_field(&mut bytes, preference.as_bytes());
        }
        append_bool(&mut bytes, self.policy.force_ignore);
        append_bool(&mut bytes, self.policy.no_failover);
        append_field(&mut bytes, self.catalog.source.as_bytes());
        append_field(&mut bytes, self.catalog.version.as_bytes());
        append_count(&mut bytes, self.ordered_admitted.len());
        for identity in &self.ordered_admitted {
            append_field(&mut bytes, identity.tool().as_bytes());
            append_field(&mut bytes, identity.provider().as_bytes());
            append_field(&mut bytes, identity.model().as_bytes());
            append_field(&mut bytes, identity.reasoning().as_bytes());
        }
        bytes
    }
}

impl<'de> Deserialize<'de> for CommandAuthoritySnapshot {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawSnapshot {
            source: CommandAuthoritySource,
            policy: CommandAuthorityPolicy,
            catalog: CommandAuthorityCatalogIdentity,
            ordered_admitted: Vec<AdmittedModelIdentity>,
        }

        let raw = RawSnapshot::deserialize(deserializer)?;
        Self::new(raw.source, raw.policy, raw.catalog, raw.ordered_admitted)
            .map_err(D::Error::custom)
    }
}

fn append_field(bytes: &mut Vec<u8>, value: &[u8]) {
    bytes.extend_from_slice(&(value.len() as u64).to_be_bytes());
    bytes.extend_from_slice(value);
}

fn append_count(bytes: &mut Vec<u8>, count: usize) {
    bytes.extend_from_slice(&(count as u64).to_be_bytes());
}

fn append_bool(bytes: &mut Vec<u8>, value: bool) {
    bytes.push(u8::from(value));
}
