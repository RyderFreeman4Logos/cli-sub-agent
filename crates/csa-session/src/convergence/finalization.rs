use serde::{Deserialize, Serialize};

use super::{DiscoveryAttemptId, EpochId};

/// Persisted marker sealing the complete coverage plan for one opened epoch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoveragePlanFinalizationRecord {
    epoch_id: EpochId,
}

impl CoveragePlanFinalizationRecord {
    /// Construct a coverage-plan finalization marker.
    #[must_use]
    pub fn new(epoch_id: EpochId) -> Self {
        Self { epoch_id }
    }

    /// Return the epoch whose coverage plan is sealed.
    #[must_use]
    pub fn epoch_id(&self) -> &EpochId {
        &self.epoch_id
    }
}

/// Persisted marker sealing the candidate evidence produced by one discovery attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryAttemptFinalizationRecord {
    discovery_attempt_id: DiscoveryAttemptId,
}

impl DiscoveryAttemptFinalizationRecord {
    /// Construct a discovery-attempt evidence finalization marker.
    #[must_use]
    pub fn new(discovery_attempt_id: DiscoveryAttemptId) -> Self {
        Self {
            discovery_attempt_id,
        }
    }

    /// Return the discovery attempt whose candidate evidence is sealed.
    #[must_use]
    pub fn discovery_attempt_id(&self) -> &DiscoveryAttemptId {
        &self.discovery_attempt_id
    }
}
