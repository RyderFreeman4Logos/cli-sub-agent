use anyhow::{Context, Result, bail};
use csa_process::ProviderTurnCompletion;
use csa_session::convergence::{
    AdmittedModelIdentity, ArtifactEvidenceRef, CsaSessionId, SessionRelativeArtifactPath,
    Sha256Digest,
};
use serde::{Deserialize, Serialize};

use super::bundle::ProviderEvidenceIdentity;
use super::schema::{ParsedDiscoveryPage, parse_discovery_page};

const DISCOVERY_PAGE_ARTIFACT_SCHEMA_VERSION: u32 = 3;
const DISCOVERY_PAGE_ARTIFACT_KIND: &str = "convergence_discovery_observation_page";

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DiscoveryPageEnvelope {
    schema_version: u32,
    kind: String,
    provider_evidence: ProviderEvidenceIdentity,
    provider_response_raw: String,
    pub(super) parsed_page: ParsedDiscoveryPage,
}

pub(super) fn encode_discovery_page_artifact(
    raw_response: &str,
    parsed_page: &ParsedDiscoveryPage,
    provider_evidence: &ProviderEvidenceIdentity,
) -> Result<Vec<u8>> {
    serde_json::to_vec(&DiscoveryPageEnvelope {
        schema_version: DISCOVERY_PAGE_ARTIFACT_SCHEMA_VERSION,
        kind: DISCOVERY_PAGE_ARTIFACT_KIND.to_string(),
        provider_evidence: provider_evidence.clone(),
        provider_response_raw: raw_response.to_string(),
        parsed_page: parsed_page.clone(),
    })
    .context("serialize convergence discovery page artifact")
}

pub(super) fn decode_discovery_page_artifact(
    artifact_bytes: &[u8],
    expected_digest: &Sha256Digest,
    expected_provider_evidence: &ProviderEvidenceIdentity,
) -> Result<DiscoveryPageEnvelope> {
    let actual_digest = Sha256Digest::compute(artifact_bytes);
    if &actual_digest != expected_digest {
        bail!(
            "convergence discovery page artifact digest mismatch: expected {expected_digest}, got {actual_digest}"
        );
    }
    let artifact: DiscoveryPageEnvelope = serde_json::from_slice(artifact_bytes)
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
    if &artifact.provider_evidence != expected_provider_evidence {
        bail!("convergence discovery page artifact provider evidence identity mismatch");
    }
    let reparsed = parse_discovery_page(&artifact.provider_response_raw)
        .context("parse raw response embedded in convergence discovery page envelope")?;
    if reparsed != artifact.parsed_page {
        bail!("convergence discovery page envelope parsed page does not match its raw response");
    }
    Ok(artifact)
}

pub(crate) struct DiscoveryRunOutput {
    #[cfg(test)]
    pub(crate) raw_response: String,
    pub(super) page: ParsedDiscoveryPage,
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
        provider_evidence: &ProviderEvidenceIdentity,
    ) -> Result<Self> {
        let page = parse_discovery_page(&raw_response)
            .context("parse scripted convergence discovery page before artifact publication")?;
        let artifact_digest = Sha256Digest::compute(&encode_discovery_page_artifact(
            &raw_response,
            &page,
            provider_evidence,
        )?);
        Self::new_with_artifact_digest(
            raw_response,
            page,
            session_id,
            completion,
            model_identity,
            artifact_path,
            artifact_digest,
        )
    }

    pub(super) fn new_with_artifact_digest(
        #[cfg(test)] raw_response: String,
        #[cfg(not(test))] _raw_response: String,
        page: ParsedDiscoveryPage,
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
            #[cfg(test)]
            raw_response,
            page,
            completion,
            model_identity,
            artifact,
        })
    }
}
