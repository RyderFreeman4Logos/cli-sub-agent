use std::fs;
use std::path::Path;

use anyhow::Result;
use csa_core::types::ReviewDecision;
use csa_session::{
    Finding, FindingsFile, ReviewFinding, ReviewFindingFileRange, ReviewVerdictArtifact, Severity,
    write_findings_toml,
};

use super::artifacts::{
    has_blocking_severity, load_findings_toml_from_output, load_review_artifact_from_output,
    severity_counts_are_zero, severity_counts_for_artifact,
};
use super::clean_detection::review_contains_prose_clean_conclusion;
use super::prose_signals::{
    current_round_review_prose_signals, reconcile_counts_with_prose, review_prose_signals,
};
use super::review_meta_for_verdict_artifact;
use super::text::zero_severity_counts;
use crate::review_cmd::prose_findings::severity_counts_from_review_findings;

const PROSE_FINDINGS_UNPARSED_REASON: &str = "prose_findings_present_but_unparsed";
const SEVERITY_FINDINGS_MISMATCH_REASON: &str = "severity_counts_findings_mismatch";
const EMPTY_FAIL_FINDINGS_ARTIFACT_REASON: &str = "fail_verdict_empty_findings_artifact";
const ARTIFACT_GENERATION_FINDING_ID: &str = "artifact-generation-001";

