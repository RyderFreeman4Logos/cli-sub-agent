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
