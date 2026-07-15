use std::error::Error;
use std::fmt;

use csa_session::convergence::{
    AdmittedModelIdentity, ArtifactEvidenceRef, CampaignId, CandidateId, EpochId, EpochRecord,
    RepairBatchId, RootClusterId,
};

use super::discovery_contract::{CampaignSelection, CleanRoomReviewOutput, DiscoveryFocus};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CompletionBudget {
    pub(super) max_cycles: u32,
    pub(super) max_provider_actions: u32,
}

impl CompletionBudget {
    pub(crate) fn new(max_cycles: u32, max_provider_actions: u32) -> Result<Self, CompletionError> {
        if max_cycles == 0
            || max_provider_actions == 0
            || max_cycles > 10_000
            || max_provider_actions > 1_000
        {
            return Err(CompletionError::InvalidBudget);
        }
        Ok(Self {
            max_cycles,
            max_provider_actions,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CompletionPhase {
    Start,
    Discover,
    VerifyAndCluster,
    RunAuthorizedRepairs,
    RunFinalGates,
    RunFreshCleanRoom,
    PublishFinalPair,
    Attested,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AuthorizedRepairBatch {
    pub(super) root_cluster_id: RootClusterId,
    pub(super) repair_batch_id: RepairBatchId,
}

impl AuthorizedRepairBatch {
    pub(crate) fn new(root_cluster_id: RootClusterId, repair_batch_id: RepairBatchId) -> Self {
        Self {
            root_cluster_id,
            repair_batch_id,
        }
    }

    pub(crate) fn root_cluster_id(&self) -> &RootClusterId {
        &self.root_cluster_id
    }

    pub(crate) fn repair_batch_id(&self) -> &RepairBatchId {
        &self.repair_batch_id
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompletionState {
    pub(super) phase: CompletionPhase,
    pub(super) budget: CompletionBudget,
    pub(super) cycles: u32,
    pub(super) provider_actions: u32,
    pub(super) campaign_id: Option<CampaignId>,
    pub(super) epoch: Option<EpochRecord>,
    pub(super) discovery_focus: Option<DiscoveryFocus>,
    pub(super) campaign_selection: Option<CampaignSelection>,
    pub(super) pending_candidates: Vec<CandidateId>,
    pub(super) repair_batches: Vec<AuthorizedRepairBatch>,
    pub(super) gate_artifact: Option<ArtifactEvidenceRef>,
    pub(super) clean_room: Option<CleanRoomReviewOutput>,
}

impl CompletionState {
    pub(crate) fn new(budget: CompletionBudget) -> Self {
        Self {
            phase: CompletionPhase::Start,
            budget,
            cycles: 0,
            provider_actions: 0,
            campaign_id: None,
            epoch: None,
            discovery_focus: None,
            campaign_selection: None,
            pending_candidates: Vec::new(),
            repair_batches: Vec::new(),
            gate_artifact: None,
            clean_room: None,
        }
    }

    pub(crate) fn phase(&self) -> CompletionPhase {
        self.phase
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CompletionAction {
    Discover {
        focus: DiscoveryFocus,
        selection: CampaignSelection,
        epoch: EpochRecord,
    },
    VerifyAndCluster {
        campaign_id: CampaignId,
        epoch: EpochRecord,
        candidates: Vec<CandidateId>,
    },
    RunAuthorizedRepairs {
        campaign_id: CampaignId,
        epoch: EpochRecord,
        batches: Vec<AuthorizedRepairBatch>,
    },
    RunFinalGates {
        campaign_id: CampaignId,
        epoch: EpochRecord,
    },
    RunFreshCleanRoom {
        campaign_id: CampaignId,
        epoch: EpochRecord,
        gate_artifact: ArtifactEvidenceRef,
    },
    PublishFinalPair {
        campaign_id: CampaignId,
        epoch: EpochRecord,
        gate_artifact: ArtifactEvidenceRef,
        clean_room: Box<CleanRoomReviewOutput>,
    },
}

impl CompletionAction {
    pub(super) fn consumes_provider_action(&self) -> bool {
        matches!(
            self,
            Self::Discover { .. }
                | Self::VerifyAndCluster { .. }
                | Self::RunAuthorizedRepairs { .. }
                | Self::RunFreshCleanRoom { .. }
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CompletionEvent {
    Started {
        epoch: EpochRecord,
        selection: CampaignSelection,
    },
    DiscoveryCompleted {
        focus: DiscoveryFocus,
        selection: CampaignSelection,
        campaign_id: CampaignId,
        epoch: EpochRecord,
        candidates: Vec<CandidateId>,
    },
    ClustersReady {
        campaign_id: CampaignId,
        epoch_id: EpochId,
        verified_candidates: Vec<CandidateId>,
        root_clusters: Vec<RootClusterId>,
        repair_batches: Vec<AuthorizedRepairBatch>,
    },
    RepairsCompleted {
        campaign_id: CampaignId,
        previous_epoch_id: EpochId,
        completed_batches: Vec<RepairBatchId>,
        new_epoch: EpochRecord,
    },
    FinalGatesPassed {
        campaign_id: CampaignId,
        epoch_id: EpochId,
        artifact: ArtifactEvidenceRef,
    },
    CleanRoomCompleted {
        campaign_id: CampaignId,
        epoch_id: EpochId,
        output: CleanRoomReviewOutput,
    },
    FinalPairPublished {
        campaign_id: CampaignId,
        epoch_id: EpochId,
        gate_artifact: ArtifactEvidenceRef,
        review_artifact: ArtifactEvidenceRef,
        model_identity: AdmittedModelIdentity,
    },
    DriftDetected,
    CleanupUncertain,
    ProviderUnavailable,
    IncompleteProviderOutput,
    MaxRoundsReached,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CompletionOutcome {
    Attested {
        campaign_id: CampaignId,
        epoch: EpochRecord,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompletionTransition {
    pub(crate) state: CompletionState,
    pub(crate) action: Option<CompletionAction>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CompletionError {
    InvalidBudget,
    InvalidTransition(&'static str),
    IdentityMismatch,
    DuplicateIdentity,
    CardinalityMismatch,
    EpochDidNotChange,
    BudgetExhausted,
    BlockedCleanRoom,
    DriftDetected,
    CleanupUncertain,
    ProviderUnavailable,
    IncompleteProviderOutput,
    MaxRoundsReached,
    Port(String),
}

impl fmt::Display for CompletionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBudget => formatter.write_str("completion budget is invalid"),
            Self::InvalidTransition(message) => {
                write!(formatter, "invalid completion transition: {message}")
            }
            Self::IdentityMismatch => formatter.write_str("completion event identity mismatch"),
            Self::DuplicateIdentity => {
                formatter.write_str("completion event contains a duplicate identity")
            }
            Self::CardinalityMismatch => {
                formatter.write_str("root cluster and repair batch cardinality mismatch")
            }
            Self::EpochDidNotChange => {
                formatter.write_str("authorized repair did not create a changed HEAD epoch")
            }
            Self::BudgetExhausted => formatter.write_str("completion budget exhausted"),
            Self::BlockedCleanRoom => {
                formatter.write_str("clean-room questions or unchecked items block completion")
            }
            Self::DriftDetected => formatter.write_str("completion input drift detected"),
            Self::CleanupUncertain => formatter.write_str("completion cleanup is uncertain"),
            Self::ProviderUnavailable => formatter.write_str("completion provider unavailable"),
            Self::IncompleteProviderOutput => {
                formatter.write_str("completion provider output is incomplete")
            }
            Self::MaxRoundsReached => formatter.write_str("completion maximum rounds reached"),
            Self::Port(message) => write!(formatter, "completion port failed: {message}"),
        }
    }
}

impl Error for CompletionError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompletionPortError(String);

impl CompletionPortError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for CompletionPortError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for CompletionPortError {}