pub(super) fn enforce_final_verdict_consistency(
    session_dir: &Path,
    artifact: &mut ReviewVerdictArtifact,
) -> Result<(), anyhow::Error> {
    let prose_signals = review_prose_signals(session_dir)?;
    let findings_file = load_findings_toml_from_output(session_dir)?.unwrap_or_default();
    let extraction_confirmed_empty = findings_file.findings.is_empty()
        && session_dir
            .join("output")
            .join(super::super::findings_toml::FINDINGS_TOML_EXTRACTED_MARKER)
            .exists();
    let synthetic_empty = findings_file.findings.is_empty()
        && session_dir
            .join("output")
            .join(super::super::findings_toml::FINDINGS_TOML_SYNTHETIC_MARKER)
            .exists();
    let has_prose_failure_evidence = !prose_signals.findings.is_empty()
        || prose_signals.blocking_summary
        || prose_signals.uncertain_conclusion
        || prose_signals.parsed_findings_sections
        || prose_signals.unparseable_findings_sections
        || prose_signals.cross_dimension_blockers
        || prose_signals.checklist_violation_findings;
    let clean_prose_conclusion = review_contains_prose_clean_conclusion(session_dir)?;
    let repair_prose_signals = current_round_review_prose_signals(session_dir)?;
    let repair_fail_prose_conclusion = repair_prose_signals.fail_conclusion;
    let repair_uncertain_prose_conclusion = repair_prose_signals.uncertain_conclusion;
    let blocking_summary_for_repair = (repair_fail_prose_conclusion || !clean_prose_conclusion)
        && repair_prose_signals.blocking_summary;
    let has_hard_prose_failure_evidence = blocking_summary_for_repair
        || repair_uncertain_prose_conclusion
        || has_blocking_severity(&repair_prose_signals.severity_counts)
        || !repair_prose_signals.findings.is_empty()
        || repair_prose_signals.parsed_findings_sections
        || repair_prose_signals.unparseable_findings_sections
        || repair_prose_signals.cross_dimension_blockers
        || repair_prose_signals.checklist_violation_findings;
    let skip_prose_override =
        (extraction_confirmed_empty || synthetic_empty) && !has_prose_failure_evidence;
    let findings_file = if findings_file.findings.is_empty()
        && !prose_signals.findings.is_empty()
        && !skip_prose_override
    {
        let findings_file = FindingsFile {
            findings: prose_signals.findings.clone(),
        };
        write_findings_toml(session_dir, &findings_file)
            .map_err(|error| anyhow::anyhow!("write prose-derived findings.toml: {error}"))?;
        let marker_path = session_dir
            .join("output")
            .join(super::super::findings_toml::FINDINGS_TOML_SYNTHETIC_MARKER);
        let _ = fs::remove_file(marker_path);
        findings_file
    } else {
        findings_file
    };

    let placeholder_findings_only =
        findings_file_contains_only_empty_fail_placeholder(&findings_file);
    let effective_findings_empty = findings_file.findings.is_empty() || placeholder_findings_only;
    let findings_counts = if placeholder_findings_only {
        zero_severity_counts()
    } else {
        severity_counts_from_review_findings(&findings_file.findings)
    };
    if !skip_prose_override {
        artifact.severity_counts =
            reconcile_counts_with_prose(artifact.severity_counts.clone(), &findings_counts);
        artifact.severity_counts = reconcile_counts_with_prose(
            artifact.severity_counts.clone(),
            &prose_signals.severity_counts,
        );
    }

    let prose_grade = highest_prose_severity_grade(session_dir);

    let resume_to_fix = has_resume_to_fix_suggestion(session_dir)?;
    let review_artifact = load_review_artifact_from_output(session_dir)?;
    let review_artifact_findings: &[Finding] = review_artifact
        .as_ref()
        .map(|artifact| artifact.findings.as_slice())
        .unwrap_or(&[]);
    let review_artifact_counts = review_artifact
        .as_ref()
        .map(|artifact| severity_counts_for_artifact(artifact, zero_severity_counts));
    let review_artifact_has_severity_counts = review_artifact_counts
        .as_ref()
        .is_some_and(|counts| !severity_counts_are_zero(counts));
    let review_artifact_has_blocking_risk = review_artifact.as_ref().is_some_and(|artifact| {
        artifact.overall_risk.as_deref().is_some_and(|risk| {
            risk.eq_ignore_ascii_case("high") || risk.eq_ignore_ascii_case("critical")
        })
    });
    let has_review_artifact_findings = !review_artifact_findings.is_empty();
    let has_structured_findings = !effective_findings_empty || has_review_artifact_findings;
    let structured_mismatch =
        !severity_counts_are_zero(&artifact.severity_counts) && !has_structured_findings;
    let blocking_prose =
        prose_signals.blocking_summary || has_blocking_severity(&prose_signals.severity_counts);
    let uncertain_prose = prose_signals.uncertain_conclusion;
    let blocking_structured = !severity_counts_are_zero(&artifact.severity_counts);
    let parsed_findings_prose = prose_signals.parsed_findings_sections;
    let unparsed_findings_prose = prose_signals.unparseable_findings_sections;
    let cross_dimension_blocker = prose_signals.cross_dimension_blockers;
    let checklist_violation_mismatch = prose_signals.checklist_violation_findings
        && !has_structured_findings
        && severity_counts_are_zero(&artifact.severity_counts);
    let cross_dimension_blocker_mismatch = cross_dimension_blocker
        && !has_structured_findings
        && severity_counts_are_zero(&artifact.severity_counts);
    let resume_to_fix_blocks_clean_recovery = resume_to_fix
        && !artifact_failure_reason_is_placeholder(artifact.failure_reason.as_deref())
        && !placeholder_findings_only;

    if clean_review_can_recover_to_pass(
        artifact,
        CleanReviewRecoverySignals {
            artifact_counts_clean: severity_counts_are_zero(&artifact.severity_counts)
                || artifact_failure_reason_is_placeholder(artifact.failure_reason.as_deref())
                || placeholder_findings_only,
            has_structured_findings,
            has_prose_failure_evidence: has_hard_prose_failure_evidence,
            resume_to_fix: resume_to_fix_blocks_clean_recovery,
            review_artifact_has_fail_signal: review_artifact_has_severity_counts
                || review_artifact_has_blocking_risk,
            clean_prose_conclusion,
            fail_prose_conclusion: repair_fail_prose_conclusion,
            uncertain_prose_conclusion: repair_uncertain_prose_conclusion,
        },
    ) {
        recover_clean_review_to_pass(session_dir, artifact)?;
        return Ok(());
    }

    if unparsed_findings_prose || checklist_violation_mismatch || cross_dimension_blocker_mismatch {
        artifact
            .failure_reason
            .get_or_insert_with(|| PROSE_FINDINGS_UNPARSED_REASON.to_string());
    }
    if structured_mismatch {
        artifact
            .failure_reason
            .get_or_insert_with(|| SEVERITY_FINDINGS_MISMATCH_REASON.to_string());
    }

    if artifact.decision == ReviewDecision::Pass
        && !skip_prose_override
        && (resume_to_fix
            || blocking_prose
            || blocking_structured
            || parsed_findings_prose
            || unparsed_findings_prose
            || cross_dimension_blocker
            || checklist_violation_mismatch
            || cross_dimension_blocker_mismatch
            || structured_mismatch)
    {
        artifact.decision = ReviewDecision::Fail;
        artifact.verdict_legacy = "HAS_ISSUES".to_string();
    }
    if artifact.decision == ReviewDecision::Pass && !skip_prose_override && uncertain_prose {
        artifact.decision = ReviewDecision::Uncertain;
        artifact.verdict_legacy = "UNCERTAIN".to_string();
    }

    // #1852: a Fail verdict must carry a severity count that reflects the
    // reviewer's stated prose GRADE. The structured findings can be empty (a
    // failed-over/degraded reviewer whose machine-readable block never
    // persisted) or under-grade the prose (a real `[HIGH]` whose
    // backtick-wrapped tag the structured finding parsers skip). Grade the
    // fail-closed placeholder by the highest legible prose severity — defaulting
    // to MEDIUM only when no grade is legible — and never downgrade an existing
    // higher count.
    if artifact.decision == ReviewDecision::Fail {
        ensure_fail_closed_grade(&mut artifact.severity_counts, prose_grade);
        ensure_failed_verdict_findings_artifact(
            session_dir,
            artifact,
            &findings_file,
            &prose_signals.findings,
            review_artifact_findings,
        )?;
    }

    Ok(())
}

