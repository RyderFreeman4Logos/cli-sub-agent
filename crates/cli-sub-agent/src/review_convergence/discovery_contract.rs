use std::collections::HashSet;
use std::path::{Component, Path};

use anyhow::{Context, Result, bail};
use csa_session::convergence::{
    AdmittedModelIdentity, ArtifactEvidenceRef, CampaignId, CoverageCellRecord, DiscoveryRunIntent,
    SemanticFindingIdentity, StableFindingId,
};
use serde::{Deserialize, Serialize};

use super::continuation::ContinuationEvidence;
#[cfg(test)]
use super::coverage::{CoverageManifestPlan, plan_coverage_manifest};
#[cfg(test)]
use super::engine::PAGE_CANDIDATE_LIMIT;
use super::engine::{FrozenWorkspace, PhaseTimings};

const CLEAN_ROOM_SCHEMA_VERSION: u32 = 1;
const MAX_FINDINGS: usize = 64;

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
        if review.findings.is_empty() {
            bail!("targeted discovery requires at least one clean-room finding");
        }
        if !review.questions.is_empty() || !review.unchecked_items.is_empty() {
            bail!("targeted discovery rejects questions or unchecked items");
        }
        Ok(Self {
            artifact: review.artifact.clone(),
            semantic_finding_ids: review
                .findings
                .iter()
                .map(|finding| finding.stable_id.clone())
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CleanRoomSeverity {
    Blocker,
    Critical,
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CleanRoomFinding {
    stable_id: StableFindingId,
    semantic_identity: SemanticFindingIdentity,
    path: String,
    start_line: u32,
    end_line: u32,
    category: String,
    severity: CleanRoomSeverity,
    summary: String,
    evidence: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CleanRoomReviewOutput {
    artifact: ArtifactEvidenceRef,
    model_identity: AdmittedModelIdentity,
    findings: Vec<CleanRoomFinding>,
    questions: Vec<String>,
    unchecked_items: Vec<String>,
}

impl CleanRoomReviewOutput {
    pub(crate) fn artifact(&self) -> &ArtifactEvidenceRef {
        &self.artifact
    }

    pub(crate) fn model_identity(&self) -> &AdmittedModelIdentity {
        &self.model_identity
    }

    pub(crate) fn findings(&self) -> &[CleanRoomFinding] {
        &self.findings
    }

    pub(crate) fn questions(&self) -> &[String] {
        &self.questions
    }

    pub(crate) fn unchecked_items(&self) -> &[String] {
        &self.unchecked_items
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum RawKind {
    ConvergenceCleanRoomReview,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSpan {
    start_line: u32,
    end_line: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFinding {
    semantic_identity: SemanticFindingIdentity,
    path: String,
    span: RawSpan,
    category: String,
    severity: CleanRoomSeverity,
    summary: String,
    evidence: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawOutput {
    schema_version: u32,
    kind: RawKind,
    artifact: ArtifactEvidenceRef,
    model_identity: AdmittedModelIdentity,
    findings: Vec<RawFinding>,
    questions: Vec<String>,
    unchecked_items: Vec<String>,
}

pub(crate) fn parse_clean_room_review_output(raw: &str) -> Result<CleanRoomReviewOutput> {
    let mut deserializer = serde_json::Deserializer::from_str(raw);
    let parsed =
        RawOutput::deserialize(&mut deserializer).context("invalid clean-room review JSON")?;
    deserializer
        .end()
        .context("clean-room review contains trailing content")?;
    if parsed.schema_version != CLEAN_ROOM_SCHEMA_VERSION
        || !matches!(parsed.kind, RawKind::ConvergenceCleanRoomReview)
    {
        bail!("unsupported clean-room review schema or kind");
    }
    if parsed.findings.len() > MAX_FINDINGS {
        bail!("clean-room finding count exceeds the bounded maximum");
    }
    let questions = bounded_list("clean-room question", parsed.questions, 32, 500)?;
    let unchecked_items =
        bounded_list("clean-room unchecked item", parsed.unchecked_items, 32, 500)?;
    let mut identities = HashSet::new();
    let mut findings = Vec::with_capacity(parsed.findings.len());
    for finding in parsed.findings {
        let stable_id = StableFindingId::compute(&finding.semantic_identity);
        if !identities.insert(stable_id.as_str().to_string()) {
            bail!("duplicate semantic identity in clean-room review");
        }
        validate_path(&finding.path)?;
        if finding.span.start_line == 0
            || finding.span.start_line > finding.span.end_line
            || finding.span.end_line > 1_000_000
        {
            bail!("clean-room finding span is invalid");
        }
        let category = bounded_token("clean-room category", &finding.category, 64)?;
        let summary = bounded_text("clean-room summary", &finding.summary, 500)?;
        let evidence = bounded_text("clean-room evidence", &finding.evidence, 2_000)?;
        findings.push(CleanRoomFinding {
            stable_id,
            semantic_identity: finding.semantic_identity,
            path: finding.path,
            start_line: finding.span.start_line,
            end_line: finding.span.end_line,
            category,
            severity: finding.severity,
            summary,
            evidence,
        });
    }
    Ok(CleanRoomReviewOutput {
        artifact: parsed.artifact,
        model_identity: parsed.model_identity,
        findings,
        questions,
        unchecked_items,
    })
}

fn validate_path(value: &str) -> Result<()> {
    bounded_text("clean-room path", value, 512)?;
    let path = Path::new(value);
    if path.is_absolute() || value.contains('\\') {
        bail!("clean-room path must be a normalized relative path");
    }
    let normalized = path
        .components()
        .map(|component| match component {
            Component::Normal(segment) => Ok(segment.to_string_lossy()),
            _ => bail!("clean-room path must be a normalized relative path"),
        })
        .collect::<Result<Vec<_>>>()?
        .join("/");
    if normalized != value {
        bail!("clean-room path must be a normalized relative path");
    }
    Ok(())
}

fn bounded_token(label: &str, value: &str, maximum: usize) -> Result<String> {
    let value = bounded_text(label, value, maximum)?;
    if !value.bytes().all(|byte| {
        byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
    }) {
        bail!("{label} must contain only lowercase ASCII token characters");
    }
    Ok(value)
}

fn bounded_text(label: &str, value: &str, maximum: usize) -> Result<String> {
    if value.is_empty()
        || value.trim() != value
        || value.chars().any(char::is_control)
        || value.len() > maximum
    {
        bail!("{label} must be nonblank, normalized, and at most {maximum} bytes");
    }
    Ok(value.to_string())
}

fn bounded_list(
    label: &str,
    values: Vec<String>,
    maximum: usize,
    width: usize,
) -> Result<Vec<String>> {
    if values.len() > maximum {
        bail!("{label} count exceeds the bounded maximum");
    }
    let mut seen = HashSet::new();
    for value in &values {
        bounded_text(label, value, width)?;
        if !seen.insert(value) {
            bail!("duplicate {label}");
        }
    }
    Ok(values)
}
