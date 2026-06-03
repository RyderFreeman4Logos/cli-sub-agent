use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;
use csa_session::{ReviewFinding, Severity};

use super::clean_detection::{contains_clean_phrase, detect_prose_clean_conclusion};
use super::text::{
    contains_blocking_issue_signal, severity_counts_from_text, zero_severity_counts,
};
use crate::review_cmd::prose_findings::{
    extract_review_findings_from_prose_with_default, findings_section_bodies,
    severity_counts_from_review_findings,
};

#[derive(Debug, Clone)]
pub(super) struct ReviewProseSignals {
    pub(super) severity_counts: BTreeMap<Severity, u32>,
    pub(super) blocking_summary: bool,
    pub(super) unclean_findings_sections: bool,
    pub(super) findings: Vec<ReviewFinding>,
}

pub(super) fn review_prose_signals(session_dir: &Path) -> Result<ReviewProseSignals> {
    let prose = current_round_review_prose_contents(session_dir)?;
    let mut signals = ReviewProseSignals {
        severity_counts: zero_severity_counts(),
        blocking_summary: prose.blocking_summary,
        unclean_findings_sections: false,
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

    let blocking_summary = latest_summary
        .as_deref()
        .is_some_and(contains_blocking_issue_signal);

    if let Some(content) =
        crate::review_cmd::findings_toml::load_canonical_review_text(session_dir)?
    {
        return Ok(CurrentRoundReviewProseContents {
            blocking_summary,
            contents: vec![("canonical".to_string(), content)],
        });
    }

    let mut contents = Vec::new();
    if let Some(content) = latest_summary {
        contents.push(("summary".to_string(), content));
    }

    if let Some(content) = latest_details {
        contents.push(("details".to_string(), content));
    }

    Ok(CurrentRoundReviewProseContents {
        blocking_summary,
        contents,
    })
}

fn record_review_prose_signal(
    signals: &mut ReviewProseSignals,
    _section_id: &str,
    content: &str,
    default_unlabeled_severity: Option<Severity>,
) {
    signals.unclean_findings_sections |=
        contains_unclean_findings_section(content, default_unlabeled_severity.clone());
    let findings =
        extract_review_findings_from_prose_with_default(content, default_unlabeled_severity);
    let mut counts = severity_counts_from_review_findings(&findings);
    reconcile_counts_max(&mut counts, &severity_counts_from_text(content));
    merge_severity_counts_add(&mut signals.severity_counts, &counts);
    signals.findings.extend(findings);
}

fn contains_unclean_findings_section(
    content: &str,
    default_unlabeled_severity: Option<Severity>,
) -> bool {
    findings_section_bodies(content).into_iter().any(|body| {
        findings_section_body_is_unclean(body.as_str(), default_unlabeled_severity.clone())
    })
}

fn contains_canonical_clean_conclusion(content: &str) -> bool {
    contains_clean_phrase(content) || detect_prose_clean_conclusion(content)
}

fn findings_section_body_is_unclean(
    body: &str,
    default_unlabeled_severity: Option<Severity>,
) -> bool {
    body.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("```"))
        .any(|line| {
            !contains_canonical_clean_conclusion(line)
                && !line_parses_as_structured_finding(line, default_unlabeled_severity.clone())
        })
}

fn line_parses_as_structured_finding(
    line: &str,
    default_unlabeled_severity: Option<Severity>,
) -> bool {
    let parser_input = format!("Findings\n{line}");
    !extract_review_findings_from_prose_with_default(&parser_input, default_unlabeled_severity)
        .is_empty()
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

fn reconcile_counts_max(target: &mut BTreeMap<Severity, u32>, source: &BTreeMap<Severity, u32>) {
    for (severity, source_count) in source {
        let target_count = target.entry(severity.clone()).or_insert(0);
        *target_count = (*target_count).max(*source_count);
    }
}