pub(crate) fn repair_clean_empty_fail_review_verdict(session_dir: &Path) -> Result<bool> {
    let verdict_path = session_dir.join("output").join("review-verdict.json");
    if !verdict_path.is_file() {
        return Ok(false);
    }
    let meta = read_review_meta(session_dir)?;
    if meta
        .as_ref()
        .is_some_and(review_meta_has_hard_failure_evidence)
    {
        return Ok(false);
    }
    let raw = fs::read_to_string(&verdict_path)
        .map_err(|error| anyhow::anyhow!("read {}: {error}", verdict_path.display()))?;
    let mut artifact: ReviewVerdictArtifact = serde_json::from_str(&raw)
        .map_err(|error| anyhow::anyhow!("parse {}: {error}", verdict_path.display()))?;
    let original_artifact = artifact.clone();
    enforce_final_verdict_consistency(session_dir, &mut artifact)?;
    if artifact == original_artifact {
        return Ok(false);
    }

    csa_session::write_review_verdict(session_dir, &artifact)
        .map_err(|error| anyhow::anyhow!("write {}: {error}", verdict_path.display()))?;
    if let Some(meta) = meta {
        let final_meta = review_meta_for_verdict_artifact(&meta, &artifact);
        write_review_meta_preserving_extra(session_dir, &final_meta)?;
    }
    Ok(true)
}

fn read_review_meta(session_dir: &Path) -> Result<Option<csa_session::ReviewSessionMeta>> {
    let path = session_dir.join("review_meta.json");
    if !path.is_file() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path)
        .map_err(|error| anyhow::anyhow!("read {}: {error}", path.display()))?;
    let meta = serde_json::from_str(&raw)
        .map_err(|error| anyhow::anyhow!("parse {}: {error}", path.display()))?;
    Ok(Some(meta))
}

