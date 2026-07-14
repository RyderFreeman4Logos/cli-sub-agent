use anyhow::{Context, Result, bail};
use csa_process::ProviderTurnCompletion;
use csa_session::convergence::{
    AdmittedModelIdentity, ArtifactEvidenceRef, CsaSessionId, SessionRelativeArtifactPath,
    Sha256Digest,
};
use serde::{Deserialize, Serialize};

const DISCOVERY_PAGE_ARTIFACT_SCHEMA_VERSION: u32 = 1;
const DISCOVERY_PAGE_ARTIFACT_KIND: &str = "convergence_discovery_observation_page";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DiscoveryPageArtifact {
    schema_version: u32,
    kind: String,
    provider_response_raw: String,
}

pub(super) fn encode_discovery_page_artifact(raw_response: &str) -> Result<Vec<u8>> {
    serde_json::to_vec(&DiscoveryPageArtifact {
        schema_version: DISCOVERY_PAGE_ARTIFACT_SCHEMA_VERSION,
        kind: DISCOVERY_PAGE_ARTIFACT_KIND.to_string(),
        provider_response_raw: raw_response.to_string(),
    })
    .context("serialize convergence discovery page artifact")
}

pub(super) fn decode_discovery_page_artifact(
    artifact_bytes: &[u8],
    expected_digest: &Sha256Digest,
) -> Result<String> {
    let actual_digest = Sha256Digest::compute(artifact_bytes);
    if &actual_digest != expected_digest {
        bail!(
            "convergence discovery page artifact digest mismatch: expected {expected_digest}, got {actual_digest}"
        );
    }
    let artifact: DiscoveryPageArtifact = serde_json::from_slice(artifact_bytes)
        .context("parse convergence discovery page artifact")?;
    if artifact.schema_version != DISCOVERY_PAGE_ARTIFACT_SCHEMA_VERSION {
        bail!(
            "unsupported convergence discovery page artifact schema version {}",
            artifact.schema_version
        );
    }
    if artifact.kind != DISCOVERY_PAGE_ARTIFACT_KIND {
        bail!(
            "unexpected convergence discovery page artifact kind {}",
            artifact.kind
        );
    }
    Ok(artifact.provider_response_raw)
}

pub(crate) struct DiscoveryRunOutput {
    pub(crate) raw_response: String,
    pub(crate) completion: ProviderTurnCompletion,
    pub(crate) model_identity: AdmittedModelIdentity,
    pub(crate) artifact: ArtifactEvidenceRef,
}

impl DiscoveryRunOutput {
    #[cfg(test)]
    pub(crate) fn new(
        raw_response: String,
        session_id: &str,
        completion: ProviderTurnCompletion,
        model_identity: AdmittedModelIdentity,
        artifact_path: &str,
    ) -> Result<Self> {
        let artifact_digest =
            Sha256Digest::compute(&encode_discovery_page_artifact(&raw_response)?);
        Self::new_with_artifact_digest(
            raw_response,
            session_id,
            completion,
            model_identity,
            artifact_path,
            artifact_digest,
        )
    }

    pub(crate) fn new_with_artifact_digest(
        raw_response: String,
        session_id: &str,
        completion: ProviderTurnCompletion,
        model_identity: AdmittedModelIdentity,
        artifact_path: &str,
        artifact_digest: Sha256Digest,
    ) -> Result<Self> {
        let artifact = ArtifactEvidenceRef::new(
            CsaSessionId::parse(session_id)?,
            SessionRelativeArtifactPath::new(artifact_path)?,
            artifact_digest,
        );
        Ok(Self {
            raw_response,
            completion,
            model_identity,
            artifact,
        })
    }
}
