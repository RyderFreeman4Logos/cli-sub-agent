use anyhow::{Result, bail};
use csa_session::convergence::{
    ArtifactEvidenceRef, CampaignId, CoverageCellRecord, DiscoveryRunIntent, StableFindingId,
};
use serde::Serialize;

use super::clean_room_v2::CleanRoomReviewOutput;
use super::continuation::ContinuationEvidence;
#[cfg(test)]
use super::coverage::{CoverageManifestPlan, plan_coverage_manifest};
#[cfg(test)]
use super::engine::PAGE_CANDIDATE_LIMIT;
use super::engine::{FrozenWorkspace, PhaseTimings};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DiscoveryFocus {
    Broad,
    Targeted(TargetedDiscoveryFocus),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CampaignSelection {
    LegacyReuse,
    Fresh,
    Continue(CampaignId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TargetedDiscoveryFocus {
    artifact: ArtifactEvidenceRef,
    semantic_finding_ids: Vec<StableFindingId>,
}

impl TargetedDiscoveryFocus {
    pub(crate) fn from_review(review: &CleanRoomReviewOutput) -> Result<Self> {
        if review.findings().is_empty() {
            bail!("targeted discovery requires at least one clean-room finding");
        }
        if !review.questions().is_empty() || !review.unchecked_items().is_empty() {
            bail!("targeted discovery rejects questions or unchecked items");
        }
        Ok(Self {
            artifact: review.artifact().clone(),
            semantic_finding_ids: review
                .findings()
                .iter()
                .map(|finding| finding.stable_id().clone())
                .collect(),
        })
    }

    pub(crate) fn artifact(&self) -> &ArtifactEvidenceRef {
        &self.artifact
    }

    pub(crate) fn semantic_finding_ids(&self) -> &[StableFindingId] {
        &self.semantic_finding_ids
    }
}

#[derive(Debug, Clone)]
pub(crate) struct DiscoveryRequest {
    pub(crate) frozen: FrozenWorkspace,
    pub(crate) range: String,
    pub(crate) cell: CoverageCellRecord,
    pub(crate) prior_finalized_attempt_count: u32,
    pub(crate) intent: DiscoveryRunIntent,
    pub(crate) candidate_limit: u32,
    pub(crate) continuation: ContinuationEvidence,
    pub(crate) focus: DiscoveryFocus,
    pub(crate) campaign_selection: CampaignSelection,
}

impl DiscoveryRequest {
    #[cfg(test)]
    pub(crate) fn for_test(frozen: FrozenWorkspace) -> Self {
        let epoch = frozen.epoch().expect("test epoch");
        let CoverageManifestPlan::Ready(manifest) =
            plan_coverage_manifest(&epoch, &frozen.changed_paths).expect("test manifest")
        else {
            panic!("test manifest must be bounded");
        };
        let cell = manifest.cells()[0].clone();
        Self {
            frozen,
            range: "main...HEAD".to_string(),
            cell: cell.clone(),
            prior_finalized_attempt_count: 0,
            intent: DiscoveryRunIntent::Initial,
            candidate_limit: PAGE_CANDIDATE_LIMIT,
            continuation: ContinuationEvidence::new(Vec::new(), Vec::new(), vec![cell], Vec::new()),
            focus: DiscoveryFocus::Broad,
            campaign_selection: CampaignSelection::LegacyReuse,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ObservationInput {
    pub(crate) range: String,
    pub(crate) command_authority: csa_session::convergence::CommandAuthoritySnapshot,
    pub(crate) focus: DiscoveryFocus,
    pub(crate) campaign_selection: CampaignSelection,
}

impl ObservationInput {
    pub(crate) fn new(
        range: &str,
        command_authority: csa_session::convergence::CommandAuthoritySnapshot,
    ) -> Self {
        Self {
            range: range.to_string(),
            command_authority,
            focus: DiscoveryFocus::Broad,
            campaign_selection: CampaignSelection::LegacyReuse,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ObservationSummary {
    pub(crate) kind: &'static str,
    pub(crate) campaign_id: String,
    pub(crate) epoch_id: String,
    pub(crate) base_oid: String,
    pub(crate) head_oid: String,
    pub(crate) diff_digest: String,
    pub(crate) index_clean: bool,
    pub(crate) worktree_clean: bool,
    pub(crate) coverage_cell_count: u32,
    pub(crate) provider_calls: usize,
    pub(crate) candidates: usize,
    pub(crate) phase_timings: PhaseTimings,
    pub(crate) discovery_evidence_complete: bool,
    pub(crate) review_verdict: Option<String>,
    pub(crate) merge_attestation: bool,
    pub(crate) semantic_coverage: &'static str,
}

impl ObservationSummary {
    #[cfg(test)]
    pub(crate) fn for_test(frozen: FrozenWorkspace) -> Self {
        Self {
            kind: "convergence_discovery_observation",
            campaign_id: CampaignId::generate().to_string(),
            epoch_id: frozen.epoch().expect("epoch").id().to_string(),
            base_oid: frozen.base_oid,
            head_oid: frozen.head_oid,
            diff_digest: frozen.diff_digest.to_string(),
            index_clean: frozen.index_clean,
            worktree_clean: frozen.worktree_clean,
            coverage_cell_count: 9,
            provider_calls: 0,
            candidates: 0,
            phase_timings: PhaseTimings {
                planning_ms: 0,
                execution_ms: 0,
                persistence_ms: 0,
                total_ms: 0,
            },
            discovery_evidence_complete: true,
            review_verdict: None,
            merge_attestation: false,
            semantic_coverage: "deterministic scope-by-lens manifest; every required cell saturated",
        }
    }
}
