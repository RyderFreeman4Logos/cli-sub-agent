//! Immutable completion authorization bindings recorded before external work begins.

use std::path::{Component, Path, PathBuf};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use ulid::Ulid;

use super::{AdmittedModelIdentity, CampaignId, EpochId, EpochRecord, Sha256Digest};

/// Immutable filesystem identity bound to one owned completion workspace lease.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceLeaseIdentity {
    campaign_id: CampaignId,
    epoch: EpochRecord,
    generation: u64,
    workspace_root: PathBuf,
    device: u64,
    inode: u64,
    nonce: String,
}

impl WorkspaceLeaseIdentity {
    /// Construct a lease identity from host-observed workspace metadata.
    ///
    /// # Errors
    ///
    /// Returns an error when the generation, root path, device/inode pair, or nonce is invalid.
    pub fn new(
        campaign_id: CampaignId,
        epoch: EpochRecord,
        generation: u64,
        workspace_root: PathBuf,
        device: u64,
        inode: u64,
        nonce: String,
    ) -> Result<Self> {
        epoch.validate()?;
        if generation == 0 {
            bail!("workspace lease generation must be nonzero");
        }
        validate_workspace_root(&workspace_root)?;
        if device == 0 || inode == 0 {
            bail!("workspace lease device and inode must be nonzero");
        }
        let parsed_nonce = Ulid::from_string(&nonce)
            .map_err(|error| anyhow::anyhow!("invalid workspace lease nonce '{nonce}': {error}"))?;
        Ok(Self {
            campaign_id,
            epoch,
            generation,
            workspace_root,
            device,
            inode,
            nonce: parsed_nonce.to_string(),
        })
    }

    /// Return the campaign that owns this lease.
    #[must_use]
    pub fn campaign_id(&self) -> &CampaignId {
        &self.campaign_id
    }

    /// Return the exact epoch observed when the lease was acquired.
    #[must_use]
    pub fn epoch(&self) -> &EpochRecord {
        &self.epoch
    }

    /// Return the monotonically assigned completion generation.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Return the canonical, direct workspace root.
    #[must_use]
    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    /// Return the observed filesystem device number.
    #[must_use]
    pub fn device(&self) -> u64 {
        self.device
    }

    /// Return the observed directory inode number.
    #[must_use]
    pub fn inode(&self) -> u64 {
        self.inode
    }

    /// Return the unguessable lease nonce.
    #[must_use]
    pub fn nonce(&self) -> &str {
        &self.nonce
    }
}

/// Ledger record authorizing one completion attempt against an owned workspace lease.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompletionAuthorizationRecord {
    capability: String,
    campaign_id: CampaignId,
    epoch_id: EpochId,
    repair_batch_count: u32,
    admitted_executor: AdmittedModelIdentity,
    policy_digest: Sha256Digest,
    workspace_lease: WorkspaceLeaseIdentity,
}

impl CompletionAuthorizationRecord {
    /// Construct an immutable completion authorization record.
    ///
    /// # Errors
    ///
    /// Returns an error when the record does not bind the same campaign and exact epoch as the
    /// workspace lease.
    pub fn new(
        campaign_id: CampaignId,
        epoch: &EpochRecord,
        repair_batch_count: u32,
        admitted_executor: AdmittedModelIdentity,
        policy_digest: Sha256Digest,
        workspace_lease: WorkspaceLeaseIdentity,
    ) -> Result<Self> {
        if workspace_lease.campaign_id() != &campaign_id {
            bail!("completion authorization campaign does not match workspace lease campaign");
        }
        if workspace_lease.epoch() != epoch {
            bail!("completion authorization epoch does not match workspace lease epoch");
        }
        Ok(Self {
            capability: "execute_completion".to_string(),
            campaign_id,
            epoch_id: epoch.id().clone(),
            repair_batch_count,
            admitted_executor,
            policy_digest,
            workspace_lease,
        })
    }

    /// Verify the record's internal campaign and epoch bindings.
    ///
    /// # Errors
    ///
    /// Returns an error when deserialized evidence is internally inconsistent.
    pub fn validate(&self) -> Result<()> {
        if self.capability != "execute_completion" {
            bail!("unsupported completion authorization capability");
        }
        if self.workspace_lease.campaign_id() != &self.campaign_id {
            bail!("completion authorization campaign does not match workspace lease campaign");
        }
        if self.workspace_lease.epoch().id() != &self.epoch_id {
            bail!("completion authorization epoch does not match workspace lease epoch");
        }
        Ok(())
    }

