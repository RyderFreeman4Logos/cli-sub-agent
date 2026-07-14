use anyhow::Result;
use csa_process::ProviderTurnCompletion;
use csa_session::convergence::{
    AdmittedModelIdentity, ArtifactEvidenceRef, CsaSessionId, SessionRelativeArtifactPath,
    Sha256Digest,
};

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
        let artifact_digest = Sha256Digest::compute(raw_response.as_bytes());
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