fn write_review_meta_preserving_extra(
    session_dir: &Path,
    meta: &csa_session::ReviewSessionMeta,
) -> Result<()> {
    let path = session_dir.join("review_meta.json");
    let meta_value = serde_json::to_value(meta)?;
    let mut value = fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    if let (Some(existing), Some(updated)) = (value.as_object_mut(), meta_value.as_object()) {
        for (key, new_value) in updated {
            existing.insert(key.clone(), new_value.clone());
        }
        if meta.decision == csa_core::types::ReviewDecision::Pass.as_str() {
            remove_clean_pass_failure_keys(existing);
        }
    } else {
        value = meta_value;
    }
    let json = serde_json::to_string_pretty(&value)?;
    fs::write(&path, json).map_err(|error| anyhow::anyhow!("write {}: {error}", path.display()))
}

fn remove_clean_pass_failure_keys(existing: &mut serde_json::Map<String, serde_json::Value>) {
    for key in ["status_reason", "primary_failure", "failure_reason"] {
        existing.remove(key);
    }
}

struct CleanReviewRecoverySignals {
    artifact_counts_clean: bool,
    has_structured_findings: bool,
    has_prose_failure_evidence: bool,
    resume_to_fix: bool,
    review_artifact_has_fail_signal: bool,
    clean_prose_conclusion: bool,
    fail_prose_conclusion: bool,
    uncertain_prose_conclusion: bool,
}

fn clean_review_can_recover_to_pass(
    artifact: &ReviewVerdictArtifact,
    signals: CleanReviewRecoverySignals,
) -> bool {
    if !matches!(
        artifact.decision,
        ReviewDecision::Fail | ReviewDecision::Uncertain
    ) {
        return false;
    }
    if !signals.artifact_counts_clean
        || signals.has_structured_findings
        || signals.has_prose_failure_evidence
        || signals.resume_to_fix
        || signals.review_artifact_has_fail_signal
        || artifact_has_hard_failure_evidence(artifact)
    {
        return false;
    }
    if signals.fail_prose_conclusion || signals.uncertain_prose_conclusion {
        return false;
    }
    signals.clean_prose_conclusion
}

fn recover_clean_review_to_pass(
    session_dir: &Path,
    artifact: &mut ReviewVerdictArtifact,
) -> Result<(), anyhow::Error> {
    artifact.decision = ReviewDecision::Pass;
    artifact.verdict_legacy = "CLEAN".to_string();
    artifact.severity_counts = zero_severity_counts();
    artifact.primary_failure = None;
    artifact.failure_reason = None;
    artifact.no_provider_launch = None;
    write_findings_toml(session_dir, &FindingsFile::default())
        .map_err(|error| anyhow::anyhow!("write recovered clean findings.toml: {error}"))?;
    clear_empty_findings_markers(session_dir);
    Ok(())
}

fn artifact_has_hard_failure_evidence(artifact: &ReviewVerdictArtifact) -> bool {
    artifact.no_provider_launch.is_some()
        || non_empty(artifact.primary_failure.as_deref()).is_some()
        || artifact
            .failure_reason
            .as_deref()
            .and_then(non_empty_str)
            .is_some_and(|reason| !artifact_failure_reason_is_placeholder(Some(reason)))
}

fn review_meta_has_hard_failure_evidence(meta: &csa_session::ReviewSessionMeta) -> bool {
    if matches!(
        meta.decision.parse::<ReviewDecision>(),
        Ok(ReviewDecision::Unavailable) | Err(_)
    ) {
        return true;
    }
    if non_empty(meta.status_reason.as_deref()).is_some()
        || non_empty(meta.primary_failure.as_deref()).is_some()
        || meta
            .failure_reason
            .as_deref()
            .and_then(non_empty_str)
            .is_some_and(|reason| !artifact_failure_reason_is_placeholder(Some(reason)))
    {
        return true;
    }
    meta.fix_attempted && !meta.fix_clean_converged()
}

