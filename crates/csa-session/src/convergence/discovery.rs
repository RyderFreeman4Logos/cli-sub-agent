use anyhow::{Context, Result, bail};
use csa_process::ProviderTurnCompletion;

use super::{
    CampaignRecord, ConvergenceEvent, ConvergenceLedger, CoverageCellRecord,
    CoverageDispositionRecord, CoveragePlanFinalizationRecord, CoverageRequirement,
    DiscoveryAttemptFinalizationRecord, DiscoveryAttemptId, DiscoveryAttemptRecord, EpochId,
    EpochRecord,
};

/// Why the next discovery execution is required for one coverage cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryRunIntent {
    /// No finalized discovery attempt exists for the cell.
    Initial,
    /// The latest finalized attempt reported an explicit continuation signal.
    Continuation,
    /// The latest finalized natural page produced candidates and needs a zero-new challenge.
    SaturationChallenge,
}

/// The one deterministic protocol action that should follow the supplied immutable ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryDirective {
    /// Append the carried coverage-cell record to the target campaign.
    DefineCoverageCell {
        /// Exact manifest cell to define.
        cell: CoverageCellRecord,
    },
    /// Obtain and append a disposition for the exact carried manifest cell.
    RecordCoverageDisposition {
        /// Exact manifest cell requiring a planning disposition.
        cell: CoverageCellRecord,
    },
    /// Append the carried coverage-plan finalization record.
    FinalizeCoveragePlan {
        /// Event-ready finalization record for the target epoch.
        record: CoveragePlanFinalizationRecord,
    },
    /// Persist candidate records still owed by an already recorded attempt.
    RecordMissingCandidates {
        /// Attempt whose reported candidate evidence is incomplete.
        attempt_id: DiscoveryAttemptId,
        /// Exact number of candidate records still required.
        missing_candidate_count: u32,
    },
    /// Append the carried discovery-attempt finalization record.
    FinalizeDiscoveryAttempt {
        /// Event-ready finalization record for the reconciled attempt.
        record: DiscoveryAttemptFinalizationRecord,
    },
    /// Execute discovery for the exact manifest cell with the inferred intent.
    RunDiscovery {
        /// Exact required coverage cell to discover.
        cell: CoverageCellRecord,
        /// Number of finalized prior attempts for this cell.
        prior_finalized_attempt_count: u32,
        /// Deterministically inferred reason for this execution.
        intent: DiscoveryRunIntent,
    },
    /// Discovery evidence collection is complete for the epoch.
    ///
    /// This is not a clean review verdict and may coexist with candidates that have not yet
    /// received a disposition or independent verification.
    DiscoveryEvidenceComplete {
        /// Epoch whose required cells have fresh zero-new discovery evidence.
        epoch_id: EpochId,
    },
}

struct RecordedCell<'a> {
    record: &'a CoverageCellRecord,
    disposition: Option<&'a CoverageDispositionRecord>,
}

struct RecordedAttempt<'a> {
    record: &'a DiscoveryAttemptRecord,
    candidate_count: u32,
    finalized_at_sequence: Option<u64>,
}

