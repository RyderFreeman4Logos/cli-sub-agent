use anyhow::Result;
use serde::{Deserialize, Deserializer, Serialize, de::Error as _};

use super::{
    AdmittedModelIdentity, ArtifactEvidenceRef, CandidateDisposition, CandidateId, EpochId,
    normalize_nonblank,
};

/// Whether a verifier was independent of the discovery executor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VerificationIndependence {
    /// The verifier used a different admitted executor than discovery.
    Heterogeneous,
    /// No heterogeneous admitted executor was available for this verification.
    Degraded { reason: String },
}

impl VerificationIndependence {
    /// Record the explicit reason that independent verification was unavailable.
    ///
    /// # Errors
    /// Returns an error when `reason` is blank or contains NUL.
    pub fn degraded(reason: &str) -> Result<Self> {
        Ok(Self::Degraded {
            reason: normalize_nonblank("verification degraded-independence reason", reason)?,
        })
    }
}

impl<'de> Deserialize<'de> for VerificationIndependence {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
        enum RawVerificationIndependence {
            Heterogeneous,
            Degraded { reason: String },
        }

        match RawVerificationIndependence::deserialize(deserializer)? {
            RawVerificationIndependence::Heterogeneous => Ok(Self::Heterogeneous),
            RawVerificationIndependence::Degraded { reason } => Self::degraded(&reason),
        }
        .map_err(D::Error::custom)
    }
}

/// Immutable verifier evidence bound to one frozen candidate epoch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateVerificationEvidence {
    epoch_id: EpochId,
    actual_executor: AdmittedModelIdentity,
    independence: VerificationIndependence,
    artifact: ArtifactEvidenceRef,
}

impl CandidateVerificationEvidence {
    /// Construct the complete verifier evidence for a terminal disposition.
    #[must_use]
    pub fn new(
        epoch_id: EpochId,
        actual_executor: AdmittedModelIdentity,
        independence: VerificationIndependence,
        artifact: ArtifactEvidenceRef,
    ) -> Self {
        Self {
            epoch_id,
            actual_executor,
            independence,
            artifact,
        }
    }

    /// Return the immutable epoch observed by the verifier.
    #[must_use]
    pub fn epoch_id(&self) -> &EpochId {
        &self.epoch_id
    }

    /// Return the executor that actually produced the verifier artifact.
    #[must_use]
    pub fn actual_executor(&self) -> &AdmittedModelIdentity {
        &self.actual_executor
    }

    /// Return the recorded verifier-independence classification.
    #[must_use]
    pub fn independence(&self) -> &VerificationIndependence {
        &self.independence
    }

    /// Return the digest-bound verifier artifact reference.
    #[must_use]
    pub fn artifact(&self) -> &ArtifactEvidenceRef {
        &self.artifact
    }
}

/// Immutable terminal disposition evidence for one candidate observation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateDispositionRecord {
    candidate_id: CandidateId,
    disposition: CandidateDisposition,
    verification: CandidateVerificationEvidence,
}

impl CandidateDispositionRecord {
    /// Construct terminal disposition evidence for a candidate.
    #[must_use]
    pub fn new(
        candidate_id: CandidateId,
        disposition: CandidateDisposition,
        verification: CandidateVerificationEvidence,
    ) -> Self {
        Self {
            candidate_id,
            disposition,
            verification,
        }
    }

    /// Return the candidate receiving this terminal disposition.
    #[must_use]
    pub fn candidate_id(&self) -> &CandidateId {
        &self.candidate_id
    }

    /// Return the terminal disposition and any candidate relation it carries.
    #[must_use]
    pub fn disposition(&self) -> &CandidateDisposition {
        &self.disposition
    }

    /// Return the immutable verifier evidence bound to this disposition.
    #[must_use]
    pub fn verification(&self) -> &CandidateVerificationEvidence {
        &self.verification
    }

    /// Return the immutable epoch observed by the verifier.
    #[must_use]
    pub fn epoch_id(&self) -> &EpochId {
        self.verification.epoch_id()
    }

    /// Return the executor that actually produced the verifier artifact.
    #[must_use]
    pub fn actual_executor(&self) -> &AdmittedModelIdentity {
        self.verification.actual_executor()
    }

    /// Return the verifier-independence classification.
    #[must_use]
    pub fn independence(&self) -> &VerificationIndependence {
        self.verification.independence()
    }

    /// Return the required disposition artifact evidence.
    #[must_use]
    pub fn artifact(&self) -> &ArtifactEvidenceRef {
        self.verification.artifact()
    }

    /// Revalidate the terminal evidence fields that are not enforced by Rust types.
    ///
    /// # Errors
    /// Returns an error when degraded-independence evidence is blank or malformed.
    pub fn validate(&self) -> Result<()> {
        if let VerificationIndependence::Degraded { reason } = self.independence() {
            VerificationIndependence::degraded(reason)?;
        }
        Ok(())
    }
}
