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
const MISSING_BUG_CATEGORY_CHECKLIST_REASON: &str = "missing_bug_category_checklist";
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
    let review_artifact_missing_bug_category_checklist = review_artifact
        .as_ref()
        .is_some_and(|artifact| artifact.missing_required_bug_category_checklist());
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
                || review_artifact_has_blocking_risk
                || review_artifact_missing_bug_category_checklist,
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
    if artifact.decision == ReviewDecision::Pass && review_artifact_missing_bug_category_checklist {
        artifact.decision = ReviewDecision::Uncertain;
        artifact.verdict_legacy = "UNCERTAIN".to_string();
        artifact
            .failure_reason
            .get_or_insert_with(|| MISSING_BUG_CATEGORY_CHECKLIST_REASON.to_string());
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

include!("review_cmd_output_consistency_helpers.rs");