fn artifact_failure_reason_is_placeholder(reason: Option<&str>) -> bool {
    reason.is_some_and(|reason| reason.trim() == EMPTY_FAIL_FINDINGS_ARTIFACT_REASON)
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.and_then(non_empty_str)
}

fn non_empty_str(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

fn findings_file_contains_only_empty_fail_placeholder(findings_file: &FindingsFile) -> bool {
    let [finding] = findings_file.findings.as_slice() else {
        return false;
    };
    finding.id == ARTIFACT_GENERATION_FINDING_ID
        && finding.file_ranges.is_empty()
        && finding
            .description
            .contains(EMPTY_FAIL_FINDINGS_ARTIFACT_REASON)
}

fn ensure_failed_verdict_findings_artifact(
    session_dir: &Path,
    artifact: &mut ReviewVerdictArtifact,
    findings_file: &FindingsFile,
    prose_findings: &[ReviewFinding],
    review_artifact_findings: &[Finding],
) -> Result<(), anyhow::Error> {
    if !findings_file.findings.is_empty() {
        return Ok(());
    }

    let backfilled_findings = if !prose_findings.is_empty() {
        prose_findings.to_vec()
    } else if !review_artifact_findings.is_empty() {
        review_artifact_findings
            .iter()
            .enumerate()
            .map(|(index, finding)| review_artifact_finding_to_findings_toml(finding, index + 1))
            .collect()
    } else {
        artifact
            .failure_reason
            .get_or_insert_with(|| EMPTY_FAIL_FINDINGS_ARTIFACT_REASON.to_string());
        vec![artifact_generation_failure_finding(artifact)]
    };

    write_findings_toml(
        session_dir,
        &FindingsFile {
            findings: backfilled_findings,
        },
    )
    .map_err(|error| anyhow::anyhow!("write fail-closed findings.toml: {error}"))?;
    clear_empty_findings_markers(session_dir);
    Ok(())
}

fn review_artifact_finding_to_findings_toml(finding: &Finding, index: usize) -> ReviewFinding {
    let file_ranges = finding
        .line
        .filter(|_| !finding.file.trim().is_empty())
        .map(|start| ReviewFindingFileRange {
            path: finding.file.clone(),
            start,
            end: None,
        })
        .into_iter()
        .collect();

    ReviewFinding {
        id: non_empty_or_else(&finding.fid, || format!("review-findings-{index:03}")),
        severity: finding.severity.clone(),
        file_ranges,
        is_regression_of_commit: None,
        suggested_test_scenario: None,
        description: non_empty_or_else(&finding.summary, || {
            "Review finding imported from review-findings.json".to_string()
        }),
    }
}

fn artifact_generation_failure_finding(artifact: &ReviewVerdictArtifact) -> ReviewFinding {
    let reason = artifact
        .failure_reason
        .as_deref()
        .unwrap_or(EMPTY_FAIL_FINDINGS_ARTIFACT_REASON);
    ReviewFinding {
        id: ARTIFACT_GENERATION_FINDING_ID.to_string(),
        severity: highest_counted_severity(&artifact.severity_counts).unwrap_or(Severity::Medium),
        file_ranges: Vec::new(),
        is_regression_of_commit: None,
        suggested_test_scenario: None,
        description: format!(
            "Artifact generation failed: review verdict is FAIL but CSA could not extract a structured finding. Reason: {reason}. Inspect output/details.md and output/review-verdict.json."
        ),
    }
}

fn highest_counted_severity(
    severity_counts: &std::collections::BTreeMap<Severity, u32>,
) -> Option<Severity> {
    severity_counts
        .iter()
        .filter_map(|(severity, count)| (*count > 0).then_some(severity.clone()))
        .max()
}

fn non_empty_or_else(value: &str, fallback: impl FnOnce() -> String) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        fallback()
    } else {
        trimmed.to_string()
    }
}

