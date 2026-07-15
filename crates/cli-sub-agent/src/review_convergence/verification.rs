//! Independent, artifact-bound verification of discovery candidates.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;

use super::engine::{EngineError, FrozenWorkspace, LedgerPort, blocked};
use anyhow::{Context, Result, bail};
use csa_session::convergence::{
    AdmittedModelIdentity, CampaignId, CampaignRecord, CandidateDisposition,
    CandidateDispositionRecord, CandidateId, CandidateRecord, CandidateVerificationEvidence,
    CommandAuthoritySnapshot, ConvergenceEvent, ConvergenceLedger, EpochRecord,
    VerificationIndependence,
};

pub(crate) use super::verification_schema::{
    VERIFIER_ARTIFACT_FILE, VERIFIER_ARTIFACT_PATH, decode_verifier_artifact,
    encode_verifier_artifact, parse_verifier_page,
};

/// The execution constraints which make each candidate verifier independent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VerifierExecutionPolicy {
    pub(crate) fresh_session: bool,
    pub(crate) readonly_project_root: bool,
    pub(crate) resumes_discovery_state: bool,
    pub(crate) includes_discovery_transcript: bool,
}

impl VerifierExecutionPolicy {
    pub(crate) const fn independent() -> Self {
        Self {
            fresh_session: true,
            readonly_project_root: true,
            resumes_discovery_state: false,
            includes_discovery_transcript: false,
        }
    }
}

/// One immutable request for a fresh verifier session.
#[derive(Debug, Clone)]
pub(crate) struct CandidateVerificationRequest {
    pub(crate) frozen: FrozenWorkspace,
    pub(crate) campaign_id: CampaignId,
    pub(crate) candidate: CandidateRecord,
    pub(crate) discovery_executor: AdmittedModelIdentity,
    pub(crate) selected_verifier: AdmittedModelIdentity,
    pub(crate) independence: VerificationIndependence,
    pub(crate) policy: VerifierExecutionPolicy,
}

/// Strict, verifier-supplied repair scope for one blocking candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VerifiedRepairScope {
    pub(crate) root_cause_key: String,
    pub(crate) corrections: Vec<String>,
    pub(crate) regression_tests: Vec<String>,
    pub(crate) docs_contracts: Vec<String>,
    pub(crate) compatibility_migrations: Vec<String>,
    pub(crate) sibling_call_sites: Vec<String>,
}

/// Strictly parsed terminal verification response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedVerificationPage {
    pub(crate) candidate_id: CandidateId,
    pub(crate) stable_finding_id: String,
    pub(crate) disposition: CandidateDisposition,
    pub(crate) repair_scope: Option<VerifiedRepairScope>,
}

/// Artifact-bound result of one fresh verifier session.
#[derive(Debug, Clone)]
pub(crate) struct CandidateVerificationOutput {
    pub(crate) page: ParsedVerificationPage,
    pub(crate) actual_executor: AdmittedModelIdentity,
    pub(crate) artifact: csa_session::convergence::ArtifactEvidenceRef,
}

/// Boundary that runs a new verifier without exposing discovery session state.
pub(crate) trait CandidateVerifier {
    fn verify<'a>(
        &'a mut self,
        request: CandidateVerificationRequest,
    ) -> Pin<Box<dyn Future<Output = Result<CandidateVerificationOutput>> + 'a>>;
}

/// Deterministic result after every canonical candidate has one disposition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VerificationSummary {
    pub(crate) canonical_candidates: usize,
    pub(crate) terminal_dispositions: usize,
}

