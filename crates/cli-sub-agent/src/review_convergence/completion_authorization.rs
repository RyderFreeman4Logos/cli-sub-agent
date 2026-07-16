//! Immutable authorization evidence formed before completion performs external work.

use anyhow::{Context, Result};
use csa_config::EffectiveConvergenceCompletionPolicy;
use csa_session::convergence::{AdmittedModelIdentity, CampaignId, EpochRecord, Sha256Digest};
use serde::Serialize;

/// Auditable authorization binding for one future completion attempt.
///
/// This record deliberately names the admitted executor rather than claiming that a provider
/// routed to a particular downstream model. Later completion adapters must still record their
/// host-observed execution evidence before attestation.
#[derive(Debug, Serialize)]
pub(crate) struct CompletionAuthorizationEvent {
    capability: &'static str,
    campaign_id: CampaignId,
    epoch_id: String,
    repair_batch_count: u32,
    admitted_executor: AdmittedModelIdentity,
    policy_digest: Sha256Digest,
}

impl CompletionAuthorizationEvent {
    /// Bind the clustered campaign and effective safety policy before completion work begins.
    pub(crate) fn new(
        campaign_id: CampaignId,
        epoch: &EpochRecord,
        repair_batch_count: usize,
        admitted_executor: AdmittedModelIdentity,
        policy: &EffectiveConvergenceCompletionPolicy,
    ) -> Result<Self> {
        let repair_batch_count = u32::try_from(repair_batch_count)
            .context("repair batch count exceeds the authorization event limit")?;
        let policy_json = serde_json::to_vec(policy)
            .context("encode effective completion safety policy for authorization evidence")?;
        Ok(Self {
            capability: "execute_completion",
            campaign_id,
            epoch_id: epoch.id().as_str().to_string(),
            repair_batch_count,
            admitted_executor,
            policy_digest: Sha256Digest::compute(&policy_json),
        })
    }
}
