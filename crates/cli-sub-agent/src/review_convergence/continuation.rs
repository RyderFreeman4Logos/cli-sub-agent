use std::collections::{BTreeMap, BTreeSet};

use csa_session::convergence::{
    CampaignRecord, ConvergenceEvent, ConvergenceLedger, CoverageCellRecord, EpochRecord,
    SemanticFindingIdentity, StableFindingId,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContinuationFinding {
    pub(crate) stable_finding_id: StableFindingId,
    pub(crate) semantic_identity: SemanticFindingIdentity,
}

impl ContinuationFinding {
    pub(crate) fn new(
        stable_finding_id: StableFindingId,
        semantic_identity: SemanticFindingIdentity,
    ) -> Self {
        Self {
            stable_finding_id,
            semantic_identity,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContinuationEvidence {
    pub(crate) findings: Vec<ContinuationFinding>,
    pub(crate) latest_finalized_unscanned_items: Vec<String>,
    pub(crate) uncovered_cells: Vec<CoverageCellRecord>,
    pub(crate) uncovered_items: Vec<String>,
}

impl ContinuationEvidence {
    pub(crate) fn new(
        findings: Vec<ContinuationFinding>,
        latest_finalized_unscanned_items: Vec<String>,
        uncovered_cells: Vec<CoverageCellRecord>,
        uncovered_items: Vec<String>,
    ) -> Self {
        Self {
            findings,
            latest_finalized_unscanned_items,
            uncovered_cells,
            uncovered_items,
        }
    }

    pub(crate) fn stable_finding_ids(&self) -> BTreeSet<String> {
        self.findings
            .iter()
            .map(|finding| finding.stable_finding_id.as_str().to_string())
            .collect()
    }
}

pub(crate) fn from_ledger(
    ledger: &ConvergenceLedger,
    campaign: &CampaignRecord,
    epoch: &EpochRecord,
    uncovered_cells: Vec<CoverageCellRecord>,
) -> ContinuationEvidence {
    let attempts = ledger
        .entries()
        .iter()
        .filter(|entry| entry.campaign_id() == campaign.id())
        .filter_map(|entry| match entry.event() {
            ConvergenceEvent::DiscoveryAttemptRecorded(record)
                if record.epoch_id() == epoch.id() =>
            {
                Some((record.id().as_str().to_string(), record))
            }
            _ => None,
        })
        .collect::<BTreeMap<_, _>>();
    let findings = ledger
        .entries()
        .iter()
        .filter(|entry| entry.campaign_id() == campaign.id())
        .filter_map(|entry| match entry.event() {
            ConvergenceEvent::CandidateRecorded(record)
                if attempts.contains_key(record.discovery_attempt_id().as_str()) =>
            {
                Some((
                    record.stable_finding_id().as_str().to_string(),
                    ContinuationFinding::new(
                        record.stable_finding_id().clone(),
                        record.semantic_identity().clone(),
                    ),
                ))
            }
            _ => None,
        })
        .collect::<BTreeMap<_, _>>()
        .into_values()
        .collect();
    let latest_finalized_unscanned_items = ledger
        .entries()
        .iter()
        .rev()
        .filter(|entry| entry.campaign_id() == campaign.id())
        .find_map(|entry| match entry.event() {
            ConvergenceEvent::DiscoveryAttemptFinalized(record) => attempts
                .get(record.discovery_attempt_id().as_str())
                .map(|attempt| attempt.unscanned_items().to_vec()),
            _ => None,
        })
        .unwrap_or_default();
    let uncovered_items = latest_finalized_unscanned_items.clone();
    ContinuationEvidence::new(
        findings,
        latest_finalized_unscanned_items,
        uncovered_cells,
        uncovered_items,
    )
}