/// Prefer the first admitted executor different from discovery, recording a reason otherwise.
pub(crate) fn select_verifier(
    authority: &CommandAuthoritySnapshot,
    discovery_executor: &AdmittedModelIdentity,
) -> Result<(AdmittedModelIdentity, VerificationIndependence)> {
    if let Some(identity) = authority
        .ordered_admitted()
        .iter()
        .find(|identity| *identity != discovery_executor)
    {
        return Ok((identity.clone(), VerificationIndependence::Heterogeneous));
    }
    let selected = authority
        .ordered_admitted()
        .first()
        .cloned()
        .context("frozen command authority has no admitted verifier")?;
    if &selected != discovery_executor {
        bail!("frozen verifier selection did not preserve a heterogeneous admitted executor");
    }
    Ok((
        selected,
        VerificationIndependence::degraded(
            "the frozen command authority has no admitted executor distinct from discovery",
        )?,
    ))
}

/// Verify every canonical candidate and atomically record its terminal disposition.
pub(crate) async fn verify_campaign<S: LedgerPort, R: CandidateVerifier>(
    store: &S,
    campaign: &CampaignRecord,
    epoch: &EpochRecord,
    frozen: &FrozenWorkspace,
    verifier: &mut R,
) -> std::result::Result<VerificationSummary, EngineError> {
    let ledger = store
        .load()
        .map_err(|error| blocked("store_failure", format!("{error:#}"), 0))?;
    let candidates = canonical_candidates(&ledger, campaign, epoch)
        .map_err(|error| blocked("verification_evidence_invalid", format!("{error:#}"), 0))?;
    let existing = terminal_dispositions(&ledger, campaign, epoch)
        .map_err(|error| blocked("verification_evidence_invalid", format!("{error:#}"), 0))?;
    if existing.len() == candidates.total_count {
        return Ok(VerificationSummary {
            canonical_candidates: candidates.canonical.len(),
            terminal_dispositions: existing.len(),
        });
    }
    if !existing.is_empty() {
        return Err(blocked(
            "incomplete_terminal_dispositions",
            "candidate verification cannot resume from a partial terminal-disposition set",
            0,
        ));
    }

    let mut events = Vec::with_capacity(candidates.total_count);
    let mut canonical_evidence = HashMap::new();
    for candidate in candidates.all {
        let canonical_id = candidates
            .canonical_by_stable
            .get(candidate.stable_finding_id().as_str())
            .context("candidate stable identity lost its deterministic canonical target")
            .map_err(|error| blocked("verification_evidence_invalid", format!("{error:#}"), 0))?;
        if candidate.id() != canonical_id {
            let verification = canonical_evidence
                .get(canonical_id)
                .cloned()
                .context("duplicate candidate appeared before its canonical verifier evidence")
                .map_err(|error| {
                    blocked("verification_evidence_invalid", format!("{error:#}"), 0)
                })?;
            events.push(ConvergenceEvent::CandidateDispositionRecorded(
                CandidateDispositionRecord::new(
                    candidate.id().clone(),
                    CandidateDisposition::Duplicate {
                        canonical_candidate_id: canonical_id.clone(),
                    },
                    verification,
                ),
            ));
            continue;
        }
        let discovery_executor = candidates
            .attempt_executors
            .get(candidate.discovery_attempt_id())
            .cloned()
            .context("canonical candidate discovery attempt is absent from immutable epoch")
            .map_err(|error| blocked("verification_evidence_invalid", format!("{error:#}"), 0))?;
        let (selected_verifier, independence) =
            select_verifier(campaign.command_authority(), &discovery_executor).map_err(
                |error| blocked("verification_selection_failure", format!("{error:#}"), 0),
            )?;
        let request = CandidateVerificationRequest {
            frozen: frozen.clone(),
            campaign_id: campaign.id().clone(),
            candidate: candidate.clone(),
            discovery_executor,
            selected_verifier: selected_verifier.clone(),
            independence: independence.clone(),
            policy: VerifierExecutionPolicy::independent(),
        };
        let output = verifier
            .verify(request)
            .await
            .map_err(|error| blocked("verifier_failure", format!("{error:#}"), 0))?;
        validate_verifier_output(
            &output,
            &candidate,
            epoch,
            &selected_verifier,
            campaign.command_authority(),
        )
        .map_err(|error| blocked("verifier_output_invalid", format!("{error:#}"), 0))?;
        let verification = CandidateVerificationEvidence::new(
            epoch.id().clone(),
            output.actual_executor,
            independence,
            output.artifact,
        );
        canonical_evidence.insert(candidate.id().clone(), verification.clone());
        events.push(ConvergenceEvent::CandidateDispositionRecorded(
            CandidateDispositionRecord::new(
                candidate.id().clone(),
                output.page.disposition,
                verification,
            ),
        ));
    }
    store
        .append_batch(campaign.id().clone(), events)
        .map_err(|error| blocked("store_failure", format!("{error:#}"), 0))?;
    Ok(VerificationSummary {
        canonical_candidates: candidates.canonical.len(),
        terminal_dispositions: candidates.total_count,
    })
}

