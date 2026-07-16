//! Layered, host-authoritative model evidence for clean-room reviews.

use anyhow::{Result, bail};
use serde::{Deserialize, Deserializer, Serialize, de::Error as _};

use super::{AdmittedModelIdentity, ProviderTurnExecutionId, Sha256Digest, normalize_nonblank};

/// Host-observed executable identity for a provider invocation.
///
/// This records the local tool and its observed version. It deliberately says nothing about the
/// model the remote provider actually served.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ObservedToolEvidence {
    tool: String,
    version: String,
}

impl ObservedToolEvidence {
    /// Construct bounded, host-observed executable evidence.
    pub fn new(tool: &str, version: &str) -> Result<Self> {
        Ok(Self {
            tool: bounded_evidence_text("observed tool", tool, 128)?,
            version: bounded_evidence_text("observed tool version", version, 256)?,
        })
    }

    /// Return the host-observed executable name.
    #[must_use]
    pub fn tool(&self) -> &str {
        &self.tool
    }

    /// Return the host-observed executable version.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }
}

impl<'de> Deserialize<'de> for ObservedToolEvidence {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawObservedToolEvidence {
            tool: String,
            version: String,
        }

        let raw = RawObservedToolEvidence::deserialize(deserializer)?;
        Self::new(&raw.tool, &raw.version).map_err(D::Error::custom)
    }
}

/// Source category for a model statement retained in terminal evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelEvidenceProvenance {
    /// The host admitted a model and observed the local executable only.
    HostAdmissionAndToolObservation,
    /// The provider transport additionally reported a model name; this remains unverified.
    TransportReported,
    /// An independent verifier supplied a digest-bound proof for the actual model claim.
    IndependentlyVerified,
}

/// Confidence level carried by model evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelEvidenceConfidence {
    /// The admitted identity is known, but the served model is not independently verified.
    Admitted,
    /// A transport report exists but is not a verified actual-model claim.
    TransportReported,
    /// An independent digest-bound verifier proved the actual-model claim.
    IndependentlyVerified,
}

/// Digest-bound proof required before terminal evidence may claim an actual model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IndependentlyVerifiedModel {
    model: AdmittedModelIdentity,
    verifier: String,
    proof_digest: Sha256Digest,
}

impl IndependentlyVerifiedModel {
    /// Construct an independently verifiable actual-model claim.
    pub fn new(
        model: AdmittedModelIdentity,
        verifier: &str,
        proof_digest: Sha256Digest,
    ) -> Result<Self> {
        Ok(Self {
            model,
            verifier: bounded_evidence_text("model verifier", verifier, 128)?,
            proof_digest,
        })
    }

    /// Return the only model identity that may be described as verified actual.
    #[must_use]
    pub fn model(&self) -> &AdmittedModelIdentity {
        &self.model
    }

    /// Return the independent verifier identity.
    #[must_use]
    pub fn verifier(&self) -> &str {
        &self.verifier
    }

    /// Return the immutable verification proof digest.
    #[must_use]
    pub fn proof_digest(&self) -> &Sha256Digest {
        &self.proof_digest
    }
}

impl<'de> Deserialize<'de> for IndependentlyVerifiedModel {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawIndependentlyVerifiedModel {
            model: AdmittedModelIdentity,
            verifier: String,
            proof_digest: Sha256Digest,
        }

        let raw = RawIndependentlyVerifiedModel::deserialize(deserializer)?;
        Self::new(raw.model, &raw.verifier, raw.proof_digest).map_err(D::Error::custom)
    }
}

/// Layered host-side model evidence for one reserved provider execution.
///
/// A provider cannot construct this type. In particular, a transport-reported model never
/// upgrades into a verified actual-model claim without [`IndependentlyVerifiedModel`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelEvidence {
    admitted_model: AdmittedModelIdentity,
    observed_tool: ObservedToolEvidence,
    transport_reported_model: Option<String>,
    execution_id: ProviderTurnExecutionId,
    provenance: ModelEvidenceProvenance,
    confidence: ModelEvidenceConfidence,
    independently_verified_actual_model: Option<IndependentlyVerifiedModel>,
}

