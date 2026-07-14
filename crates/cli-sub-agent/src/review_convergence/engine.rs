use std::collections::{BTreeSet, HashSet};
use std::error::Error;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use chrono::Utc;
use csa_process::ProviderTurnCompletion;
use csa_session::convergence::{
    ArtifactEvidenceRef, CampaignId, CampaignRecord, ConvergenceEvent, ConvergenceLedger,
    ConvergenceLedgerStore, CoverageCellRecord, CoverageDispositionRecord, CoverageRequirement,
    CoverageScope, DiscoveryAttemptId, DiscoveryAttemptRecord, DiscoveryDirective,
    DiscoveryRunIntent, EpochRecord, GitObjectId, SemanticLens, Sha256Digest,
    next_discovery_directive,
};
use serde::Serialize;

use super::bundle::ProviderEvidenceRef;
pub(crate) use super::output::DiscoveryRunOutput;
use super::recovery::recover_missing_candidate;
use super::schema::parse_discovery_page;

/// Walking-skeleton guardrail: one cell can consume at most four provider calls.
pub(crate) const MAX_PROVIDER_CALLS_PER_CELL: usize = 4;
pub(crate) const PAGE_CANDIDATE_LIMIT: u32 = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FrozenWorkspace {
    pub(crate) base_oid: String,
    pub(crate) head_oid: String,
    pub(crate) diff_digest: Sha256Digest,
    pub(crate) index_clean: bool,
    pub(crate) worktree_clean: bool,
    pub(crate) provider_evidence: ProviderEvidenceRef,
}

impl FrozenWorkspace {
    #[cfg(test)]
    pub(crate) fn new(
        base_oid: &str,
        head_oid: &str,
        diff_digest: Sha256Digest,
        index_clean: bool,
        worktree_clean: bool,
    ) -> Result<Self> {
        let provider_evidence = ProviderEvidenceRef::synthetic(base_oid, head_oid, &diff_digest);
        Self::new_with_provider_evidence(
            base_oid,
            head_oid,
            diff_digest,
            index_clean,
            worktree_clean,
            provider_evidence,
        )
    }

    pub(crate) fn new_with_provider_evidence(
        base_oid: &str,
        head_oid: &str,
        diff_digest: Sha256Digest,
        index_clean: bool,
        worktree_clean: bool,
        provider_evidence: ProviderEvidenceRef,
    ) -> Result<Self> {
        if !provider_evidence.matches_tuple(base_oid, head_oid, &diff_digest) {
            anyhow::bail!("provider evidence identity does not match the frozen exact-OID tuple");
        }
        Ok(Self {
            base_oid: GitObjectId::parse(base_oid)?.as_str().to_string(),
            head_oid: GitObjectId::parse(head_oid)?.as_str().to_string(),
            diff_digest,
            index_clean,
            worktree_clean,
            provider_evidence,
        })
    }

    fn epoch(&self) -> Result<EpochRecord> {
        Ok(EpochRecord::new(
            GitObjectId::parse(&self.base_oid)?,
            GitObjectId::parse(&self.head_oid)?,
            self.diff_digest.clone(),
        ))
    }
}

pub(crate) trait WorkspaceProbe {
    fn capture(&mut self, range: &str) -> Result<FrozenWorkspace>;
}

pub(crate) trait LedgerPort {
    fn load(&self) -> Result<ConvergenceLedger>;
    fn append(&self, campaign_id: CampaignId, event: ConvergenceEvent) -> Result<()>;
}

impl LedgerPort for ConvergenceLedgerStore {
    fn load(&self) -> Result<ConvergenceLedger> {
        ConvergenceLedgerStore::load(self)
    }