pub(crate) fn campaign_epoch(
    ledger: &ConvergenceLedger,
    campaign_id: &CampaignId,
    epoch: &EpochRecord,
) -> Result<CampaignRecord> {
    let campaign = ledger
        .entries()
        .iter()
        .find_map(|entry| match entry.event() {
            ConvergenceEvent::CampaignStarted(record) if entry.campaign_id() == campaign_id => {
                Some(record.clone())
            }
            _ => None,
        })
        .context("verification campaign is absent from the immutable ledger")?;
    if !ledger.entries().iter().any(|entry| {
        entry.campaign_id() == campaign_id
            && matches!(entry.event(), ConvergenceEvent::EpochOpened(record) if record == epoch)
    }) {
        bail!("verification epoch is absent from the immutable ledger campaign");
    }
    Ok(campaign)
}

fn validate_verifier_output(
    output: &CandidateVerificationOutput,
    candidate: &CandidateRecord,
    epoch: &EpochRecord,
    selected: &AdmittedModelIdentity,
    authority: &CommandAuthoritySnapshot,
) -> Result<()> {
    if output.page.candidate_id != *candidate.id() {
        bail!("verifier response candidate id does not match its request");
    }
    if output.page.stable_finding_id != candidate.stable_finding_id().as_str() {
        bail!("verifier response stable finding id does not match its request");
    }
    if &output.actual_executor != selected || !authority.contains(&output.actual_executor) {
        bail!("verifier actual executor does not match the frozen selected authority");
    }
    if output.artifact.csa_session_id() == candidate.artifact().csa_session_id() {
        bail!("verifier artifact must come from a fresh session, not discovery");
    }
    if output.artifact.path().as_str().is_empty() || epoch.id().as_str().is_empty() {
        bail!("verifier artifact or immutable epoch binding is empty");
    }
    let blocking = matches!(
        &output.page.disposition,
        CandidateDisposition::Verified | CandidateDisposition::NeedsContractOrDocumentation
    );
    if blocking != output.page.repair_scope.is_some() {
        bail!("blocking verifier dispositions require exactly one repair scope");
    }
    if matches!(
        &output.page.disposition,
        CandidateDisposition::Duplicate { .. } | CandidateDisposition::Superseded { .. }
    ) {
        bail!("canonical verifier output must not delegate duplicate or superseded resolution");
    }
    Ok(())
}

struct CanonicalCandidates {
    all: Vec<CandidateRecord>,
    canonical: Vec<CandidateRecord>,
    canonical_by_stable: BTreeMap<String, CandidateId>,
    attempt_executors: HashMap<csa_session::convergence::DiscoveryAttemptId, AdmittedModelIdentity>,
    total_count: usize,
}

