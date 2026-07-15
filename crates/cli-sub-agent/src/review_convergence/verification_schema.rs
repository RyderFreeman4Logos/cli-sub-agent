//! Strict wire and artifact schemas for candidate verification.

use anyhow::{Context, Result, bail};
use csa_session::convergence::{CandidateDisposition, CandidateId, Sha256Digest};
use serde::{Deserialize, Serialize};

use super::bundle::ProviderEvidenceIdentity;
use super::verification::{ParsedVerificationPage, VerifiedRepairScope};

const VERIFIER_SCHEMA_VERSION: u32 = 1;
pub(crate) const VERIFIER_ARTIFACT_PATH: &str = "output/convergence-candidate-verification.json";
pub(crate) const VERIFIER_ARTIFACT_FILE: &str = "convergence-candidate-verification.json";
const VERIFIER_ARTIFACT_SCHEMA_VERSION: u32 = 1;
const VERIFIER_ARTIFACT_KIND: &str = "convergence_candidate_verification";

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct VerifierArtifactEnvelope {
    schema_version: u32,
    kind: String,
    provider_evidence: ProviderEvidenceIdentity,
    provider_response_raw: String,
    parsed_page: ParsedVerificationPageArtifact,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ParsedVerificationPageArtifact {
    candidate_id: CandidateId,
    stable_finding_id: String,
    disposition: CandidateDisposition,
    repair_scope: Option<VerifiedRepairScopeArtifact>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct VerifiedRepairScopeArtifact {
    root_cause_key: String,
    corrections: Vec<String>,
    regression_tests: Vec<String>,
    docs_contracts: Vec<String>,
    compatibility_migrations: Vec<String>,
    sibling_call_sites: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum RawVerifierKind {
    ConvergenceCandidateVerification,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum RawVerifierDisposition {
    Verified,
    RejectedWithEvidence,
    NeedsContractOrDocumentation,
    PreExistingOutsideDiffScope,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawVerifierPage {
    schema_version: u32,
    kind: RawVerifierKind,
    candidate_id: CandidateId,
    stable_finding_id: String,
    disposition: RawVerifierDisposition,
    root_cause_key: Option<String>,
    corrections: Vec<String>,
    regression_tests: Vec<String>,
    docs_contracts: Vec<String>,
    compatibility_migrations: Vec<String>,
    sibling_call_sites: Vec<String>,
}

/// Parse the sole accepted strict verifier JSON envelope.
pub(crate) fn parse_verifier_page(raw: &str) -> Result<ParsedVerificationPage> {
    let json = complete_json(raw, "verifier response")?;
    let mut deserializer = serde_json::Deserializer::from_str(json);
    let raw = RawVerifierPage::deserialize(&mut deserializer).context("invalid verifier JSON")?;
    deserializer
        .end()
        .context("verifier response contains trailing content")?;
    if raw.schema_version != VERIFIER_SCHEMA_VERSION
        || !matches!(raw.kind, RawVerifierKind::ConvergenceCandidateVerification)
    {
        bail!("unsupported verifier response schema or kind");
    }
    if raw.stable_finding_id.trim().is_empty()
        || raw.stable_finding_id.trim() != raw.stable_finding_id.as_str()
    {
        bail!("verifier stable finding id must be nonblank and normalized");
    }
    let disposition = match raw.disposition {
        RawVerifierDisposition::Verified => CandidateDisposition::Verified,
        RawVerifierDisposition::RejectedWithEvidence => CandidateDisposition::RejectedWithEvidence,
        RawVerifierDisposition::NeedsContractOrDocumentation => {
            CandidateDisposition::NeedsContractOrDocumentation
        }
        RawVerifierDisposition::PreExistingOutsideDiffScope => {
            CandidateDisposition::PreExistingOutsideDiffScope
        }
    };
    let blocking = matches!(
        disposition,
        CandidateDisposition::Verified | CandidateDisposition::NeedsContractOrDocumentation
    );
    let repair_scope = if blocking {
        Some(VerifiedRepairScope {
            root_cause_key: canonical_nonblank(
                "verifier root cause key",
                raw.root_cause_key.as_deref(),
            )?,
            corrections: canonical_work("verifier correction", raw.corrections)?,
            regression_tests: canonical_work("verifier regression test", raw.regression_tests)?,
            docs_contracts: canonical_work("verifier docs or contract", raw.docs_contracts)?,
            compatibility_migrations: canonical_work(
                "verifier compatibility or migration",
                raw.compatibility_migrations,
            )?,
            sibling_call_sites: canonical_work(
                "verifier sibling call site",
                raw.sibling_call_sites,
            )?,
        })
    } else {
        if raw.root_cause_key.is_some()
            || !raw.corrections.is_empty()
            || !raw.regression_tests.is_empty()
            || !raw.docs_contracts.is_empty()
            || !raw.compatibility_migrations.is_empty()
            || !raw.sibling_call_sites.is_empty()
        {
            bail!("nonblocking verifier disposition must not carry repair work");
        }
        None
    };
    Ok(ParsedVerificationPage {
        candidate_id: raw.candidate_id,
        stable_finding_id: raw.stable_finding_id,
        disposition,
        repair_scope,
    })
}

pub(crate) fn encode_verifier_artifact(
    raw_response: &str,
    page: &ParsedVerificationPage,
    provider_evidence: &ProviderEvidenceIdentity,
) -> Result<Vec<u8>> {
    serde_json::to_vec(&VerifierArtifactEnvelope {
        schema_version: VERIFIER_ARTIFACT_SCHEMA_VERSION,
        kind: VERIFIER_ARTIFACT_KIND.to_string(),
        provider_evidence: provider_evidence.clone(),
        provider_response_raw: raw_response.to_string(),
        parsed_page: page_to_artifact(page),
    })
    .context("serialize candidate verifier artifact")
}

pub(crate) fn decode_verifier_artifact(
    artifact: &[u8],
    expected_digest: &Sha256Digest,
    expected_provider_evidence: &ProviderEvidenceIdentity,
) -> Result<ParsedVerificationPage> {
    if &Sha256Digest::compute(artifact) != expected_digest {
        bail!("candidate verifier artifact digest mismatch");
    }
    let envelope: VerifierArtifactEnvelope =
        serde_json::from_slice(artifact).context("parse candidate verifier artifact")?;
    if envelope.schema_version != VERIFIER_ARTIFACT_SCHEMA_VERSION
        || envelope.kind != VERIFIER_ARTIFACT_KIND
        || &envelope.provider_evidence != expected_provider_evidence
    {
        bail!("candidate verifier artifact identity mismatch");
    }
    let reparsed = parse_verifier_page(&envelope.provider_response_raw)
        .context("parse raw response embedded in candidate verifier artifact")?;
    let stored = page_from_artifact(envelope.parsed_page);
    if reparsed != stored {
        bail!("candidate verifier artifact parsed response mismatch");
    }
    Ok(reparsed)
}

fn page_to_artifact(page: &ParsedVerificationPage) -> ParsedVerificationPageArtifact {
    ParsedVerificationPageArtifact {
        candidate_id: page.candidate_id.clone(),
        stable_finding_id: page.stable_finding_id.clone(),
        disposition: page.disposition.clone(),
        repair_scope: page
            .repair_scope
            .as_ref()
            .map(|scope| VerifiedRepairScopeArtifact {
                root_cause_key: scope.root_cause_key.clone(),
                corrections: scope.corrections.clone(),
                regression_tests: scope.regression_tests.clone(),
                docs_contracts: scope.docs_contracts.clone(),
                compatibility_migrations: scope.compatibility_migrations.clone(),
                sibling_call_sites: scope.sibling_call_sites.clone(),
            }),
    }
}

fn page_from_artifact(page: ParsedVerificationPageArtifact) -> ParsedVerificationPage {
    ParsedVerificationPage {
        candidate_id: page.candidate_id,
        stable_finding_id: page.stable_finding_id,
        disposition: page.disposition,
        repair_scope: page.repair_scope.map(|scope| VerifiedRepairScope {
            root_cause_key: scope.root_cause_key,
            corrections: scope.corrections,
            regression_tests: scope.regression_tests,
            docs_contracts: scope.docs_contracts,
            compatibility_migrations: scope.compatibility_migrations,
            sibling_call_sites: scope.sibling_call_sites,
        }),
    }
}

fn complete_json<'a>(raw: &'a str, label: &str) -> Result<&'a str> {
    if raw.starts_with("```json\n") {
        let body = raw
            .strip_prefix("```json\n")
            .context("missing JSON fence opener")?;
        body.strip_suffix("\n```\n")
            .or_else(|| body.strip_suffix("\n```"))
            .with_context(|| {
                format!("{label} must contain one complete JSON fence with no trailing prose")
            })
    } else {
        Ok(raw)
    }
}

fn canonical_nonblank(field: &str, value: Option<&str>) -> Result<String> {
    let value = value.context(format!(
        "{field} is required for a blocking verifier disposition"
    ))?;
    if value.is_empty() || value.trim() != value || value.contains('\0') {
        bail!("{field} must be nonblank and normalized");
    }
    Ok(value.to_string())
}

fn canonical_work(field: &str, values: Vec<String>) -> Result<Vec<String>> {
    let mut values = values
        .into_iter()
        .map(|value| canonical_nonblank(field, Some(&value)))
        .collect::<Result<Vec<_>>>()?;
    values.sort_unstable();
    if values.windows(2).any(|pair| pair[0] == pair[1]) {
        bail!("{field} values must be unique");
    }
    Ok(values)
}
