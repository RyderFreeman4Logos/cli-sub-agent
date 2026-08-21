use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;
use csa_session::{ReviewFinding, Severity};

use super::artifacts::has_blocking_severity;
use super::clean_detection::{
    contains_clean_phrase, current_round_review_sections, detect_prose_clean_conclusion,
    detect_prose_fail_conclusion, detect_prose_uncertain_conclusion,
};
use super::text::{contains_blocking_issue_signal, zero_severity_counts};
use crate::review_cmd::prose_findings::{
    FindingsSectionParse, classify_findings_section_body,
    extract_review_findings_from_prose_with_default, findings_section_bodies,
    generated_prose_finding_id, is_generated_prose_finding_id, review_finding_payload_eq,
    severity_counts_from_review_findings,
};

#[derive(Debug, Clone)]
pub(super) struct ReviewProseSignals {
    pub(super) severity_counts: BTreeMap<Severity, u32>,
    pub(super) blocking_summary: bool,
    pub(super) fail_conclusion: bool,
    pub(super) uncertain_conclusion: bool,
    pub(super) parsed_findings_sections: bool,
    pub(super) unparseable_findings_sections: bool,
    pub(super) cross_dimension_blockers: bool,
    pub(super) checklist_violation_findings: bool,
    pub(super) findings: Vec<ReviewFinding>,
}

impl ReviewProseSignals {
    pub(super) fn has_failure_evidence(&self) -> bool {
        has_blocking_severity(&self.severity_counts)
            || self.blocking_summary
            || self.uncertain_conclusion
            || self.parsed_findings_sections
            || self.unparseable_findings_sections
            || self.cross_dimension_blockers
            || self.checklist_violation_findings
            || !self.findings.is_empty()
    }
}

pub(super) fn review_prose_signals(session_dir: &Path) -> Result<ReviewProseSignals> {
    let prose = review_prose_contents(session_dir, CanonicalProseMode::IncludeWhenDistinct)?;
    review_prose_signals_from_contents(prose)
}

pub(super) fn current_round_review_prose_signals(session_dir: &Path) -> Result<ReviewProseSignals> {
    let prose = review_prose_contents(session_dir, CanonicalProseMode::FallbackOnly)?;
    review_prose_signals_from_contents(prose)
}

fn review_prose_signals_from_contents(
    prose: CurrentRoundReviewProseContents,
) -> Result<ReviewProseSignals> {
    let mut signals = ReviewProseSignals {
        severity_counts: zero_severity_counts(),
        blocking_summary: prose.blocking_summary,
        fail_conclusion: false,
        uncertain_conclusion: false,
        parsed_findings_sections: false,
        unparseable_findings_sections: false,
        cross_dimension_blockers: false,
        checklist_violation_findings: false,
        findings: Vec::new(),
    };
    let default_unlabeled_severity = prose.blocking_summary.then_some(Severity::Medium);
    for (section_id, content) in prose.contents {
        record_review_prose_signal(
            &mut signals,
            &section_id,
            &content,
            default_unlabeled_severity.clone(),
        );
    }
    signals.severity_counts = severity_counts_from_review_findings(&signals.findings);

    Ok(signals)
}

enum CanonicalProseMode {
    IncludeWhenDistinct,
    FallbackOnly,
}

struct CurrentRoundReviewProseContents {
    blocking_summary: bool,
    contents: Vec<(String, String)>,
}

fn review_prose_contents(
    session_dir: &Path,
    canonical_mode: CanonicalProseMode,
) -> Result<CurrentRoundReviewProseContents> {
    let mut contents = Vec::new();
    if let Some(current_sections) = current_round_review_sections(session_dir)? {
        for (section_id, content) in current_sections {
            push_review_content(&mut contents, &section_id, content);
        }
    }
    let indexed_review_content_count = contents.len();

    let should_load_canonical = match canonical_mode {
        CanonicalProseMode::IncludeWhenDistinct => true,
        CanonicalProseMode::FallbackOnly => indexed_review_content_count == 0,
    };
    if should_load_canonical
        && let Some(content) =
            crate::review_cmd::findings_toml::load_canonical_review_text(session_dir)?
        && !review_content_is_covered_by_sections(&contents, &content)
    {
        push_review_content(&mut contents, "canonical", content);
    }

    let blocking_summary = contents
        .iter()
        .take(if indexed_review_content_count == 0 {
            contents.len()
        } else {
            indexed_review_content_count
        })
        .any(|(_, content)| content_has_blocking_review_outcome(content));

    Ok(CurrentRoundReviewProseContents {
        blocking_summary,
        contents,
    })
}

