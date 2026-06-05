use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;
use csa_session::{ReviewFinding, Severity};

use super::clean_detection::{contains_clean_phrase, detect_prose_clean_conclusion};
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
    pub(super) findings: Vec<ReviewFinding>,
}

pub(super) fn review_prose_signals(session_dir: &Path) -> Result<ReviewProseSignals> {
    let prose = current_round_review_prose_contents(session_dir)?;
    let mut signals = ReviewProseSignals {
        severity_counts: zero_severity_counts(),
        blocking_summary: prose.blocking_summary,
        parsed_findings_sections: false,
        unparseable_findings_sections: false,
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
    let (parsed_findings_sections, unparseable_findings_sections) =
        classify_findings_sections(content, default_unlabeled_severity.clone());
    signals.parsed_findings_sections |= parsed_findings_sections;
    signals.unparseable_findings_sections |= unparseable_findings_sections;
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

fn contains_canonical_clean_conclusion(content: &str) -> bool {
    contains_clean_phrase(content) || detect_prose_clean_conclusion(content)
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
