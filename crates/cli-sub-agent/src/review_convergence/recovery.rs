use std::collections::HashSet;

use csa_session::convergence::{
    CampaignRecord, CandidateId, CandidateRecord, ConvergenceEvent, ConvergenceLedger,
    DiscoveryAttemptId, DiscoveryAttemptRecord,
};

use super::bundle::ProviderEvidenceIdentity;
use super::engine::{DiscoveryRunner, EngineError, blocked};
use super::output::decode_discovery_page_artifact;
use super::schema::{ParsedDiscoveryPage, parse_discovery_page};

pub(super) async fn recover_missing_candidate<R: DiscoveryRunner>(
    runner: &mut R,
    ledger: &ConvergenceLedger,
    campaign: &CampaignRecord,
    attempt_id: &DiscoveryAttemptId,
    expected_provider_evidence: &ProviderEvidenceIdentity,
    calls: usize,
) -> std::result::Result<CandidateRecord, EngineError> {
    let attempt = ledger
        .entries()
        .iter()
        .filter(|entry| entry.campaign_id() == campaign.id())
        .find_map(|entry| match entry.event() {
            ConvergenceEvent::DiscoveryAttemptRecorded(record) if record.id() == attempt_id => {
                Some(record)
            }
            _ => None,
        })
        .ok_or_else(|| {
            blocked(
                "recovery_attempt_missing",
                format!("persisted discovery attempt {attempt_id} was not found"),
                calls,
            )
        })?;
    let artifact_bytes = runner
        .read_artifact(attempt.artifact())
        .await
        .map_err(|error| blocked("artifact_read_failure", format!("{error:#}"), calls))?;
    let raw = decode_discovery_page_artifact(
        &artifact_bytes,
        attempt.artifact().digest(),
        expected_provider_evidence,
    )
    .map_err(|error| blocked("artifact_validation_failure", format!("{error:#}"), calls))?;
    let page = parse_discovery_page(&raw)
        .map_err(|error| blocked("artifact_parser_failure", format!("{error:#}"), calls))?;
    validate_recovered_page(attempt, &page, calls)?;

    let recorded = ledger
        .entries()
        .iter()
        .filter(|entry| entry.campaign_id() == campaign.id())
        .filter_map(|entry| match entry.event() {
            ConvergenceEvent::CandidateRecorded(record)
                if record.discovery_attempt_id() == attempt_id =>
            {
                Some(record.stable_finding_id().as_str().to_string())
            }
            _ => None,
        })
        .collect::<HashSet<_>>();
    let identity = page
        .candidates
        .into_iter()
        .find(|identity| {
            let stable = csa_session::convergence::StableFindingId::compute(identity);
            !recorded.contains(stable.as_str())
        })
        .ok_or_else(|| {
            blocked(
                "artifact_candidate_mismatch",
                "artifact contains no candidate still owed by the persisted attempt",
                calls,
            )
        })?;
    Ok(CandidateRecord::new(
        CandidateId::generate(),
        attempt_id.clone(),
        identity,
        attempt.artifact().clone(),
    ))
}

fn validate_recovered_page(
    attempt: &DiscoveryAttemptRecord,
    page: &ParsedDiscoveryPage,
    calls: usize,
) -> std::result::Result<(), EngineError> {
    let candidate_count = u32::try_from(page.candidates.len()).map_err(|error| {
        blocked(
            "artifact_candidate_count_overflow",
            error.to_string(),
            calls,
        )
    })?;
    if page.candidate_limit != attempt.candidate_limit()
        || candidate_count != attempt.reported_candidate_count()
        || page.continuation_required() != attempt.more_candidates_possible()
        || page.unscanned_items != attempt.unscanned_items()
    {
        return Err(blocked(
            "artifact_attempt_mismatch",
            "durable page content does not match its persisted discovery-attempt metadata",
            calls,
        ));
    }
    Ok(())
}