fn canonical_candidates(
    ledger: &ConvergenceLedger,
    campaign: &CampaignRecord,
    epoch: &EpochRecord,
) -> Result<CanonicalCandidates> {
    let mut attempt_executors = HashMap::new();
    for entry in ledger.entries() {
        if entry.campaign_id() != campaign.id() {
            continue;
        }
        if let ConvergenceEvent::DiscoveryAttemptRecorded(record) = entry.event()
            && record.epoch_id() == epoch.id()
        {
            attempt_executors.insert(record.id().clone(), record.model_identity().clone());
        }
    }
    let mut all = Vec::new();
    let mut canonical = Vec::new();
    let mut canonical_by_stable = BTreeMap::new();
    for entry in ledger.entries() {
        if entry.campaign_id() != campaign.id() {
            continue;
        }
        let ConvergenceEvent::CandidateRecorded(candidate) = entry.event() else {
            continue;
        };
        if !attempt_executors.contains_key(candidate.discovery_attempt_id()) {
            continue;
        }
        let stable = candidate.stable_finding_id().as_str().to_string();
        if let std::collections::btree_map::Entry::Vacant(entry) = canonical_by_stable.entry(stable)
        {
            entry.insert(candidate.id().clone());
            canonical.push(candidate.clone());
        }
        all.push(candidate.clone());
    }
    if all.is_empty() {
        bail!("immutable discovery epoch contains no candidates to verify");
    }
    Ok(CanonicalCandidates {
        total_count: all.len(),
        all,
        canonical,
        canonical_by_stable,
        attempt_executors,
    })
}

fn terminal_dispositions(
    ledger: &ConvergenceLedger,
    campaign: &CampaignRecord,
    epoch: &EpochRecord,
) -> Result<HashSet<CandidateId>> {
    let known = canonical_candidates(ledger, campaign, epoch)?
        .all
        .into_iter()
        .map(|candidate| candidate.id().clone())
        .collect::<HashSet<_>>();
    let mut dispositions = HashSet::new();
    for entry in ledger.entries() {
        if entry.campaign_id() != campaign.id() {
            continue;
        }
        let ConvergenceEvent::CandidateDispositionRecorded(record) = entry.event() else {
            continue;
        };
        if known.contains(record.candidate_id()) {
            if record.epoch_id() != epoch.id() {
                bail!("candidate disposition is bound to a different immutable epoch");
            }
            if !dispositions.insert(record.candidate_id().clone()) {
                bail!("candidate has more than one terminal disposition");
            }
        }
    }
    Ok(dispositions)
}

pub(crate) fn build_verifier_prompt(request: &CandidateVerificationRequest) -> String {
    let independence = match &request.independence {
        VerificationIndependence::Heterogeneous => "heterogeneous admitted verifier",
        VerificationIndependence::Degraded { reason } => reason,
    };
    format!(
        "Use the csa-review skill. Observe only; do not modify files. Start a fresh analysis and do not resume or request any prior discovery session, transcript, finding list, or continuation state. The only review evidence is immutable bundle `./{}`. Verify its SHA-256 before and after inspection with `sha256sum {}` and require `{}`. Read it with `tar -tf`, `tar -xOf ... manifest.json`, `tar -xOf ... diff.patch`, and `tar -xOf ... source.tar | tar -tf`; do not extract to disk.\n\nThis is one isolated candidate verification. Campaign: {}. Candidate id: {}. Stable semantic id: {}. Frozen epoch: {}. Discovery executor: {}/{}/{}/{}. Independence selection: {}.\n\nReturn exact JSON or one complete json fence and no prose. Schema: {{\"schema_version\":1,\"kind\":\"convergence_candidate_verification\",\"candidate_id\":\"{}\",\"stable_finding_id\":\"{}\",\"disposition\":\"verified|rejected_with_evidence|needs_contract_or_documentation|pre_existing_outside_diff_scope\",\"root_cause_key\":\"required only for verified or needs_contract_or_documentation\",\"corrections\":[],\"regression_tests\":[],\"docs_contracts\":[],\"compatibility_migrations\":[],\"sibling_call_sites\":[]}}. Nonblocking dispositions must use null root_cause_key and empty work arrays.",
        request.frozen.provider_evidence.identity.bundle_file,
        request.frozen.provider_evidence.identity.bundle_file,
        request.frozen.provider_evidence.identity.bundle_digest,
        request.campaign_id,
        request.candidate.id(),
        request.candidate.stable_finding_id(),
        request
            .frozen
            .epoch()
            .expect("frozen workspace validated")
            .id(),
        request.discovery_executor.tool(),
        request.discovery_executor.provider(),
        request.discovery_executor.model(),
        request.discovery_executor.reasoning(),
        independence,
        request.candidate.id(),
        request.candidate.stable_finding_id(),
    )
}

