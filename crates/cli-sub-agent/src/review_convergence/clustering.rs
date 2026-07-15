//! Deterministic root-cause clustering of verified blocking candidates.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;

use anyhow::{Context, Result, bail};
use csa_session::convergence::{
    ArtifactEvidenceRef, CampaignRecord, CandidateDisposition, CandidateDispositionRecord,
    CandidateId, CandidateRecord, ConvergenceEvent, ConvergenceLedger, EpochRecord,
    RepairBatchRecord, RootClusterRecord,
};

use super::engine::{EngineError, FrozenWorkspace, LedgerPort, blocked};
use super::verification::{ParsedVerificationPage, decode_verifier_artifact};

/// Reads a durable verifier artifact without launching another provider session.
pub(crate) trait VerificationArtifactReader {
    fn read_verifier_artifact<'a>(
        &'a mut self,
        artifact: &'a ArtifactEvidenceRef,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>>> + 'a>>;
}

/// Complete immutable records created by one clustering pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClusteringSummary {
    pub(crate) root_clusters: usize,
    pub(crate) repair_batches: usize,
    pub(crate) blocking_candidates: usize,
}

#[derive(Default)]
struct ClusterInput {
    candidate_ids: Vec<CandidateId>,
    dispositions: Vec<CandidateDispositionRecord>,
    corrections: Vec<String>,
    regression_tests: Vec<String>,
    docs_contracts: Vec<String>,
    compatibility_migrations: Vec<String>,
    sibling_call_sites: Vec<String>,
}

/// Cluster all and only verified blocking candidates into one batch per root cause.
pub(crate) async fn cluster_verified_findings<S: LedgerPort, R: VerificationArtifactReader>(
    store: &S,
    campaign: &CampaignRecord,
    epoch: &EpochRecord,
    frozen: &FrozenWorkspace,
    reader: &mut R,
) -> std::result::Result<ClusteringSummary, EngineError> {
    let ledger = store
        .load()
        .map_err(|error| blocked("store_failure", format!("{error:#}"), 0))?;
    let inputs = collect_cluster_inputs(&ledger, campaign, epoch, frozen, reader)
        .await
        .map_err(|error| blocked("clustering_evidence_invalid", format!("{error:#}"), 0))?;
    if inputs.is_empty() {
        return Ok(ClusteringSummary {
            root_clusters: 0,
            repair_batches: 0,
            blocking_candidates: 0,
        });
    }
    if ledger.entries().iter().any(|entry| {
        entry.campaign_id() == campaign.id()
            && matches!(
                entry.event(),
                ConvergenceEvent::RootClusterRecorded(_) | ConvergenceEvent::RepairBatchRecorded(_)
            )
    }) {
        return Err(blocked(
            "existing_cluster_records",
            "clustering cannot resume from an incomplete or duplicate root-cluster set",
            0,
        ));
    }
    let (events, summary) = records_for_clusters(epoch, inputs)
        .map_err(|error| blocked("clustering_evidence_invalid", format!("{error:#}"), 0))?;
    store
        .append_batch(campaign.id().clone(), events)
        .map_err(|error| blocked("store_failure", format!("{error:#}"), 0))?;
    Ok(summary)
}

