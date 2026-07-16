//! Fail-closed authorization derived from a complete convergence ledger.

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, bail};

use super::{
    AdmittedModelIdentity, ArtifactEvidenceRef, CampaignId, CampaignRecord,
    CandidateDispositionRecord, CandidateId, ConvergenceEvent, ConvergenceLedger,
    CoverageCellRecord, DiscoveryAttemptId, DiscoveryDirective, EpochRecord, RepairBatchId,
    RepairBatchRecord, RepairHandoffRecord, RootClusterRecord, Sha256Digest,
    next_discovery_directive,
};

/// Immutable authority for dispatching exactly one writer for each current root-cause batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsolidatedRepairAuthorization {
    campaign: CampaignRecord,
    epoch: EpochRecord,
    batches: Vec<RepairBatchRecord>,
    cluster_set_digest: Sha256Digest,
    repair_batch_set_digest: Sha256Digest,
}

impl ConsolidatedRepairAuthorization {
    /// Return the captured campaign authority used for every writer dispatch.
    #[must_use]
    pub fn campaign(&self) -> &CampaignRecord {
        &self.campaign
    }

    /// Return the latest immutable epoch authorized by the ledger.
    #[must_use]
    pub fn epoch(&self) -> &EpochRecord {
        &self.epoch
    }

    /// Return one canonical consolidated batch for every current root cluster.
    #[must_use]
    pub fn batches(&self) -> &[RepairBatchRecord] {
        &self.batches
    }

    /// Require the workspace observed immediately before dispatch to equal the authorized epoch.
    ///
    /// # Errors
    /// Returns an error when HEAD, merge-base, or diff evidence changed after authorization.
    pub fn validate_observed_epoch(&self, observed: &EpochRecord) -> Result<()> {
        if observed != &self.epoch {
            bail!("observed workspace does not match the current authorized epoch");
        }
        Ok(())
    }

    /// Bind a successful writer's actual executor and artifact evidence to one complete batch.
    ///
    /// # Errors
    /// Returns an error for an unknown batch or an executor outside the captured authority.
    pub fn handoff_for(
        &self,
        batch_id: &RepairBatchId,
        actual_executor: AdmittedModelIdentity,
        artifact: ArtifactEvidenceRef,
    ) -> Result<RepairHandoffRecord> {
        if !self.campaign.command_authority().contains(&actual_executor) {
            bail!("actual repair executor is outside the captured command authority");
        }
        let batch = self
            .batches
            .iter()
            .find(|batch| batch.id() == batch_id)
            .with_context(|| format!("repair batch {batch_id} is not authorized"))?;
        Ok(RepairHandoffRecord::new(
            self.campaign.id().clone(),
            self.epoch.id().clone(),
            batch.id().clone(),
            batch.content_digest().clone(),
            self.campaign.command_authority_digest().clone(),
            batch.candidate_set_digest().clone(),
            batch.disposition_set_digest().clone(),
            self.cluster_set_digest.clone(),
            self.repair_batch_set_digest.clone(),
            actual_executor,
            artifact,
        ))
    }
}

/// Derive explicit repair authority from the selected campaign's latest complete epoch.
///
/// # Errors
/// Returns an error for invalid, missing, stale, incomplete, ambiguous, or already-used evidence.
pub fn authorize_consolidated_repairs(
    ledger: &ConvergenceLedger,
    campaign_id: &CampaignId,
) -> Result<ConsolidatedRepairAuthorization> {
    ledger
        .validate()
        .context("repair authorization requires a valid convergence ledger")?;
    let campaign = selected_campaign(ledger, campaign_id)?;
    if campaign.command_authority().digest() != *campaign.command_authority_digest() {
        bail!("selected campaign command authority digest mismatch");
    }
    let epoch = current_epoch(ledger, campaign_id)?;
    require_complete_discovery(ledger, &campaign, &epoch)?;
    require_complete_dispositions(ledger, campaign_id, &epoch)?;

    let mut clusters = Vec::new();
    let mut batches = Vec::new();
    let mut handed_off_batches = HashSet::new();
    for entry in ledger
        .entries()
        .iter()
        .filter(|entry| entry.campaign_id() == campaign_id)
    {
        match entry.event() {
            ConvergenceEvent::RootClusterRecorded(record) if record.epoch_id() == epoch.id() => {
                clusters.push(record.clone());
            }
            ConvergenceEvent::RepairBatchRecorded(record) if record.epoch_id() == epoch.id() => {
                batches.push(record.clone());
            }
            ConvergenceEvent::RepairHandoffRecorded(record) if record.epoch_id() == epoch.id() => {
                handed_off_batches.insert(record.repair_batch_id().clone());
            }
            _ => {}
        }
    }
    if clusters.is_empty() || batches.is_empty() {
        bail!("current epoch has no complete consolidated repair batches");
    }
    if clusters.len() != batches.len() {
        bail!("current epoch does not have exactly one consolidated repair batch per root cluster");
    }
    require_authorized_batch_epoch_bindings(ledger, campaign_id, &epoch, &batches)?;
    if batches
        .iter()
        .any(|batch| handed_off_batches.contains(batch.id()))
    {
        bail!("current epoch contains an already-used repair authorization");
    }
    batches.sort_by(|left, right| {
        left.content_digest()
            .as_str()
            .cmp(right.content_digest().as_str())
    });
    Ok(ConsolidatedRepairAuthorization {
        campaign,
        epoch,
        cluster_set_digest: RootClusterRecord::set_digest(&clusters),
        repair_batch_set_digest: RepairBatchRecord::set_digest(&batches),
        batches,
    })
}

