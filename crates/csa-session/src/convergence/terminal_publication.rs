//! Preflight and publication of a complete terminal convergence attestation pair.

use thiserror::Error;

use super::store::{ConvergenceAppendError, ConvergenceLedgerStore};
use super::{
    AttestationArtifactReader, CampaignId, CleanRoomReviewRecord, CleanupConfirmation,
    ConvergenceLedgerEntry, GateEvidenceRecord, MergeAttestationRecord, TerminalExecutionBinding,
    compute_attestation_bindings, verify_terminal_artifact_pair,
};

/// Failure while preflighting or atomically publishing one terminal attestation pair.
#[derive(Debug, Error)]
pub enum FinalAttestationPublicationError {
    /// The current ledger, cleanup receipt, or immutable artifacts could not be verified.
    #[error("terminal attestation publication was rejected before ledger publication: {0:#}")]
    Preflight(#[source] anyhow::Error),
    /// The ledger transaction did not establish durable publication certainty.
    #[error(transparent)]
    Publication(#[from] ConvergenceAppendError),
}

impl FinalAttestationPublicationError {
    /// Whether recovery must reload the ledger before deciding whether the complete pair exists.
    #[must_use]
    pub fn may_have_been_published(&self) -> bool {
        matches!(
            self,
            Self::Publication(error) if error.may_have_been_published()
        )
    }
}

impl ConvergenceLedgerStore {
    /// Reload, verify, bind, and atomically publish one terminal pair.
    ///
    /// This is the only high-level terminal publication entry point. It re-reads both
    /// content-addressed artifacts from `reader`, derives bindings from the latest durable ledger
    /// generation, and then performs the terminal pair generation compare-and-swap under the
    /// ledger transaction lock.
    pub fn publish_verified_final_attestation<R: AttestationArtifactReader + ?Sized>(
        &self,
        campaign_id: CampaignId,
        gate: GateEvidenceRecord,
        final_review: CleanRoomReviewRecord,
        cleanup_confirmation: CleanupConfirmation,
        execution_binding: TerminalExecutionBinding,
        reader: &R,
    ) -> Result<Vec<ConvergenceLedgerEntry>, FinalAttestationPublicationError> {
        let ledger = self
            .load()
            .map_err(FinalAttestationPublicationError::Preflight)?;
        verify_terminal_artifact_pair(&gate, &final_review, reader)
            .map_err(FinalAttestationPublicationError::Preflight)?;
        let bindings = compute_attestation_bindings(&ledger, &campaign_id, &gate, &final_review)
            .map_err(FinalAttestationPublicationError::Preflight)?;
        let attestation = MergeAttestationRecord::new(
            &gate,
            &final_review,
            cleanup_confirmation,
            execution_binding,
            bindings,
        )
        .map_err(FinalAttestationPublicationError::Preflight)?;
        self.publish_final_attestation_at_generation(
            campaign_id,
            ledger.generation(),
            final_review,
            attestation,
        )
        .map_err(FinalAttestationPublicationError::from)
    }
}