async fn collect_cluster_inputs<R: VerificationArtifactReader>(
    ledger: &ConvergenceLedger,
    campaign: &CampaignRecord,
    epoch: &EpochRecord,
    frozen: &FrozenWorkspace,
    reader: &mut R,
) -> Result<BTreeMap<String, ClusterInput>> {
    let candidates = epoch_candidates(ledger, campaign, epoch)?;
    let dispositions = epoch_dispositions(ledger, campaign, epoch, &candidates)?;
    let mut inputs = BTreeMap::<String, ClusterInput>::new();
    for candidate in candidates.values() {
        let disposition = dispositions
            .get(candidate.id())
            .context("candidate disposition set was incomplete after validation")?;
        let blocking = matches!(
            disposition.disposition(),
            CandidateDisposition::Verified | CandidateDisposition::NeedsContractOrDocumentation
        );
        if !blocking {
            continue;
        }
        let artifact = reader
            .read_verifier_artifact(disposition.artifact())
            .await
            .with_context(|| {
                format!(
                    "read verifier artifact for blocking candidate {}",
                    candidate.id()
                )
            })?;
        let page = decode_verifier_artifact(
            &artifact,
            disposition.artifact().digest(),
            &frozen.provider_evidence.identity,
        )?;
        validate_cluster_page(candidate, disposition, &page)?;
        let scope = page
            .repair_scope
            .context("blocking verifier artifact omitted its repair scope")?;
        let input = inputs.entry(scope.root_cause_key.clone()).or_default();
        input.candidate_ids.push(candidate.id().clone());
        input.dispositions.push(disposition.clone());
        input.corrections.extend(scope.corrections);
        input.regression_tests.extend(scope.regression_tests);
        input.docs_contracts.extend(scope.docs_contracts);
        input
            .compatibility_migrations
            .extend(scope.compatibility_migrations);
        input.sibling_call_sites.extend(scope.sibling_call_sites);
    }
    Ok(inputs)
}

fn epoch_candidates(
    ledger: &ConvergenceLedger,
    campaign: &CampaignRecord,
    epoch: &EpochRecord,
) -> Result<HashMap<CandidateId, CandidateRecord>> {
    let mut candidate_attempts = HashSet::new();
    for entry in ledger.entries() {
        if entry.campaign_id() == campaign.id()
            && let ConvergenceEvent::DiscoveryAttemptRecorded(record) = entry.event()
            && record.epoch_id() == epoch.id()
        {
            candidate_attempts.insert(record.id().clone());
        }
    }
    let mut candidates = HashMap::new();
    for entry in ledger.entries() {
        if entry.campaign_id() != campaign.id() {
            continue;
        }
        let ConvergenceEvent::CandidateRecorded(record) = entry.event() else {
            continue;
        };
        if candidate_attempts.contains(record.discovery_attempt_id())
            && candidates
                .insert(record.id().clone(), record.clone())
                .is_some()
        {
            bail!("candidate was repeated in its immutable epoch");
        }
    }
    if candidates.is_empty() {
        bail!("immutable epoch has no candidates to cluster");
    }
    Ok(candidates)
}

fn epoch_dispositions(
    ledger: &ConvergenceLedger,
    campaign: &CampaignRecord,
    epoch: &EpochRecord,
    candidates: &HashMap<CandidateId, CandidateRecord>,
) -> Result<HashMap<CandidateId, CandidateDispositionRecord>> {
    let mut dispositions = HashMap::new();
    for entry in ledger.entries() {
        if entry.campaign_id() != campaign.id() {
            continue;
        }
        let ConvergenceEvent::CandidateDispositionRecorded(record) = entry.event() else {
            continue;
        };
        if candidates.contains_key(record.candidate_id()) {
            if record.epoch_id() != epoch.id() {
                bail!("terminal disposition belongs to a different immutable epoch");
            }
            if dispositions
                .insert(record.candidate_id().clone(), record.clone())
                .is_some()
            {
                bail!("candidate has conflicting terminal dispositions");
            }
        }
    }
    if dispositions.len() != candidates.len() {
        bail!("every immutable epoch candidate requires exactly one terminal disposition");
    }
    Ok(dispositions)
}

fn validate_cluster_page(
    candidate: &CandidateRecord,
    disposition: &CandidateDispositionRecord,
    page: &ParsedVerificationPage,
) -> Result<()> {
    if page.candidate_id != *candidate.id()
        || page.stable_finding_id != candidate.stable_finding_id().as_str()
        || page.disposition != *disposition.disposition()
    {
        bail!("verifier artifact page does not bind its candidate terminal disposition");
    }
    if !matches!(
        page.disposition,
        CandidateDisposition::Verified | CandidateDisposition::NeedsContractOrDocumentation
    ) {
        bail!("nonblocking verifier artifact was offered to root clustering");
    }
    Ok(())
}