/// Validate and replay immutable convergence evidence, returning exactly the next directive.
///
/// The reducer is synchronous and headless. It performs no I/O, execution, adjudication, or
/// candidate disposition work, and identical inputs always produce the same result.
///
/// # Errors
///
/// Returns an error when the ledger is invalid, the target snapshots are absent or mismatched,
/// the expected manifest is invalid or has drifted from recorded target-epoch cells, a finalized
/// plan is incomplete, or recorded attempt counts cannot be reconciled.
pub fn next_discovery_directive(
    ledger: &ConvergenceLedger,
    expected_campaign: &CampaignRecord,
    expected_epoch: &EpochRecord,
    expected_cells: &[CoverageCellRecord],
) -> Result<DiscoveryDirective> {
    ledger
        .validate()
        .map_err(|error| anyhow::anyhow!("cannot reduce invalid convergence ledger: {error}"))?;

    validate_target_snapshots(ledger, expected_campaign, expected_epoch)?;
    validate_expected_cells(expected_epoch, expected_cells)?;

    let (recorded_cells, plan_finalized) =
        replay_target_plan(ledger, expected_campaign, expected_epoch);
    reject_manifest_drift(&recorded_cells, expected_cells)?;

    for expected_cell in expected_cells {
        let Some(recorded) = recorded_cells
            .iter()
            .find(|recorded| recorded.record.id() == expected_cell.id())
        else {
            if plan_finalized {
                bail!(
                    "finalized coverage plan for epoch {} omits expected cell {}",
                    expected_epoch.id(),
                    expected_cell.id()
                );
            }
            return Ok(DiscoveryDirective::DefineCoverageCell {
                cell: expected_cell.clone(),
            });
        };
        if recorded.record != expected_cell {
            bail!(
                "recorded coverage cell {} does not equal the expected manifest record",
                expected_cell.id()
            );
        }
        if recorded.disposition.is_none() {
            if plan_finalized {
                bail!(
                    "finalized coverage plan for epoch {} lacks a disposition for cell {}",
                    expected_epoch.id(),
                    expected_cell.id()
                );
            }
            return Ok(DiscoveryDirective::RecordCoverageDisposition {
                cell: expected_cell.clone(),
            });
        }
    }

    if !plan_finalized {
        return Ok(DiscoveryDirective::FinalizeCoveragePlan {
            record: CoveragePlanFinalizationRecord::new(expected_epoch.id().clone()),
        });
    }

    let attempts = replay_target_attempts(ledger, expected_campaign, expected_epoch)?;
    for attempt in &attempts {
        if attempt.candidate_count > attempt.record.reported_candidate_count() {
            bail!(
                "discovery attempt {} has {} candidate records but reported {}",
                attempt.record.id(),
                attempt.candidate_count,
                attempt.record.reported_candidate_count()
            );
        }
        if attempt.finalized_at_sequence.is_none() {
            if attempt.candidate_count < attempt.record.reported_candidate_count() {
                return Ok(DiscoveryDirective::RecordMissingCandidates {
                    attempt_id: attempt.record.id().clone(),
                    missing_candidate_count: attempt.record.reported_candidate_count()
                        - attempt.candidate_count,
                });
            }
            return Ok(DiscoveryDirective::FinalizeDiscoveryAttempt {
                record: DiscoveryAttemptFinalizationRecord::new(attempt.record.id().clone()),
            });
        }
    }

    for expected_cell in expected_cells {
        let recorded = recorded_cells
            .iter()
            .find(|recorded| recorded.record.id() == expected_cell.id())
            .context("validated expected coverage cell disappeared during replay")?;
        let requirement = recorded
            .disposition
            .context("finalized coverage plan disposition disappeared during replay")?
            .requirement();
        if requirement == CoverageRequirement::NotApplicable {
            continue;
        }

        let finalized_attempt_count = attempts
            .iter()
            .filter(|attempt| {
                attempt.record.coverage_cell_id() == expected_cell.id()
                    && attempt.finalized_at_sequence.is_some()
            })
            .count();
        let prior_finalized_attempt_count = u32::try_from(finalized_attempt_count)
            .context("more than u32 finalized discovery attempts exist for one coverage cell")?;
        let latest = attempts
            .iter()
            .filter(|attempt| {
                attempt.record.coverage_cell_id() == expected_cell.id()
                    && attempt.finalized_at_sequence.is_some()
            })
            .max_by_key(|attempt| attempt.finalized_at_sequence);

        let Some(latest) = latest else {
            return Ok(DiscoveryDirective::RunDiscovery {
                cell: expected_cell.clone(),
                prior_finalized_attempt_count,
                intent: DiscoveryRunIntent::Initial,
            });
        };
        if is_fresh_zero_new_challenge(latest.record) {
            continue;
        }

        let intent = if has_continuation_signal(latest.record) {
            DiscoveryRunIntent::Continuation
        } else {
            DiscoveryRunIntent::SaturationChallenge
        };
        return Ok(DiscoveryDirective::RunDiscovery {
            cell: expected_cell.clone(),
            prior_finalized_attempt_count,
            intent,
        });
    }

    Ok(DiscoveryDirective::DiscoveryEvidenceComplete {
        epoch_id: expected_epoch.id().clone(),
    })
}

fn validate_target_snapshots(
    ledger: &ConvergenceLedger,
    expected_campaign: &CampaignRecord,
    expected_epoch: &EpochRecord,
) -> Result<()> {
    let recorded_campaign = ledger
        .entries()
        .iter()
        .filter(|entry| entry.campaign_id() == expected_campaign.id())
        .find_map(|entry| match entry.event() {
            ConvergenceEvent::CampaignStarted(record) => Some(record),
            _ => None,
        })
        .with_context(|| format!("unknown convergence campaign {}", expected_campaign.id()))?;
    if recorded_campaign != expected_campaign {
        bail!(
            "campaign {} snapshot does not equal the expected record",
            expected_campaign.id()
        );
    }
    if expected_campaign.policy_digest().is_none() {
        bail!(
            "campaign {} has no frozen policy digest",
            expected_campaign.id()
        );
    }
    if expected_campaign.catalog_digest().is_none() {
        bail!(
            "campaign {} has no frozen model catalog digest",
            expected_campaign.id()
        );
    }

    let recorded_epoch = ledger
        .entries()
        .iter()
        .filter(|entry| entry.campaign_id() == expected_campaign.id())
        .find_map(|entry| match entry.event() {
            ConvergenceEvent::EpochOpened(record) if record.id() == expected_epoch.id() => {
                Some(record)
            }
            _ => None,
        })
        .with_context(|| {
            format!(
                "unknown epoch {} in campaign {}",
                expected_epoch.id(),
                expected_campaign.id()
            )
        })?;
    if recorded_epoch != expected_epoch {
        bail!(
            "epoch {} snapshot in campaign {} does not equal the expected record",
            expected_epoch.id(),
            expected_campaign.id()
        );
    }
    Ok(())
}