fn content_has_blocking_review_outcome(content: &str) -> bool {
    if has_clean_verdict_prefix(content) && !detect_prose_fail_conclusion(content) {
        return false;
    }
    contains_blocking_issue_signal(content) || detect_prose_fail_conclusion(content)
}

fn has_clean_verdict_prefix(content: &str) -> bool {
    content.lines().any(|line| {
        let trimmed = line.trim_start();
        ["PASS", "CLEAN"].iter().any(|token| {
            trimmed
                .strip_prefix(token)
                .and_then(|rest| rest.chars().next())
                .is_some_and(|ch| !ch.is_ascii_alphanumeric() && ch != '_')
        })
    })
}

fn push_review_content(contents: &mut Vec<(String, String)>, section_id: &str, content: String) {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return;
    }
    if contents
        .iter()
        .any(|(_, existing)| existing.trim() == trimmed)
    {
        return;
    }
    contents.push((section_id.to_string(), content));
}

fn review_content_is_covered_by_sections(contents: &[(String, String)], candidate: &str) -> bool {
    let mut residual = candidate.trim().to_string();
    if residual.is_empty() {
        return true;
    }

    for (_, content) in contents {
        let trimmed = content.trim();
        if trimmed.is_empty() {
            continue;
        }
        while let Some(start) = residual.find(trimmed) {
            let end = start + trimmed.len();
            residual.replace_range(start..end, "");
        }
        if residual.trim().is_empty() {
            return true;
        }
    }

    false
}

fn record_review_prose_signal(
    signals: &mut ReviewProseSignals,
    _section_id: &str,
    content: &str,
    default_unlabeled_severity: Option<Severity>,
) {
    let (parsed_findings_sections, unparseable_findings_sections) =
        classify_findings_sections(content, default_unlabeled_severity.clone());
    signals.parsed_findings_sections |= parsed_findings_sections;
    signals.unparseable_findings_sections |= unparseable_findings_sections;
    signals.cross_dimension_blockers |= cross_dimension_enumeration_has_blocker(content);
    signals.checklist_violation_findings |= checklist_violation_references_finding(content);
    signals.fail_conclusion |= detect_prose_fail_conclusion(content);
    signals.uncertain_conclusion |= detect_prose_uncertain_conclusion(content);
    let findings =
        extract_review_findings_from_prose_with_default(content, default_unlabeled_severity);
    for mut finding in findings {
        if signals
            .findings
            .iter()
            .any(|existing| review_finding_payload_eq(existing, &finding))
        {
            continue;
        }
        if is_generated_prose_finding_id(&finding.id) {
            let index = signals
                .findings
                .iter()
                .filter(|existing| is_generated_prose_finding_id(&existing.id))
                .count()
                + 1;
            finding.id = generated_prose_finding_id(index);
        }
        signals.findings.push(finding);
    }
}

fn classify_findings_sections(
    content: &str,
    default_unlabeled_severity: Option<Severity>,
) -> (bool, bool) {
    let mut parsed_findings_sections = false;
    let mut unparseable_findings_sections = false;
    for body in findings_section_bodies(content) {
        if !body.is_markdown_heading() {
            continue;
        }
        match classify_findings_section_body(
            body.as_str(),
            default_unlabeled_severity.clone(),
            contains_canonical_clean_conclusion,
        ) {
            FindingsSectionParse::Clean => {}
            FindingsSectionParse::Findings(_) => parsed_findings_sections = true,
            FindingsSectionParse::Unparseable => unparseable_findings_sections = true,
        }
    }
    (parsed_findings_sections, unparseable_findings_sections)
}

