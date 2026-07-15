use std::{collections::HashSet, fmt, path::Path, str::FromStr};

use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use csa_process::ProviderTurnCompletion;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};

use super::{
    CandidateId, CoverageCellId, CsaSessionId, DiscoveryAttemptId, EpochId,
    SemanticFindingIdentity, Sha256Digest, StableFindingId, normalize_nonblank,
};

/// Strict storage identity of the model admitted for a discovery attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AdmittedModelIdentity {
    tool: String,
    provider: String,
    model: String,
    reasoning: String,
}

impl AdmittedModelIdentity {
    /// Construct a model identity from trimmed, nonblank storage fields.
    ///
    /// # Errors
    /// Returns an error when any field is blank after trimming.
    pub fn new(tool: &str, provider: &str, model: &str, reasoning: &str) -> Result<Self> {
        Ok(Self {
            tool: normalize_nonblank("admitted model tool", tool)?,
            provider: normalize_nonblank("admitted model provider", provider)?,
            model: normalize_nonblank("admitted model name", model)?,
            reasoning: normalize_nonblank("admitted model reasoning", reasoning)?,
        })
    }
    /// Return the admitted tool transport name.
    #[must_use]
    pub fn tool(&self) -> &str {
        &self.tool
    }
    /// Return the admitted provider name.
    #[must_use]
    pub fn provider(&self) -> &str {
        &self.provider
    }
    /// Return the admitted model name.
    #[must_use]
    pub fn model(&self) -> &str {
        &self.model
    }
    /// Return the admitted reasoning configuration.
    #[must_use]
    pub fn reasoning(&self) -> &str {
        &self.reasoning
    }
}

impl<'de> Deserialize<'de> for AdmittedModelIdentity {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawAdmittedModelIdentity {
            tool: String,
            provider: String,
            model: String,
            reasoning: String,
        }

        let raw = RawAdmittedModelIdentity::deserialize(deserializer)?;
        Self::new(&raw.tool, &raw.provider, &raw.model, &raw.reasoning).map_err(D::Error::custom)
    }
}

/// Validated, normalized path relative to a CSA meta-session directory.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionRelativeArtifactPath(String);

impl SessionRelativeArtifactPath {
    /// Construct a normalized session-relative artifact path.
    ///
    /// # Errors
    /// Returns an error for blank or absolute paths, empty segments, `.` or `..`
    /// segments, surrounding whitespace, or any other non-normal component.
    pub fn new(value: &str) -> Result<Self> {
        if value.is_empty() || value.trim() != value {
            bail!("session-relative artifact path must be nonblank and normalized");
        }
        if value.contains('\0') {
            bail!("session-relative artifact path must not contain NUL bytes");
        }
        if Path::new(value).is_absolute() {
            bail!("session-relative artifact path must not be absolute");
        }
        for segment in value.split('/') {
            if segment.is_empty() {
                bail!("session-relative artifact path must not contain empty segments");
            }
            if matches!(segment, "." | "..") {
                bail!("session-relative artifact path must not contain '.' or '..' segments");
            }
        }
        if !Path::new(value)
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
        {
            bail!("session-relative artifact path must contain only normal components");
        }
        Ok(Self(value.to_string()))
    }
    /// Parse a serialized session-relative path.
    ///
    /// # Errors
    /// Returns the same errors as [`Self::new`].
    pub fn parse(value: &str) -> Result<Self> {
        Self::new(value)
    }
    /// Return the normalized relative path.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SessionRelativeArtifactPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for SessionRelativeArtifactPath {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        Self::parse(value)
    }
}

impl Serialize for SessionRelativeArtifactPath {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for SessionRelativeArtifactPath {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(D::Error::custom)
    }
}

/// Session-locatable, digest-bound reference to one persisted discovery evidence artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactEvidenceRef {
    csa_session_id: CsaSessionId,
    path: SessionRelativeArtifactPath,
    digest: Sha256Digest,
}

impl ArtifactEvidenceRef {
    /// Construct a self-contained artifact evidence reference.
    #[must_use]
    pub fn new(
        csa_session_id: CsaSessionId,
        path: SessionRelativeArtifactPath,
        digest: Sha256Digest,
    ) -> Self {
        Self {
            csa_session_id,
            path,
            digest,
        }
    }
    /// Return the CSA meta-session containing the referenced artifact.
    #[must_use]
    pub fn csa_session_id(&self) -> &CsaSessionId {
        &self.csa_session_id
    }
    /// Return the validated session-relative artifact path.
    #[must_use]
    pub fn path(&self) -> &SessionRelativeArtifactPath {
        &self.path
    }
    /// Return the expected SHA-256 digest of the artifact bytes.
    #[must_use]
    pub fn digest(&self) -> &Sha256Digest {
        &self.digest
    }
}