    fn append(&self, campaign_id: CampaignId, event: ConvergenceEvent) -> Result<()> {
        ConvergenceLedgerStore::append(self, campaign_id, event)
            .map(|_| ())
            .map_err(|error| anyhow!(error))
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
    pub(crate) existing_fingerprints: Vec<String>,
}

impl DiscoveryRequest {
    #[cfg(test)]
    pub(crate) fn for_test(frozen: FrozenWorkspace) -> Self {
        let epoch = frozen.epoch().expect("test epoch");
        Self {
            frozen,
            range: "main...HEAD".to_string(),
            cell: observation_cell(&epoch, "main...HEAD").expect("test cell"),
            prior_finalized_attempt_count: 0,
            intent: DiscoveryRunIntent::Initial,
            candidate_limit: PAGE_CANDIDATE_LIMIT,
            existing_fingerprints: Vec::new(),
        }
    }
}

pub(crate) trait DiscoveryRunner {
    fn run<'a>(
        &'a mut self,
        request: DiscoveryRequest,
    ) -> Pin<Box<dyn Future<Output = Result<DiscoveryRunOutput>> + 'a>>;

    fn read_artifact<'a>(
        &'a mut self,
        artifact: &'a ArtifactEvidenceRef,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>>> + 'a>>;
}

#[derive(Debug, Clone)]
pub(crate) struct ObservationInput {
    pub(crate) range: String,
    pub(crate) catalog_digest: Sha256Digest,
}