fn cross_dimension_enumeration_has_blocker(content: &str) -> bool {
    let mut in_section = false;
    let mut in_code_fence = false;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_code_fence = !in_code_fence;
            continue;
        }
        if in_code_fence {
            continue;
        }
        if trimmed.starts_with('#') {
            in_section = is_cross_dimension_enumeration_heading(trimmed);
            continue;
        }
        if !in_section {
            continue;
        }
        if let Some(body) = numbered_item_body(trimmed)
            && enumeration_item_has_blocker(body)
        {
            return true;
        }
    }

    false
}

fn is_cross_dimension_enumeration_heading(line: &str) -> bool {
    let normalized = line.trim_start_matches('#').trim().to_ascii_lowercase();
    normalized == "cross-dimension blocking enumeration"
        || normalized == "cross dimension blocking enumeration"
}

fn numbered_item_body(line: &str) -> Option<&str> {
    let (index, rest) = line.split_once('.')?;
    if index.is_empty() || !index.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    Some(rest.trim_start())
}

fn enumeration_item_has_blocker(body: &str) -> bool {
    let substantive = body
        .split_once(':')
        .map_or(body, |(_, description)| description)
        .trim();
    !substantive.is_empty() && !is_no_independent_blocker_phrase(substantive)
}

fn is_no_independent_blocker_phrase(text: &str) -> bool {
    let normalized = text
        .trim()
        .trim_matches(|ch: char| ch.is_ascii_punctuation() || ch.is_whitespace())
        .to_ascii_lowercase();
    matches!(
        normalized.as_str(),
        "none"
            | "n/a"
            | "not applicable"
            | "no blocker"
            | "no blockers"
            | "no blocker found"
            | "no blockers found"
            | "no independent blocker"
            | "no independent blockers"
            | "no independent blocker found"
            | "no independent blockers found"
    )
}

fn contains_canonical_clean_conclusion(content: &str) -> bool {
    contains_clean_phrase(content) || detect_prose_clean_conclusion(content)
}

fn checklist_violation_references_finding(content: &str) -> bool {
    let mut in_checklist_section = false;
    let mut in_code_fence = false;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_code_fence = !in_code_fence;
            continue;
        }
        if in_code_fence {
            continue;
        }
        if trimmed.starts_with('#') {
            in_checklist_section = is_checklist_heading(trimmed);
            continue;
        }
        if in_checklist_section && line_violation_references_finding_id(trimmed) {
            return true;
        }
    }

    false
}

fn is_checklist_heading(line: &str) -> bool {
    let normalized = line.trim_start_matches('#').trim();
    let normalized = normalized
        .split_once('(')
        .map_or(normalized, |(heading, _)| heading.trim_end());
    normalized.to_ascii_lowercase().contains("checklist")
}

fn line_violation_references_finding_id(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    if !lower.contains("violation") {
        return false;
    }
    let Some(index) = lower.find("finding") else {
        return false;
    };
    line.get(index + "finding".len()..)
        .unwrap_or_default()
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '-' || ch == '_'))
        .any(token_looks_like_finding_id)
}

fn token_looks_like_finding_id(token: &str) -> bool {
    let token = token.trim();
    let has_alpha = token.chars().any(|ch| ch.is_ascii_alphabetic());
    token.len() >= 2
        && has_alpha
        && (token.chars().any(|ch| ch.is_ascii_digit())
            || token.contains('-')
            || token.contains('_'))
}

pub(super) fn reconcile_counts_with_prose(
    mut structured_counts: BTreeMap<Severity, u32>,
    prose_counts: &BTreeMap<Severity, u32>,
) -> BTreeMap<Severity, u32> {
    for (severity, prose_count) in prose_counts {
        let count = structured_counts.entry(severity.clone()).or_insert(0);
        *count = (*count).max(*prose_count);
    }
    structured_counts
}

#[cfg(test)]
mod tests {
    use super::review_content_is_covered_by_sections;