/// Immutable evidence for one completed or interrupted discovery attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiscoveryAttemptRecord {
    id: DiscoveryAttemptId,
    epoch_id: EpochId,
    coverage_cell_id: CoverageCellId,
    completed_at: DateTime<Utc>,
    completion: ProviderTurnCompletion,
    model_identity: AdmittedModelIdentity,
    artifact: ArtifactEvidenceRef,
    candidate_limit: u32,
    reported_candidate_count: u32,
    more_candidates_possible: bool,
    unscanned_items: Vec<String>,
}

impl DiscoveryAttemptRecord {
    /// Construct and validate immutable evidence for one discovery attempt.
    ///
    /// # Errors
    /// Returns an error when the candidate limit is zero, the reported count exceeds
    /// the limit, or unscanned items are blank or duplicated after normalization.
    #[expect(
        clippy::too_many_arguments,
        reason = "all persisted evidence fields are required"
    )]
    pub fn new(
        id: DiscoveryAttemptId,
        epoch_id: EpochId,
        coverage_cell_id: CoverageCellId,
        completed_at: DateTime<Utc>,
        completion: ProviderTurnCompletion,
        model_identity: AdmittedModelIdentity,
        artifact: ArtifactEvidenceRef,
        candidate_limit: u32,
        reported_candidate_count: u32,
        more_candidates_possible: bool,
        unscanned_items: Vec<String>,
    ) -> Result<Self> {
        if candidate_limit == 0 {
            bail!("discovery attempt candidate limit must be greater than zero");
        }
        if reported_candidate_count > candidate_limit {
            bail!(
                "discovery attempt reported candidate count {reported_candidate_count} exceeds limit {candidate_limit}"
            );
        }

        let mut normalized_items = Vec::with_capacity(unscanned_items.len());
        let mut unique_items = HashSet::with_capacity(unscanned_items.len());
        for item in unscanned_items {
            let normalized = normalize_nonblank("discovery attempt unscanned item", &item)?;
            if !unique_items.insert(normalized.clone()) {
                bail!("duplicate discovery attempt unscanned item '{normalized}'");
            }
            normalized_items.push(normalized);
        }

        Ok(Self {
            id,
            epoch_id,
            coverage_cell_id,
            completed_at,
            completion,
            model_identity,
            artifact,
            candidate_limit,
            reported_candidate_count,
            more_candidates_possible,
            unscanned_items: normalized_items,
        })
    }
    /// Return the discovery attempt identifier.
    #[must_use]
    pub fn id(&self) -> &DiscoveryAttemptId {
        &self.id
    }
    /// Return the opened epoch targeted by the attempt.
    #[must_use]
    pub fn epoch_id(&self) -> &EpochId {
        &self.epoch_id
    }
    /// Return the defined coverage cell targeted by the attempt.
    #[must_use]
    pub fn coverage_cell_id(&self) -> &CoverageCellId {
        &self.coverage_cell_id
    }
    /// Return the CSA meta-session containing this attempt's evidence.
    #[must_use]
    pub fn csa_session_id(&self) -> &CsaSessionId {
        self.artifact.csa_session_id()
    }
    /// Return when the attempt completed or stopped producing evidence.
    #[must_use]
    pub fn completed_at(&self) -> &DateTime<Utc> {
        &self.completed_at
    }
    /// Return the explicit provider-turn completion classification.
    #[must_use]
    pub fn completion(&self) -> ProviderTurnCompletion {
        self.completion
    }
    /// Return the admitted model storage identity.
    #[must_use]
    pub fn model_identity(&self) -> &AdmittedModelIdentity {
        &self.model_identity
    }
    /// Return the required attempt artifact evidence.
    #[must_use]
    pub fn artifact(&self) -> &ArtifactEvidenceRef {
        &self.artifact
    }
    /// Return the maximum candidates requested from the attempt.
    #[must_use]
    pub fn candidate_limit(&self) -> u32 {
        self.candidate_limit
    }
    /// Return the number of candidate observations reported by the attempt.
    #[must_use]
    pub fn reported_candidate_count(&self) -> u32 {
        self.reported_candidate_count
    }
    /// Return whether the attempt reported that additional candidates may exist.
    #[must_use]
    pub fn more_candidates_possible(&self) -> bool {
        self.more_candidates_possible
    }
    /// Return normalized, unique items the attempt explicitly did not scan.
    #[must_use]
    pub fn unscanned_items(&self) -> &[String] {
        &self.unscanned_items
    }
    /// Revalidate all non-type-level attempt invariants.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::new`].
    pub fn validate(&self) -> Result<()> {
        Self::new(
            self.id.clone(),
            self.epoch_id.clone(),
            self.coverage_cell_id.clone(),
            self.completed_at,
            self.completion,
            self.model_identity.clone(),
            self.artifact.clone(),
            self.candidate_limit,
            self.reported_candidate_count,
            self.more_candidates_possible,
            self.unscanned_items.clone(),
        )?;
        Ok(())
    }
}