fn records_for_clusters(
    epoch: &EpochRecord,
    inputs: BTreeMap<String, ClusterInput>,
) -> Result<(Vec<ConvergenceEvent>, ClusteringSummary)> {
    let mut events = Vec::with_capacity(inputs.len() * 2);
    let mut covered = HashSet::new();
    let mut blocking_candidates = 0;
    for (root_cause_key, input) in inputs {
        if input.candidate_ids.len() != input.dispositions.len() || input.dispositions.is_empty() {
            bail!("root cluster must bind one terminal disposition for every blocking candidate");
        }
        for candidate_id in &input.candidate_ids {
            if !covered.insert(candidate_id.clone()) {
                bail!("blocking candidate appeared in more than one root cluster");
            }
        }
        blocking_candidates += input.candidate_ids.len();
        let disposition_set_digest = CandidateDispositionRecord::set_digest(&input.dispositions);
        let cluster = RootClusterRecord::new(
            epoch.id().clone(),
            &root_cause_key,
            input.candidate_ids.clone(),
            disposition_set_digest.clone(),
        )?;
        let batch = RepairBatchRecord::new(
            cluster.id().clone(),
            cluster.content_digest().clone(),
            epoch.id().clone(),
            input.candidate_ids,
            disposition_set_digest,
            input.corrections,
            input.regression_tests,
            input.docs_contracts,
            input.compatibility_migrations,
            input.sibling_call_sites,
        )?;
        events.push(ConvergenceEvent::RootClusterRecorded(cluster));
        events.push(ConvergenceEvent::RepairBatchRecorded(batch));
    }
    let root_clusters = events.len() / 2;
    Ok((
        events,
        ClusteringSummary {
            root_clusters,
            repair_batches: root_clusters,
            blocking_candidates,
        },
    ))
}

#[cfg(test)]
mod tests {
    use csa_session::convergence::{
        AdmittedModelIdentity, ArtifactEvidenceRef, CandidateDisposition,
        CandidateDispositionRecord, CandidateId, CandidateVerificationEvidence, CsaSessionId,
        EpochRecord, SessionRelativeArtifactPath, Sha256Digest, VerificationIndependence,
    };

    use super::*;

    #[test]
    fn related_blocking_candidates_form_one_canonical_cluster_and_union_batch() {
        let epoch = EpochRecord::new(
            csa_session::convergence::GitObjectId::parse(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            )
            .unwrap(),
            csa_session::convergence::GitObjectId::parse(
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            )
            .unwrap(),
            Sha256Digest::compute(b"diff"),
        );
        let candidate_a = CandidateId::parse("01ARZ3NDEKTSV4RRFFQ69G5FAV").unwrap();
        let candidate_b = CandidateId::parse("01ARZ3NDEKTSV4RRFFQ69G5FAW").unwrap();
        let evidence = |candidate: CandidateId| {
            CandidateDispositionRecord::new(
                candidate,
                CandidateDisposition::Verified,
                CandidateVerificationEvidence::new(
                    epoch.id().clone(),
                    AdmittedModelIdentity::new("codex", "test", "model", "low").unwrap(),
                    VerificationIndependence::degraded("one").unwrap(),
                    ArtifactEvidenceRef::new(
                        CsaSessionId::parse("01ARZ3NDEKTSV4RRFFQ69G5FAY").unwrap(),
                        SessionRelativeArtifactPath::new("output/verifier.json").unwrap(),
                        Sha256Digest::compute(b"artifact"),
                    ),
                ),
            )
        };
        let input = ClusterInput {
            candidate_ids: vec![candidate_b.clone(), candidate_a.clone()],
            dispositions: vec![evidence(candidate_b), evidence(candidate_a)],
            corrections: vec!["fix b".to_string(), "fix a".to_string()],
            regression_tests: vec!["test b".to_string(), "test a".to_string()],
            docs_contracts: vec!["contract".to_string()],
            compatibility_migrations: Vec::new(),
            sibling_call_sites: vec!["caller".to_string()],
        };
        let (events, summary) =
            records_for_clusters(&epoch, BTreeMap::from([("shared-root".to_string(), input)]))
                .unwrap();
        assert_eq!(summary.root_clusters, 1);
        assert_eq!(summary.repair_batches, 1);
        let ConvergenceEvent::RepairBatchRecorded(batch) = &events[1] else {
            panic!("expected repair batch");
        };
        assert_eq!(batch.corrections(), ["fix a", "fix b"]);
    }

