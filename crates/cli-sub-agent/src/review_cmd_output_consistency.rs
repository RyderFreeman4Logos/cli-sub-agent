use std::fs;
use std::path::Path;

use anyhow::Result;
use csa_core::types::ReviewDecision;
use csa_session::{FindingsFile, ReviewVerdictArtifact, Severity, write_findings_toml};

use super::artifacts::severity_counts_are_zero;
use super::artifacts::{
    has_blocking_severity, load_findings_toml_from_output, load_review_artifact_from_output,
};
use super::prose_signals::{reconcile_counts_with_prose, review_prose_signals};
use crate::review_cmd::prose_findings::severity_counts_from_review_findings;

const PROSE_FINDINGS_UNPARSED_REASON: &str = "prose_findings_present_but_unparsed";
const SEVERITY_FINDINGS_MISMATCH_REASON: &str = "severity_counts_findings_mismatch";

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
    let skip_prose_override = extraction_confirmed_empty || synthetic_empty;
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

    let findings_counts = severity_counts_from_review_findings(&findings_file.findings);
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
    let has_review_artifact_findings = load_review_artifact_from_output(session_dir)?
        .is_some_and(|artifact| !artifact.findings.is_empty());
    let has_structured_findings =
        !findings_file.findings.is_empty() || has_review_artifact_findings;
    let structured_mismatch =
        !severity_counts_are_zero(&artifact.severity_counts) && !has_structured_findings;
    let blocking_prose =
        prose_signals.blocking_summary || has_blocking_severity(&prose_signals.severity_counts);
    let blocking_structured = has_blocking_severity(&artifact.severity_counts);
    let parsed_findings_prose = prose_signals.parsed_findings_sections;
    let unparsed_findings_prose = prose_signals.unparseable_findings_sections;
    let cross_dimension_blocker = prose_signals.cross_dimension_blockers;
    let checklist_violation_mismatch = prose_signals.checklist_violation_findings
        && !has_structured_findings
        && severity_counts_are_zero(&artifact.severity_counts);
    let cross_dimension_blocker_mismatch = cross_dimension_blocker
        && !has_structured_findings
        && severity_counts_are_zero(&artifact.severity_counts);

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
    }

    Ok(())
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