impl ObservationInput {
    pub(crate) fn new(range: &str, catalog_digest: Sha256Digest) -> Self {
        Self {
            range: range.to_string(),
            catalog_digest,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct PhaseTimings {
    pub(crate) planning_ms: u64,
    pub(crate) execution_ms: u64,
    pub(crate) persistence_ms: u64,
    pub(crate) total_ms: u64,
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
            coverage_cell_count: 1,
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
            semantic_coverage: "walking-skeleton observation cell; not exhaustive semantic coverage",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct BlockedDiagnostic {
    pub(crate) kind: &'static str,
    pub(crate) reason_code: &'static str,
    pub(crate) message: String,
    pub(crate) provider_calls: usize,
    pub(crate) discovery_evidence_complete: bool,
    pub(crate) review_verdict: Option<String>,
    pub(crate) merge_attestation: bool,
}

#[derive(Debug)]
pub(crate) struct EngineError(BlockedDiagnostic);

impl EngineError {
    pub(crate) fn diagnostic(&self) -> &BlockedDiagnostic {
        &self.0
    }
}

impl fmt::Display for EngineError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.0.reason_code, self.0.message)
    }
}

impl Error for EngineError {}

pub(super) fn blocked(
    reason_code: &'static str,
    message: impl Into<String>,
    calls: usize,
) -> EngineError {
    EngineError(BlockedDiagnostic {
        kind: "convergence_discovery_blocked",
        reason_code,
        message: message.into(),
        provider_calls: calls,
        discovery_evidence_complete: false,
        review_verdict: None,
        merge_attestation: false,
    })
}

pub(crate) async fn run_discovery_observation<P, R, S>(
    input: &ObservationInput,
    probe: &mut P,
    runner: &mut R,
    store: &S,
) -> std::result::Result<ObservationSummary, EngineError>
where
    P: WorkspaceProbe,
    R: DiscoveryRunner,
    S: LedgerPort,
{
    let total_started = Instant::now();
    let planning_started = Instant::now();
    let frozen = probe
        .capture(&input.range)
        .map_err(|error| blocked("workspace_probe_failure", format!("{error:#}"), 0))?;
    if !frozen.index_clean || !frozen.worktree_clean {
        return Err(blocked(
            "workspace_not_clean",
            "the explicit range must start with a clean index and worktree",
            0,
        ));
    }
    let epoch = frozen
        .epoch()
        .map_err(|error| blocked("workspace_probe_failure", format!("{error:#}"), 0))?;
    let cell = observation_cell(&epoch, &input.range)
        .map_err(|error| blocked("planning_failure", format!("{error:#}"), 0))?;
    let policy_digest = super::walking_skeleton_policy_digest();
    let mut persistence = Duration::ZERO;
    let (campaign, mut ledger) = initialize_campaign(
        store,
        &epoch,
        &policy_digest,
        &input.catalog_digest,
        &mut persistence,
    )?;
    let planning = planning_started.elapsed();
    let mut execution = Duration::ZERO;
    let mut provider_calls = recorded_attempt_count(&ledger, &campaign, &epoch);

    loop {
        let directive =
            next_discovery_directive(&ledger, &campaign, &epoch, std::slice::from_ref(&cell))
                .map_err(|error| {
                    blocked("reducer_failure", format!("{error:#}"), provider_calls)
                })?;
        match directive {
            DiscoveryDirective::DefineCoverageCell { cell } => persist(
                store,
                &campaign,
                ConvergenceEvent::CoverageCellDefined(cell),
                &mut ledger,
                &mut persistence,
                provider_calls,
            )?,
            DiscoveryDirective::RecordCoverageDisposition { cell } => {
                let record = CoverageDispositionRecord::new(
                    cell.id().clone(),
                    CoverageRequirement::Required,
                    "walking_skeleton_observation",
                    "Required whole-range broad-discovery observation cell; this is not exhaustive semantic coverage.",
                )
                .map_err(|error| blocked("planning_failure", format!("{error:#}"), provider_calls))?;
                persist(
                    store,
                    &campaign,
                    ConvergenceEvent::CoverageDispositionRecorded(record),
                    &mut ledger,
                    &mut persistence,
                    provider_calls,
                )?;
            }
            DiscoveryDirective::FinalizeCoveragePlan { record } => persist(
                store,
                &campaign,
                ConvergenceEvent::CoveragePlanFinalized(record),
                &mut ledger,
                &mut persistence,
                provider_calls,
            )?,
            DiscoveryDirective::RunDiscovery {
                cell,
                prior_finalized_attempt_count,
                intent,
            } => {
                if usize::try_from(prior_finalized_attempt_count).unwrap_or(usize::MAX)
                    >= MAX_PROVIDER_CALLS_PER_CELL
                {
                    return Err(blocked(
                        "provider_call_budget_exhausted",
                        format!(
                            "walking-skeleton cell exceeded its {MAX_PROVIDER_CALLS_PER_CELL}-call safety budget"
                        ),
                        provider_calls,
                    ));
                }
                assert_frozen(probe, input, &frozen, provider_calls)?;
                let fingerprints = existing_fingerprints(&ledger, &campaign, &epoch);
                let request = DiscoveryRequest {
                    frozen: frozen.clone(),
                    range: input.range.clone(),
                    cell,
                    prior_finalized_attempt_count,
                    intent,
                    candidate_limit: PAGE_CANDIDATE_LIMIT,
                    existing_fingerprints: fingerprints.iter().cloned().collect(),
                };
                let execution_started = Instant::now();
                let run_result = runner.run(request.clone()).await;
                execution += execution_started.elapsed();
                provider_calls += 1;
                assert_frozen(probe, input, &frozen, provider_calls)?;
                let output = run_result.map_err(|error| {
                    blocked("provider_failure", format!("{error:#}"), provider_calls)
                })?;
                if output.completion != ProviderTurnCompletion::Natural {
                    return Err(blocked(
                        "provider_noncompletion",
                        format!("provider completion was {:?}", output.completion),
                        provider_calls,
                    ));
                }
                let page = parse_discovery_page(&output.raw_response).map_err(|error| {
                    blocked("parser_failure", format!("{error:#}"), provider_calls)
                })?;
                if page.candidate_limit != request.candidate_limit {
                    return Err(blocked(
                        "candidate_limit_mismatch",
                        "response candidate_limit did not equal the requested limit",
                        provider_calls,
                    ));
                }
                let attempt_id = DiscoveryAttemptId::generate();
                for identity in &page.candidates {
                    let stable = csa_session::convergence::StableFindingId::compute(identity);
                    if fingerprints.contains(stable.as_str()) {
                        return Err(blocked(
                            "duplicate_existing_fingerprint",
                            "provider repeated an existing semantic fingerprint",
                            provider_calls,
                        ));
                    }
                }
                let count = u32::try_from(page.candidates.len()).map_err(|error| {
                    blocked(
                        "candidate_count_overflow",
                        error.to_string(),
                        provider_calls,
                    )
                })?;
                let attempt = DiscoveryAttemptRecord::new(
                    attempt_id.clone(),
                    epoch.id().clone(),
                    request.cell.id().clone(),
                    Utc::now(),
                    output.completion,
                    output.model_identity,
                    output.artifact,
                    page.candidate_limit,
                    count,
                    page.continuation_required(),
                    page.unscanned_items.clone(),
                )
                .map_err(|error| blocked("parser_failure", format!("{error:#}"), provider_calls))?;
                persist(
                    store,
                    &campaign,
                    ConvergenceEvent::DiscoveryAttemptRecorded(attempt),
                    &mut ledger,
                    &mut persistence,
                    provider_calls,
                )?;
            }
            DiscoveryDirective::RecordMissingCandidates {
                attempt_id,
                missing_candidate_count: _,
            } => {
                let record = recover_missing_candidate(
                    runner,
                    &ledger,
                    &campaign,
                    &attempt_id,
                    &frozen.provider_evidence.identity,
                    provider_calls,
                )
                .await?;
                persist(
                    store,
                    &campaign,
                    ConvergenceEvent::CandidateRecorded(record),
                    &mut ledger,
                    &mut persistence,
                    provider_calls,
                )?;
            }
            DiscoveryDirective::FinalizeDiscoveryAttempt { record } => persist(
                store,
                &campaign,
                ConvergenceEvent::DiscoveryAttemptFinalized(record),
                &mut ledger,
                &mut persistence,
                provider_calls,
            )?,
            DiscoveryDirective::DiscoveryEvidenceComplete { epoch_id } => {
                let total = total_started.elapsed();
                return Ok(ObservationSummary {
                    kind: "convergence_discovery_observation",
                    campaign_id: campaign.id().to_string(),
                    epoch_id: epoch_id.to_string(),
                    base_oid: frozen.base_oid,
                    head_oid: frozen.head_oid,
                    diff_digest: frozen.diff_digest.to_string(),
                    index_clean: frozen.index_clean,
                    worktree_clean: frozen.worktree_clean,
                    coverage_cell_count: 1,
                    provider_calls,
                    candidates: existing_fingerprints(&ledger, &campaign, &epoch).len(),
                    phase_timings: PhaseTimings {
                        planning_ms: millis(planning),
                        execution_ms: millis(execution),
                        persistence_ms: millis(persistence),
                        total_ms: millis(total),
                    },
                    discovery_evidence_complete: true,
                    review_verdict: None,
                    merge_attestation: false,
                    semantic_coverage: "walking-skeleton observation cell; not exhaustive semantic coverage",
                });
            }
        }
    }
}

fn observation_cell(epoch: &EpochRecord, range: &str) -> Result<CoverageCellRecord> {
    Ok(CoverageCellRecord::new(
        epoch.id().clone(),
        CoverageScope::new("explicit_whole_range", range)?,
        SemanticLens::new("broad_discovery_walking_skeleton_observation")?,
    ))
}

fn initialize_campaign<S: LedgerPort>(
    store: &S,
    epoch: &EpochRecord,
    policy_digest: &Sha256Digest,
    catalog_digest: &Sha256Digest,
    persistence: &mut Duration,
) -> std::result::Result<(CampaignRecord, ConvergenceLedger), EngineError> {
    let mut ledger = store
        .load()
        .map_err(|error| blocked("store_failure", format!("{error:#}"), 0))?;
    let campaign = ledger.entries().iter().rev().find_map(|entry| {
        let ConvergenceEvent::CampaignStarted(campaign) = entry.event() else {
            return None;
        };
        (campaign.policy_digest() == Some(policy_digest)
            && campaign.catalog_digest() == Some(catalog_digest))
        .then(|| campaign.clone())
    });
    let campaign = campaign.unwrap_or_else(|| {
        CampaignRecord::new(
            CampaignId::generate(),
            Utc::now(),
            Some(policy_digest.clone()),
            Some(catalog_digest.clone()),
        )
    });
    let campaign_exists = ledger.entries().iter().any(|entry| {
        entry.campaign_id() == campaign.id()
            && matches!(entry.event(), ConvergenceEvent::CampaignStarted(_))
    });
    if !campaign_exists {
        persist(
            store,
            &campaign,
            ConvergenceEvent::CampaignStarted(campaign.clone()),
            &mut ledger,
            persistence,
            0,
        )?;
    }
    let epoch_exists = ledger.entries().iter().any(|entry| {
        entry.campaign_id() == campaign.id()
            && matches!(entry.event(), ConvergenceEvent::EpochOpened(record) if record == epoch)
    });
    if !epoch_exists {
        persist(
            store,
            &campaign,
            ConvergenceEvent::EpochOpened(epoch.clone()),
            &mut ledger,
            persistence,
            0,
        )?;
    }
    Ok((campaign, ledger))
}

fn persist<S: LedgerPort>(
    store: &S,
    campaign: &CampaignRecord,
    event: ConvergenceEvent,
    ledger: &mut ConvergenceLedger,
    elapsed: &mut Duration,
    calls: usize,
) -> std::result::Result<(), EngineError> {
    let started = Instant::now();
    store
        .append(campaign.id().clone(), event)
        .map_err(|error| blocked("store_failure", format!("{error:#}"), calls))?;
    *ledger = store
        .load()
        .map_err(|error| blocked("store_failure", format!("{error:#}"), calls))?;
    *elapsed += started.elapsed();
    Ok(())
}

fn assert_frozen<P: WorkspaceProbe>(
    probe: &mut P,
    input: &ObservationInput,
    expected: &FrozenWorkspace,
    calls: usize,
) -> std::result::Result<(), EngineError> {
    let actual = probe
        .capture(&input.range)
        .map_err(|error| blocked("workspace_probe_failure", format!("{error:#}"), calls))?;
    if &actual != expected {
        return Err(blocked(
            "workspace_mutated",
            "base/head/diff/cleanliness tuple changed during discovery; mixed evidence was rejected",
            calls,
        ));
    }
    Ok(())
}

fn existing_fingerprints(
    ledger: &ConvergenceLedger,
    campaign: &CampaignRecord,
    epoch: &EpochRecord,
) -> BTreeSet<String> {
    let attempts = ledger
        .entries()
        .iter()
        .filter(|entry| entry.campaign_id() == campaign.id())
        .filter_map(|entry| match entry.event() {
            ConvergenceEvent::DiscoveryAttemptRecorded(record)
                if record.epoch_id() == epoch.id() =>
            {
                Some(record.id().as_str().to_string())
            }
            _ => None,
        })
        .collect::<HashSet<_>>();
    ledger
        .entries()
        .iter()
        .filter(|entry| entry.campaign_id() == campaign.id())
        .filter_map(|entry| match entry.event() {
            ConvergenceEvent::CandidateRecorded(record)
                if attempts.contains(record.discovery_attempt_id().as_str()) =>
            {
                Some(record.stable_finding_id().as_str().to_string())
            }
            _ => None,
        })
        .collect()
}

fn recorded_attempt_count(
    ledger: &ConvergenceLedger,
    campaign: &CampaignRecord,
    epoch: &EpochRecord,
) -> usize {
    ledger
        .entries()
        .iter()
        .filter(|entry| entry.campaign_id() == campaign.id())
        .filter(|entry| {
            matches!(
                entry.event(),
                ConvergenceEvent::DiscoveryAttemptRecorded(record)
                    if record.epoch_id() == epoch.id()
            )
        })
        .count()
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}