#[cfg(test)]
mod tests {
    use csa_session::convergence::{
        CommandAuthorityCatalogIdentity, CommandAuthorityPolicy, CommandAuthoritySource,
    };
    use serde_json::json;

    use super::*;

    fn identity(tool: &str, model: &str) -> AdmittedModelIdentity {
        AdmittedModelIdentity::new(tool, "test-provider", model, "low").unwrap()
    }

    fn authority(identities: Vec<AdmittedModelIdentity>) -> CommandAuthoritySnapshot {
        CommandAuthoritySnapshot::new(
            CommandAuthoritySource::tier("review", "test fixture").unwrap(),
            CommandAuthorityPolicy::new(true, vec!["review".to_string()], false, false).unwrap(),
            CommandAuthorityCatalogIdentity::new("test catalog", "v1").unwrap(),
            identities,
        )
        .unwrap()
    }

    #[test]
    fn verifier_policy_is_fresh_readonly_and_has_no_discovery_history() {
        let policy = VerifierExecutionPolicy::independent();
        assert!(policy.fresh_session);
        assert!(policy.readonly_project_root);
        assert!(!policy.resumes_discovery_state);
        assert!(!policy.includes_discovery_transcript);
    }

    #[test]
    fn verifier_selection_prefers_heterogeneous_authority_or_records_degradation() {
        let discovery = identity("codex", "discovery");
        let heterogeneous = identity("gemini-cli", "verifier");
        let (selected, independence) = select_verifier(
            &authority(vec![discovery.clone(), heterogeneous.clone()]),
            &discovery,
        )
        .unwrap();
        assert_eq!(selected, heterogeneous);
        assert_eq!(independence, VerificationIndependence::Heterogeneous);

        let (selected, independence) =
            select_verifier(&authority(vec![discovery.clone()]), &discovery).unwrap();
        assert_eq!(selected, discovery);
        assert!(matches!(
            independence,
            VerificationIndependence::Degraded { .. }
        ));
    }

    #[test]
    fn verifier_json_is_exact_and_requires_repair_scope_only_for_blocking_dispositions() {
        let candidate_id = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
        let stable_id = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let valid = json!({
            "schema_version": 1,
            "kind": "convergence_candidate_verification",
            "candidate_id": candidate_id,
            "stable_finding_id": stable_id,
            "disposition": "verified",
            "root_cause_key": "immutable_epoch_mismatch",
            "corrections": ["validate epoch"],
            "regression_tests": ["reject stale epoch"],
            "docs_contracts": [],
            "compatibility_migrations": [],
            "sibling_call_sites": []
        })
        .to_string();
        assert!(parse_verifier_page(&valid).is_ok());
        assert!(parse_verifier_page(&format!("{valid}\ntrailing")).is_err());
        let missing_scope = json!({
            "schema_version": 1,
            "kind": "convergence_candidate_verification",
            "candidate_id": candidate_id,
            "stable_finding_id": stable_id,
            "disposition": "verified",
            "root_cause_key": null,
            "corrections": [],
            "regression_tests": [],
            "docs_contracts": [],
            "compatibility_migrations": [],
            "sibling_call_sites": []
        })
        .to_string();
        assert!(parse_verifier_page(&missing_scope).is_err());
    }
}