    #[test]
    fn cluster_and_batch_set_digests_bind_every_member_and_work_item() {
        let epoch = EpochRecord::new(
            csa_session::convergence::GitObjectId::parse(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            )
            .unwrap(),
            csa_session::convergence::GitObjectId::parse(
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            )
            .unwrap(),
            Sha256Digest::compute(b"diff"),
        );
        let candidate_a = CandidateId::parse("01ARZ3NDEKTSV4RRFFQ69G5FAV").unwrap();
        let candidate_b = CandidateId::parse("01ARZ3NDEKTSV4RRFFQ69G5FAW").unwrap();
        let evidence = |candidate: CandidateId, artifact_bytes: &'static [u8]| {
            CandidateDispositionRecord::new(
                candidate,
                CandidateDisposition::Verified,
                CandidateVerificationEvidence::new(
                    epoch.id().clone(),
                    AdmittedModelIdentity::new("codex", "test", "model", "low").unwrap(),
                    VerificationIndependence::degraded("one").unwrap(),
                    ArtifactEvidenceRef::new(
                        CsaSessionId::parse("01ARZ3NDEKTSV4RRFFQ69G5FAY").unwrap(),
                        SessionRelativeArtifactPath::new("output/verifier.json").unwrap(),
                        Sha256Digest::compute(artifact_bytes),
                    ),
                ),
            )
        };
        let build = |correction: &str, reverse_members: bool, artifact_bytes: &'static [u8]| {
            let (candidate_ids, dispositions) = if reverse_members {
                (
                    vec![candidate_a.clone(), candidate_b.clone()],
                    vec![
                        evidence(candidate_a.clone(), artifact_bytes),
                        evidence(candidate_b.clone(), artifact_bytes),
                    ],
                )
            } else {
                (
                    vec![candidate_b.clone(), candidate_a.clone()],
                    vec![
                        evidence(candidate_b.clone(), artifact_bytes),
                        evidence(candidate_a.clone(), artifact_bytes),
                    ],
                )
            };
            records_for_clusters(
                &epoch,
                BTreeMap::from([(
                    "shared-root".to_string(),
                    ClusterInput {
                        candidate_ids,
                        dispositions,
                        corrections: vec![correction.to_string()],
                        regression_tests: vec!["test".to_string()],
                        docs_contracts: vec!["contract".to_string()],
                        compatibility_migrations: vec!["migration".to_string()],
                        sibling_call_sites: vec!["sibling".to_string()],
                    },
                )]),
            )
            .unwrap()
            .0
        };
        let first = build("fix one", false, b"artifact");
        let same = build("fix one", true, b"artifact");
        let work_changed = build("fix two", false, b"artifact");
        let disposition_changed = build("fix one", false, b"changed artifact");
        let clusters = |events: &[ConvergenceEvent]| {
            events
                .iter()
                .filter_map(|event| match event {
                    ConvergenceEvent::RootClusterRecorded(cluster) => Some(cluster.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        let batches = |events: &[ConvergenceEvent]| {
            events
                .iter()
                .filter_map(|event| match event {
                    ConvergenceEvent::RepairBatchRecorded(batch) => Some(batch.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            RootClusterRecord::set_digest(&clusters(&first)),
            RootClusterRecord::set_digest(&clusters(&same))
        );
        assert_eq!(
            RepairBatchRecord::set_digest(&batches(&first)),
            RepairBatchRecord::set_digest(&batches(&same))
        );
        assert_ne!(
            RepairBatchRecord::set_digest(&batches(&first)),
            RepairBatchRecord::set_digest(&batches(&work_changed))
        );
        assert_eq!(
            RootClusterRecord::set_digest(&clusters(&first)),
            RootClusterRecord::set_digest(&clusters(&work_changed))
        );
        assert_ne!(
            RootClusterRecord::set_digest(&clusters(&first)),
            RootClusterRecord::set_digest(&clusters(&disposition_changed))
        );
        assert_ne!(
            RepairBatchRecord::set_digest(&batches(&first)),
            RepairBatchRecord::set_digest(&batches(&disposition_changed))
        );
    }
}