    /// Return the authorized campaign.
    #[must_use]
    pub fn campaign_id(&self) -> &CampaignId {
        &self.campaign_id
    }

    /// Return the exact authorized epoch identifier.
    #[must_use]
    pub fn epoch_id(&self) -> &EpochId {
        &self.epoch_id
    }

    /// Return the number of repair batches authorized for this generation.
    #[must_use]
    pub fn repair_batch_count(&self) -> u32 {
        self.repair_batch_count
    }

    /// Return the catalog-admitted executor identity.
    #[must_use]
    pub fn admitted_executor(&self) -> &AdmittedModelIdentity {
        &self.admitted_executor
    }

    /// Return the effective completion-policy digest.
    #[must_use]
    pub fn policy_digest(&self) -> &Sha256Digest {
        &self.policy_digest
    }

    /// Return the owned workspace lease identity.
    #[must_use]
    pub fn workspace_lease(&self) -> &WorkspaceLeaseIdentity {
        &self.workspace_lease
    }
}

fn validate_workspace_root(root: &Path) -> Result<()> {
    if !root.is_absolute() {
        bail!("workspace lease root must be absolute: {}", root.display());
    }
    if root.as_os_str().is_empty() || root.to_str().is_none() {
        bail!(
            "workspace lease root must be nonempty valid UTF-8: {}",
            root.display()
        );
    }
    if root.components().any(|component| {
        matches!(
            component,
            Component::CurDir | Component::ParentDir | Component::Prefix(_)
        )
    }) {
        bail!(
            "workspace lease root must be normalized: {}",
            root.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::convergence::{ConvergenceEvent, ConvergenceLedger, GitObjectId, Sha256Digest};

    fn epoch() -> EpochRecord {
        EpochRecord::new(
            GitObjectId::parse(&"a".repeat(40)).expect("base"),
            GitObjectId::parse(&"b".repeat(40)).expect("head"),
            Sha256Digest::compute(b"diff"),
        )
    }

    #[test]
    fn authorization_rejects_a_workspace_lease_for_a_different_epoch() {
        let campaign = CampaignId::generate();
        let lease = WorkspaceLeaseIdentity::new(
            campaign.clone(),
            epoch(),
            1,
            PathBuf::from("/workspace"),
            1,
            2,
            Ulid::new().to_string(),
        )
        .expect("lease identity");
        let other_epoch = EpochRecord::new(
            GitObjectId::parse(&"c".repeat(40)).expect("base"),
            GitObjectId::parse(&"d".repeat(40)).expect("head"),
            Sha256Digest::compute(b"different diff"),
        );

        let error = CompletionAuthorizationRecord::new(
            campaign,
            &other_epoch,
            0,
            AdmittedModelIdentity::new("codex", "openai", "gpt-5.6", "xhigh").expect("model"),
            Sha256Digest::compute(b"policy"),
            lease,
        )
        .expect_err("epoch mismatch must be rejected");

        assert!(error.to_string().contains("epoch"));
    }

    #[test]
    fn ledger_persists_the_complete_workspace_lease_identity() {
        let campaign = CampaignId::generate();
        let epoch = epoch();
        let lease = WorkspaceLeaseIdentity::new(
            campaign.clone(),
            epoch.clone(),
            3,
            PathBuf::from("/workspace"),
            10,
            20,
            Ulid::new().to_string(),
        )
        .expect("lease identity");
        let record = CompletionAuthorizationRecord::new(
            campaign.clone(),
            &epoch,
            2,
            AdmittedModelIdentity::new("codex", "openai", "gpt-5.6", "xhigh").expect("model"),
            Sha256Digest::compute(b"policy"),
            lease.clone(),
        )
        .expect("authorization record");
        let mut ledger = ConvergenceLedger::empty();
        ledger
            .append_batch(
                campaign.clone(),
                vec![
                    ConvergenceEvent::CampaignStarted(
                        crate::convergence::CampaignRecord::for_test(
                            campaign.clone(),
                            chrono::Utc::now(),
                            None,
                        ),
                    ),
                    ConvergenceEvent::EpochOpened(epoch),
                    ConvergenceEvent::CompletionAuthorizationRecorded(record),
                ],
            )
            .expect("append authorization");

        let recorded = ledger.entries().last().expect("authorization entry");
        let ConvergenceEvent::CompletionAuthorizationRecorded(record) = recorded.event() else {
            panic!("expected completion authorization event");
        };
        assert_eq!(record.workspace_lease(), &lease);
        ledger.validate().expect("valid ledger");
    }
}