impl<'de> Deserialize<'de> for DiscoveryAttemptRecord {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawDiscoveryAttemptRecord {
            id: DiscoveryAttemptId,
            epoch_id: EpochId,
            coverage_cell_id: CoverageCellId,
            completed_at: DateTime<Utc>,
            completion: ProviderTurnCompletion,
            model_identity: AdmittedModelIdentity,
            artifact: ArtifactEvidenceRef,
            candidate_limit: u32,
            reported_candidate_count: u32,
            more_candidates_possible: bool,
            unscanned_items: Vec<String>,
        }

        let raw = RawDiscoveryAttemptRecord::deserialize(deserializer)?;
        Self::new(
            raw.id,
            raw.epoch_id,
            raw.coverage_cell_id,
            raw.completed_at,
            raw.completion,
            raw.model_identity,
            raw.artifact,
            raw.candidate_limit,
            raw.reported_candidate_count,
            raw.more_candidates_possible,
            raw.unscanned_items,
        )
        .map_err(D::Error::custom)
    }
}

/// Immutable observation of one semantic finding candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CandidateRecord {
    id: CandidateId,
    discovery_attempt_id: DiscoveryAttemptId,
    semantic_identity: SemanticFindingIdentity,
    stable_finding_id: StableFindingId,
    artifact: ArtifactEvidenceRef,
}

impl CandidateRecord {
    /// Construct a candidate observation and compute its stable finding identity.
    #[must_use]
    pub fn new(
        id: CandidateId,
        discovery_attempt_id: DiscoveryAttemptId,
        semantic_identity: SemanticFindingIdentity,
        artifact: ArtifactEvidenceRef,
    ) -> Self {
        let stable_finding_id = StableFindingId::compute(&semantic_identity);
        Self {
            id,
            discovery_attempt_id,
            semantic_identity,
            stable_finding_id,
            artifact,
        }
    }
    /// Return this candidate observation identifier.
    #[must_use]
    pub fn id(&self) -> &CandidateId {
        &self.id
    }
    /// Return the discovery attempt that reported this observation.
    #[must_use]
    pub fn discovery_attempt_id(&self) -> &DiscoveryAttemptId {
        &self.discovery_attempt_id
    }
    /// Return the canonical semantic finding identity.
    #[must_use]
    pub fn semantic_identity(&self) -> &SemanticFindingIdentity {
        &self.semantic_identity
    }
    /// Return the stored stable finding identity.
    #[must_use]
    pub fn stable_finding_id(&self) -> &StableFindingId {
        &self.stable_finding_id
    }
    /// Return the candidate-specific artifact evidence.
    #[must_use]
    pub fn artifact(&self) -> &ArtifactEvidenceRef {
        &self.artifact
    }
    /// Recompute the stable finding identity from semantic fields.
    #[must_use]
    pub fn recompute_stable_finding_id(&self) -> StableFindingId {
        StableFindingId::compute(&self.semantic_identity)
    }
    /// Verify that the stored stable finding identity was not tampered with.
    ///
    /// # Errors
    ///
    /// Returns an error when the stored stable ID differs from the recomputed ID.
    pub fn validate(&self) -> Result<()> {
        let expected = self.recompute_stable_finding_id();
        if self.stable_finding_id != expected {
            bail!(
                "candidate {} stable finding id mismatch: stored {}, recomputed {}",
                self.id,
                self.stable_finding_id,
                expected
            );
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for CandidateRecord {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawCandidateRecord {
            id: CandidateId,
            discovery_attempt_id: DiscoveryAttemptId,
            semantic_identity: SemanticFindingIdentity,
            stable_finding_id: StableFindingId,
            artifact: ArtifactEvidenceRef,
        }

        let raw = RawCandidateRecord::deserialize(deserializer)?;
        let record = Self {
            id: raw.id,
            discovery_attempt_id: raw.discovery_attempt_id,
            semantic_identity: raw.semantic_identity,
            stable_finding_id: raw.stable_finding_id,
            artifact: raw.artifact,
        };
        record.validate().map_err(D::Error::custom)?;
        Ok(record)
    }
}

/// Terminal disposition assigned to one candidate observation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CandidateDisposition {
    /// The candidate was verified as an actionable finding.
    Verified,
    /// The candidate duplicates an already-recorded canonical observation.
    Duplicate {
        /// Prior canonical observation with the same stable finding identity.
        canonical_candidate_id: CandidateId,
    },
    /// The candidate was rejected and the disposition artifact explains why.
    RejectedWithEvidence,
    /// The candidate requires a contract or documentation decision.
    NeedsContractOrDocumentation,
    /// The candidate is pre-existing and outside the frozen diff scope.
    PreExistingOutsideDiffScope,
    /// The candidate was replaced by another prior observation.
    Superseded {
        /// Prior replacement observation.
        replacement_candidate_id: CandidateId,
    },
}

/// Planning requirement assigned to a defined coverage cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum CoverageRequirement {
    /// Discovery evidence is required for the coverage cell.
    Required,
    /// The coverage cell does not apply for the recorded machine-readable reason.
    NotApplicable,
}