fn clear_empty_findings_markers(session_dir: &Path) {
    for marker in [
        super::super::findings_toml::FINDINGS_TOML_SYNTHETIC_MARKER,
        super::super::findings_toml::FINDINGS_TOML_EXTRACTED_MARKER,
    ] {
        let _ = fs::remove_file(session_dir.join("output").join(marker));
    }
}

/// Ensure a fail-closed verdict's severity counts reflect the reviewer's prose
/// GRADE. With zero counts, inject one placeholder at `prose_grade` (or MEDIUM
/// when no grade is legible). With non-zero counts whose highest graded entry is
/// below `prose_grade`, add the prose grade so a real `[HIGH]` is never reported
/// as a mergeable MEDIUM (#1852). Never downgrades and never inflates a matching
/// or already-higher existing grade.
fn ensure_fail_closed_grade(
    severity_counts: &mut std::collections::BTreeMap<Severity, u32>,
    prose_grade: Option<Severity>,
) {
    if severity_counts_are_zero(severity_counts) {
        let severity = prose_grade.unwrap_or(Severity::Medium);
        *severity_counts.entry(severity).or_insert(0) += 1;
        return;
    }
    let Some(grade) = prose_grade else {
        return;
    };
    let already_at_or_above = severity_counts
        .iter()
        .any(|(severity, count)| *count > 0 && *severity >= grade);
    if !already_at_or_above {
        *severity_counts.entry(grade).or_insert(0) += 1;
    }
}

/// Highest reviewer-assigned severity GRADE legible in the canonical review
/// text, tolerant of markdown inline-code backticks around the tag (e.g.
/// `` `[HIGH]` ``). The structured finding parsers require the bracket to start
/// the body and therefore skip backtick-wrapped tags, so the fail-closed grader
/// consults this to avoid under-grading a real HIGH whose machine-readable
/// findings failed to parse (#1852). Returns `None` when no bracketed severity
/// tag is present. Best-effort: unreadable review text yields `None` (callers
/// fall back to MEDIUM), never an error.
fn highest_prose_severity_grade(session_dir: &Path) -> Option<Severity> {
    let review_text = crate::review_cmd::findings_toml::load_canonical_review_text(session_dir)
        .ok()
        .flatten()?;
    let mut best: Option<Severity> = None;
    let mut in_code_fence = false;
    for line in review_text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_code_fence = !in_code_fence;
            continue;
        }
        if in_code_fence {
            continue;
        }
        for severity in bracketed_severities_in_line(trimmed) {
            best = Some(match best {
                Some(current) => current.max(severity),
                None => severity,
            });
        }
    }
    best
}

/// Every bracketed severity label on a line, mapping `[HIGH]`/`[p1]`/... to a
/// [`Severity`]. Scans the `[label]` delimiters directly so adjacent markdown
/// backticks (`` `[HIGH]` ``) do not hide the tag. Non-severity brackets (e.g.
/// `[security/correctness]`) yield nothing.
fn bracketed_severities_in_line(line: &str) -> impl Iterator<Item = Severity> + '_ {
    line.match_indices('[').filter_map(|(open, _)| {
        let rest = line.get(open + 1..)?;
        let close = rest.find(']')?;
        crate::review_cmd::prose_findings::severity_from_label(rest.get(..close)?)
    })
}

fn has_resume_to_fix_suggestion(session_dir: &Path) -> Result<bool, anyhow::Error> {
    let suggestion_path = session_dir.join("output").join("suggestion.toml");
    if !suggestion_path.exists() {
        return Ok(false);
    }
    let contents = fs::read_to_string(&suggestion_path)
        .map_err(|error| anyhow::anyhow!("read {}: {error}", suggestion_path.display()))?;
    let value = toml::from_str::<toml::Value>(&contents)
        .map_err(|error| anyhow::anyhow!("parse {}: {error}", suggestion_path.display()))?;
    let action = value
        .get("suggestion")
        .and_then(|suggestion| suggestion.get("action"))
        .and_then(toml::Value::as_str);
    Ok(matches!(
        action,
        Some("resume_to_fix" | "confirm_then_fix_finding")
    ))
}