    fn review_contents() -> Vec<(String, String)> {
        vec![
            ("summary".to_string(), "PASS".to_string()),
            (
                "details".to_string(),
                "## Findings\n\nNo findings.".to_string(),
            ),
        ]
    }

    #[test]
    fn exact_duplicate_candidate_is_covered_by_sections() {
        let candidate = "PASS\n## Findings\n\nNo findings.";

        assert!(review_content_is_covered_by_sections(
            &review_contents(),
            candidate
        ));
    }

    #[test]
    fn superset_candidate_with_raw_extra_is_not_covered_by_sections() {
        let candidate = concat!(
            "PASS\n",
            "## Findings\n\nNo findings.\n\n",
            "## Cross-Dimension Blocking Enumeration\n",
            "1. Correctness: raw transcript blocker only present in output.log.\n"
        );

        assert!(!review_content_is_covered_by_sections(
            &review_contents(),
            candidate
        ));
    }

    #[test]
    fn issue_2815_authoritative_findings_shrink_stale_verdict_counts() {
        use csa_core::types::ReviewDecision;
        use csa_session::{
            FindingsFile, ReviewFinding, ReviewFindingFileRange, ReviewVerdictArtifact, Severity,
        };

        let temp = tempfile::tempdir().expect("tempdir");
        let session_dir = temp.path();
        std::fs::create_dir(session_dir.join("output")).expect("create output dir");
        csa_session::persist_structured_output(
            session_dir,
            r#"<!-- CSA:SECTION:summary -->
Review result: FAIL. One high and one medium finding remain.
<!-- CSA:SECTION:summary:END -->
<!-- CSA:SECTION:details -->
## Findings
1. [HIGH] High finding remains (`src/high.rs:10`).
2. [MEDIUM] Medium finding remains (`src/medium.rs:20`).
<!-- CSA:SECTION:details:END -->
"#,
        )
        .expect("persist review prose");
        let finding = |id: &str, severity, path: &str, start, description: &str| ReviewFinding {
            id: id.to_string(),
            severity,
            file_ranges: vec![ReviewFindingFileRange {
                path: path.to_string(),
                start,
                end: None,
            }],
            is_regression_of_commit: None,
            suggested_test_scenario: None,
            description: description.to_string(),
        };
        let expected = FindingsFile {
            findings: vec![
                finding(
                    "structured-high",
                    Severity::High,
                    "src/high.rs",
                    10,
                    "High finding remains (`src/high.rs:10`).",
                ),
                finding(
                    "structured-medium",
                    Severity::Medium,
                    "src/medium.rs",
                    20,
                    "Medium finding remains (`src/medium.rs:20`).",
                ),
            ],
        };
        csa_session::write_findings_toml(session_dir, &expected).expect("write findings");
        let mut verdict = ReviewVerdictArtifact::from_parts(
            "01TEST2815EXACTCOUNTS00".to_string(),
            ReviewDecision::Fail,
            "HAS_ISSUES",
            &[],
            Vec::new(),
        );
        verdict.severity_counts.insert(Severity::High, 7);
        verdict.severity_counts.insert(Severity::Medium, 9);
        csa_session::write_review_verdict(session_dir, &verdict).expect("write verdict");

        assert!(
            super::super::consistency::repair_clean_empty_fail_review_verdict(session_dir)
                .expect("repair verdict")
        );

        let persisted: FindingsFile = toml::from_str(
            &std::fs::read_to_string(session_dir.join("output/findings.toml"))
                .expect("read findings"),
        )
        .expect("parse findings");
        assert_eq!(persisted.findings, expected.findings);
        let verdict: ReviewVerdictArtifact = serde_json::from_str(
            &std::fs::read_to_string(session_dir.join("output/review-verdict.json"))
                .expect("read verdict"),
        )
        .expect("parse verdict");
        assert_eq!(verdict.severity_counts.get(&Severity::High), Some(&1));
        assert_eq!(verdict.severity_counts.get(&Severity::Medium), Some(&1));
    }
}