/// Immutable planning disposition for one defined coverage cell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CoverageDispositionRecord {
    coverage_cell_id: CoverageCellId,
    requirement: CoverageRequirement,
    reason: String,
    rationale: String,
}

impl CoverageDispositionRecord {
    /// Construct a strict coverage planning disposition.
    ///
    /// # Errors
    ///
    /// Returns an error unless `reason` is canonical lowercase snake case and
    /// `rationale` is nonblank after trimming.
    pub fn new(
        coverage_cell_id: CoverageCellId,
        requirement: CoverageRequirement,
        reason: &str,
        rationale: &str,
    ) -> Result<Self> {
        validate_machine_reason(reason)?;
        Ok(Self {
            coverage_cell_id,
            requirement,
            reason: reason.to_string(),
            rationale: normalize_nonblank("coverage disposition rationale", rationale)?,
        })
    }
    /// Return the coverage cell receiving this planning disposition.
    #[must_use]
    pub fn coverage_cell_id(&self) -> &CoverageCellId {
        &self.coverage_cell_id
    }
    /// Return whether the coverage cell is required or not applicable.
    #[must_use]
    pub fn requirement(&self) -> CoverageRequirement {
        self.requirement
    }
    /// Return the canonical machine-readable reason.
    #[must_use]
    pub fn reason(&self) -> &str {
        &self.reason
    }
    /// Return the nonblank human-readable rationale.
    #[must_use]
    pub fn rationale(&self) -> &str {
        &self.rationale
    }
    /// Revalidate machine-readable reason and rationale invariants.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::new`].
    pub fn validate(&self) -> Result<()> {
        Self::new(
            self.coverage_cell_id.clone(),
            self.requirement,
            &self.reason,
            &self.rationale,
        )?;
        Ok(())
    }
}

impl<'de> Deserialize<'de> for CoverageDispositionRecord {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawCoverageDispositionRecord {
            coverage_cell_id: CoverageCellId,
            requirement: CoverageRequirement,
            reason: String,
            rationale: String,
        }

        let raw = RawCoverageDispositionRecord::deserialize(deserializer)?;
        Self::new(
            raw.coverage_cell_id,
            raw.requirement,
            &raw.reason,
            &raw.rationale,
        )
        .map_err(D::Error::custom)
    }
}

fn validate_machine_reason(reason: &str) -> Result<()> {
    if reason.is_empty() || reason.trim() != reason {
        bail!("coverage disposition reason must be nonblank and normalized");
    }
    if !reason
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        || reason.starts_with('_')
        || reason.ends_with('_')
        || reason.contains("__")
    {
        bail!("coverage disposition reason must be canonical lowercase snake case");
    }
    Ok(())
}
