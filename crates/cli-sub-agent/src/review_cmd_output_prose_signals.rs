use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;
use csa_session::{ReviewFinding, Severity};

use super::clean_detection::{
    contains_clean_phrase, detect_prose_clean_conclusion, detect_prose_fail_conclusion,
};
use super::text::{contains_blocking_issue_signal, zero_severity_counts};
use crate::review_cmd::prose_findings::{
    FindingsSectionParse, classify_findings_section_body,
    extract_review_findings_from_prose_with_default, findings_section_bodies,
    severity_counts_from_review_findings,
};

#[derive(Debug, Clone)]
pub(super) struct ReviewProseSignals {
    pub(super) severity_counts: BTreeMap<Severity, u32>,
    pub(super) blocking_summary: bool,
    pub(super) parsed_findings_sections: bool,
    pub(super) unparseable_findings_sections: bool,
    pub(super) cross_dimension_blockers: bool,
    pub(super) checklist_violation_findings: bool,
    pub(super) findings: Vec<ReviewFinding>,
}

pub(super) fn review_prose_signals(session_dir: &Path) -> Result<ReviewProseSignals> {
    let prose = current_round_review_prose_contents(session_dir)?;
    let mut signals = ReviewProseSignals {
        severity_counts: zero_severity_counts(),
        blocking_summary: prose.blocking_summary,
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

    Ok(signals)
}

struct CurrentRoundReviewProseContents {
    blocking_summary: bool,
    contents: Vec<(String, String)>,
}

fn current_round_review_prose_contents(
    session_dir: &Path,
) -> Result<CurrentRoundReviewProseContents> {
    let mut latest_summary = None;
    let mut latest_details = None;

    for (section, content) in csa_session::read_all_sections(session_dir)? {
        match section.id.as_str() {
            "summary" => latest_summary = Some(content),
            "details" => latest_details = Some(content),
            _ => {}
        }
    }

    let blocking_summary = latest_summary.as_deref().is_some_and(|summary| {
        contains_blocking_issue_signal(summary) || detect_prose_fail_conclusion(summary)
    });

    let mut contents = Vec::new();
    if let Some(content) = latest_summary {
        push_review_content(&mut contents, "summary", content);
    }

    if let Some(content) = latest_details {
        push_review_content(&mut contents, "details", content);
    }

    if let Some(content) =
        crate::review_cmd::findings_toml::load_canonical_review_text(session_dir)?
        && !review_content_is_covered_by_sections(&contents, &content)
    {
        push_review_content(&mut contents, "canonical", content);
    }

    Ok(CurrentRoundReviewProseContents {
        blocking_summary,
        contents,
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
    let findings =
        extract_review_findings_from_prose_with_default(content, default_unlabeled_severity);
    let counts = severity_counts_from_review_findings(&findings);
    merge_severity_counts_add(&mut signals.severity_counts, &counts);
    signals.findings.extend(findings);
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

fn merge_severity_counts_add(
    target: &mut BTreeMap<Severity, u32>,
    source: &BTreeMap<Severity, u32>,
) {
    for (severity, count) in source {
        *target.entry(severity.clone()).or_insert(0) += *count;
    }
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
}