/// Recheck repair candidates at the execution-authorization boundary.
///
/// Ledger replay validates each root and batch as it is recorded. This independent lookup keeps
/// authorization closed if a future replay path becomes more permissive: every executable batch
/// must bind candidates through discovery attempts and dispositions from the selected epoch.
fn require_authorized_batch_epoch_bindings(
    ledger: &ConvergenceLedger,
    campaign_id: &CampaignId,
    epoch: &EpochRecord,
    batches: &[RepairBatchRecord],
) -> Result<()> {
    let discovery_attempts = ledger
        .entries()
        .iter()
        .filter(|entry| entry.campaign_id() == campaign_id)
        .filter_map(|entry| match entry.event() {
            ConvergenceEvent::DiscoveryAttemptRecorded(record)
                if record.epoch_id() == epoch.id() =>
            {
                Some(record.id().clone())
            }
            _ => None,
        })
        .collect::<HashSet<_>>();
    let candidates = ledger
        .entries()
        .iter()
        .filter(|entry| entry.campaign_id() == campaign_id)
        .filter_map(|entry| match entry.event() {
            ConvergenceEvent::CandidateRecorded(record)
                if discovery_attempts.contains(record.discovery_attempt_id()) =>
            {
                Some(record.id().clone())
            }
            _ => None,
        })
        .collect::<HashSet<_>>();
    let dispositions = ledger
        .entries()
        .iter()
        .filter(|entry| entry.campaign_id() == campaign_id)
        .filter_map(|entry| match entry.event() {
            ConvergenceEvent::CandidateDispositionRecorded(record)
                if record.epoch_id() == epoch.id() =>
            {
                Some(record.candidate_id().clone())
            }
            _ => None,
        })
        .collect::<HashSet<_>>();
    for batch in batches {
        for candidate_id in batch.candidate_ids() {
            if !candidates.contains(candidate_id) || !dispositions.contains(candidate_id) {
                bail!(
                    "repair batch {} contains candidate {} without discovery and disposition evidence in authorized epoch {}",
                    batch.id(),
                    candidate_id,
                    epoch.id(),
                );
            }
        }
    }
    Ok(())
}

fn selected_campaign(
    ledger: &ConvergenceLedger,
    campaign_id: &CampaignId,
) -> Result<CampaignRecord> {
    ledger
        .entries()
        .iter()
        .filter(|entry| entry.campaign_id() == campaign_id)
        .find_map(|entry| match entry.event() {
            ConvergenceEvent::CampaignStarted(record) => Some(record.clone()),
            _ => None,
        })
        .with_context(|| format!("selected campaign {campaign_id} is missing"))
}

fn current_epoch(ledger: &ConvergenceLedger, campaign_id: &CampaignId) -> Result<EpochRecord> {
    ledger
        .entries()
        .iter()
        .filter(|entry| entry.campaign_id() == campaign_id)
        .filter_map(|entry| match entry.event() {
            ConvergenceEvent::EpochOpened(record) => Some(record.clone()),
            _ => None,
        })
        .next_back()
        .with_context(|| format!("selected campaign {campaign_id} has no current epoch"))
}

fn require_complete_discovery(
    ledger: &ConvergenceLedger,
    campaign: &CampaignRecord,
    epoch: &EpochRecord,
) -> Result<()> {
    let cells = ledger
        .entries()
        .iter()
        .filter(|entry| entry.campaign_id() == campaign.id())
        .filter_map(|entry| match entry.event() {
            ConvergenceEvent::CoverageCellDefined(record) if record.epoch_id() == epoch.id() => {
                Some(record.clone())
            }
            _ => None,
        })
        .collect::<Vec<CoverageCellRecord>>();
    if cells.is_empty() {
        bail!("current epoch has no finalized discovery coverage manifest");
    }
    match next_discovery_directive(ledger, campaign, epoch, &cells)? {
        DiscoveryDirective::DiscoveryEvidenceComplete { .. } => Ok(()),
        _ => bail!("current epoch discovery evidence is incomplete"),
    }
}

fn require_complete_dispositions(
    ledger: &ConvergenceLedger,
    campaign_id: &CampaignId,
    epoch: &EpochRecord,
) -> Result<()> {
    let attempt_ids = ledger
        .entries()
        .iter()
        .filter(|entry| entry.campaign_id() == campaign_id)
        .filter_map(|entry| match entry.event() {
            ConvergenceEvent::DiscoveryAttemptRecorded(record)
                if record.epoch_id() == epoch.id() =>
            {
                Some(record.id().clone())
            }
            _ => None,
        })
        .collect::<HashSet<DiscoveryAttemptId>>();
    let candidates = ledger
        .entries()
        .iter()
        .filter(|entry| entry.campaign_id() == campaign_id)
        .filter_map(|entry| match entry.event() {
            ConvergenceEvent::CandidateRecorded(record)
                if attempt_ids.contains(record.discovery_attempt_id()) =>
            {
                Some(record.id().clone())
            }
            _ => None,
        })
        .collect::<HashSet<CandidateId>>();
    let dispositions = ledger
        .entries()
        .iter()
        .filter(|entry| entry.campaign_id() == campaign_id)
        .filter_map(|entry| match entry.event() {
            ConvergenceEvent::CandidateDispositionRecorded(record)
                if record.epoch_id() == epoch.id() =>
            {
                Some((record.candidate_id().clone(), record.clone()))
            }
            _ => None,
        })
        .collect::<HashMap<CandidateId, CandidateDispositionRecord>>();
    if candidates.is_empty() || candidates.len() != dispositions.len() {
        bail!("every current epoch candidate requires exactly one terminal disposition");
    }
    if candidates
        .iter()
        .any(|candidate| !dispositions.contains_key(candidate))
    {
        bail!("current epoch terminal disposition evidence does not cover every candidate");
    }
    Ok(())
}
