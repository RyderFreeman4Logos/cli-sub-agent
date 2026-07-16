//! Immutable authorization evidence formed before completion performs external work.

use anyhow::{Context, Result};
use csa_config::EffectiveConvergenceCompletionPolicy;
use csa_session::convergence::{
    AdmittedModelIdentity, CampaignId, CompletionAuthorizationRecord, ConvergenceEvent,
    EpochRecord, Sha256Digest, WorkspaceLeaseIdentity,
};
use serde::Serialize;

/// Auditable authorization binding for one future completion attempt.
///
/// This record deliberately names the admitted executor rather than claiming that a provider
/// routed to a particular downstream model. Later completion adapters must still record their
/// host-observed execution evidence before attestation.
#[derive(Debug, Serialize)]
pub(crate) struct CompletionAuthorizationEvent {
    #[serde(flatten)]
    record: CompletionAuthorizationRecord,
}

impl CompletionAuthorizationEvent {
    /// Bind the clustered campaign and effective safety policy before completion work begins.
    pub(crate) fn new(
        campaign_id: CampaignId,
        epoch: &EpochRecord,
        repair_batch_count: usize,
        admitted_executor: AdmittedModelIdentity,
        policy: &EffectiveConvergenceCompletionPolicy,
        final_gate_authority_digest: Sha256Digest,
        workspace_lease: WorkspaceLeaseIdentity,
    ) -> Result<Self> {
        let repair_batch_count = u32::try_from(repair_batch_count)
            .context("repair batch count exceeds the authorization event limit")?;
        let policy_json = serde_json::to_vec(policy)
            .context("encode effective completion safety policy for authorization evidence")?;
        Ok(Self {
            record: CompletionAuthorizationRecord::new(
                campaign_id,
                epoch,
                repair_batch_count,
                admitted_executor,
                Sha256Digest::compute(&policy_json),
                final_gate_authority_digest,
                workspace_lease,
            )?,
        })
    }

    /// Return the immutable ledger event that must be appended before completion work starts.
    pub(crate) fn ledger_event(&self) -> ConvergenceEvent {
        ConvergenceEvent::CompletionAuthorizationRecorded(self.record.clone())
    }

    /// Return the recorded workspace lease identity for authorization audit.
    pub(crate) fn workspace_lease(&self) -> &WorkspaceLeaseIdentity {
        self.record.workspace_lease()
    }
}