fn validate_expected_cells(
    expected_epoch: &EpochRecord,
    expected_cells: &[CoverageCellRecord],
) -> Result<()> {
    if expected_cells.is_empty() {
        bail!(
            "expected coverage manifest for epoch {} must not be empty",
            expected_epoch.id()
        );
    }
    let mut seen_ids = Vec::with_capacity(expected_cells.len());
    for cell in expected_cells {
        cell.validate()
            .with_context(|| format!("invalid expected coverage cell {}", cell.id()))?;
        if cell.epoch_id() != expected_epoch.id() {
            bail!(
                "expected coverage cell {} belongs to epoch {}, not target epoch {}",
                cell.id(),
                cell.epoch_id(),
                expected_epoch.id()
            );
        }
        if seen_ids.iter().any(|seen| seen == cell.id()) {
            bail!("duplicate expected coverage cell {}", cell.id());
        }
        seen_ids.push(cell.id().clone());
    }
    Ok(())
}

fn replay_target_plan<'a>(
    ledger: &'a ConvergenceLedger,
    expected_campaign: &CampaignRecord,
    expected_epoch: &EpochRecord,
) -> (Vec<RecordedCell<'a>>, bool) {
    let mut cells = Vec::<RecordedCell<'a>>::new();
    let mut plan_finalized = false;
    for entry in ledger
        .entries()
        .iter()
        .filter(|entry| entry.campaign_id() == expected_campaign.id())
    {
        match entry.event() {
            ConvergenceEvent::CoverageCellDefined(record)
                if record.epoch_id() == expected_epoch.id() =>
            {
                cells.push(RecordedCell {
                    record,
                    disposition: None,
                });
            }
            ConvergenceEvent::CoverageDispositionRecorded(record) => {
                if let Some(cell) = cells
                    .iter_mut()
                    .find(|cell| cell.record.id() == record.coverage_cell_id())
                {
                    cell.disposition = Some(record);
                }
            }
            ConvergenceEvent::CoveragePlanFinalized(record)
                if record.epoch_id() == expected_epoch.id() =>
            {
                plan_finalized = true;
            }
            _ => {}
        }
    }
    (cells, plan_finalized)
}

fn reject_manifest_drift(
    recorded_cells: &[RecordedCell<'_>],
    expected_cells: &[CoverageCellRecord],
) -> Result<()> {
    for recorded in recorded_cells {
        let expected = expected_cells
            .iter()
            .find(|expected| expected.id() == recorded.record.id())
            .with_context(|| {
                format!(
                    "target epoch ledger contains cell {} absent from the expected manifest",
                    recorded.record.id()
                )
            })?;
        if recorded.record != expected {
            bail!(
                "target epoch ledger cell {} does not equal the expected manifest record",
                recorded.record.id()
            );
        }
    }
    Ok(())
}

fn replay_target_attempts<'a>(
    ledger: &'a ConvergenceLedger,
    expected_campaign: &CampaignRecord,
    expected_epoch: &EpochRecord,
) -> Result<Vec<RecordedAttempt<'a>>> {
    let mut attempts = Vec::<RecordedAttempt<'a>>::new();
    for entry in ledger
        .entries()
        .iter()
        .filter(|entry| entry.campaign_id() == expected_campaign.id())
    {
        match entry.event() {
            ConvergenceEvent::DiscoveryAttemptRecorded(record)
                if record.epoch_id() == expected_epoch.id() =>
            {
                attempts.push(RecordedAttempt {
                    record,
                    candidate_count: 0,
                    finalized_at_sequence: None,
                });
            }
            ConvergenceEvent::CandidateRecorded(record) => {
                if let Some(attempt) = attempts
                    .iter_mut()
                    .find(|attempt| attempt.record.id() == record.discovery_attempt_id())
                {
                    attempt.candidate_count = attempt
                        .candidate_count
                        .checked_add(1)
                        .context("discovery candidate count overflow during replay")?;
                }
            }
            ConvergenceEvent::DiscoveryAttemptFinalized(record) => {
                if let Some(attempt) = attempts
                    .iter_mut()
                    .find(|attempt| attempt.record.id() == record.discovery_attempt_id())
                {
                    attempt.finalized_at_sequence = Some(entry.sequence());
                }
            }
            _ => {}
        }
    }
    Ok(attempts)
}

fn is_fresh_zero_new_challenge(attempt: &DiscoveryAttemptRecord) -> bool {
    attempt.completion() == ProviderTurnCompletion::Natural
        && attempt.reported_candidate_count() == 0
        && attempt.reported_candidate_count() < attempt.candidate_limit()
        && !attempt.more_candidates_possible()
        && attempt.unscanned_items().is_empty()
}

fn has_continuation_signal(attempt: &DiscoveryAttemptRecord) -> bool {
    attempt.completion() != ProviderTurnCompletion::Natural
        || attempt.reported_candidate_count() == attempt.candidate_limit()
        || attempt.more_candidates_possible()
        || !attempt.unscanned_items().is_empty()
}