impl ModelEvidence {
    /// Build host evidence from catalog admission, local executable observation, and an optional
    /// untrusted transport report.
    pub fn host_observed(
        admitted_model: AdmittedModelIdentity,
        observed_tool: ObservedToolEvidence,
        transport_reported_model: Option<&str>,
        execution_id: ProviderTurnExecutionId,
    ) -> Result<Self> {
        let transport_reported_model = transport_reported_model
            .map(|model| bounded_evidence_text("transport-reported model", model, 256))
            .transpose()?;
        let (provenance, confidence) = if transport_reported_model.is_some() {
            (
                ModelEvidenceProvenance::TransportReported,
                ModelEvidenceConfidence::TransportReported,
            )
        } else {
            (
                ModelEvidenceProvenance::HostAdmissionAndToolObservation,
                ModelEvidenceConfidence::Admitted,
            )
        };
        Ok(Self {
            admitted_model,
            observed_tool,
            transport_reported_model,
            execution_id,
            provenance,
            confidence,
            independently_verified_actual_model: None,
        })
    }

    /// Attach independently verifiable proof for the only permitted actual-model claim.
    pub fn with_independent_verification(mut self, proof: IndependentlyVerifiedModel) -> Self {
        self.provenance = ModelEvidenceProvenance::IndependentlyVerified;
        self.confidence = ModelEvidenceConfidence::IndependentlyVerified;
        self.independently_verified_actual_model = Some(proof);
        self
    }

    /// Return the catalog-admitted model identity.
    #[must_use]
    pub fn admitted_model(&self) -> &AdmittedModelIdentity {
        &self.admitted_model
    }

    /// Return the local executable observed by the host.
    #[must_use]
    pub fn observed_tool(&self) -> &ObservedToolEvidence {
        &self.observed_tool
    }

    /// Return the untrusted transport report, if one was retained.
    #[must_use]
    pub fn transport_reported_model(&self) -> Option<&str> {
        self.transport_reported_model.as_deref()
    }

    /// Return the durable provider execution identity.
    #[must_use]
    pub fn execution_id(&self) -> &ProviderTurnExecutionId {
        &self.execution_id
    }

    /// Return the source category for the strongest model statement.
    #[must_use]
    pub fn provenance(&self) -> ModelEvidenceProvenance {
        self.provenance
    }

    /// Return the confidence level for the strongest model statement.
    #[must_use]
    pub fn confidence(&self) -> ModelEvidenceConfidence {
        self.confidence
    }

    /// Return a verified actual-model claim only when an independent proof is present.
    #[must_use]
    pub fn independently_verified_actual_model(&self) -> Option<&IndependentlyVerifiedModel> {
        self.independently_verified_actual_model.as_ref()
    }

    fn validate(&self) -> Result<()> {
        let expected = match (
            self.transport_reported_model.is_some(),
            self.independently_verified_actual_model.is_some(),
        ) {
            (_, true) => (
                ModelEvidenceProvenance::IndependentlyVerified,
                ModelEvidenceConfidence::IndependentlyVerified,
            ),
            (true, false) => (
                ModelEvidenceProvenance::TransportReported,
                ModelEvidenceConfidence::TransportReported,
            ),
            (false, false) => (
                ModelEvidenceProvenance::HostAdmissionAndToolObservation,
                ModelEvidenceConfidence::Admitted,
            ),
        };
        if (self.provenance, self.confidence) != expected {
            bail!("model evidence provenance and confidence do not match its proof inputs");
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for ModelEvidence {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawModelEvidence {
            admitted_model: AdmittedModelIdentity,
            observed_tool: ObservedToolEvidence,
            transport_reported_model: Option<String>,
            execution_id: ProviderTurnExecutionId,
            provenance: ModelEvidenceProvenance,
            confidence: ModelEvidenceConfidence,
            independently_verified_actual_model: Option<IndependentlyVerifiedModel>,
        }

        let raw = RawModelEvidence::deserialize(deserializer)?;
        let evidence = Self {
            admitted_model: raw.admitted_model,
            observed_tool: raw.observed_tool,
            transport_reported_model: raw
                .transport_reported_model
                .map(|model| bounded_evidence_text("transport-reported model", &model, 256))
                .transpose()
                .map_err(D::Error::custom)?,
            execution_id: raw.execution_id,
            provenance: raw.provenance,
            confidence: raw.confidence,
            independently_verified_actual_model: raw.independently_verified_actual_model,
        };
        evidence.validate().map_err(D::Error::custom)?;
        Ok(evidence)
    }
}

fn bounded_evidence_text(label: &str, value: &str, maximum: usize) -> Result<String> {
    let normalized = normalize_nonblank(label, value)?;
    if normalized.len() > maximum || normalized.chars().any(char::is_control) {
        bail!("{label} must not contain controls or exceed {maximum} bytes");
    }
    Ok(normalized)
}
