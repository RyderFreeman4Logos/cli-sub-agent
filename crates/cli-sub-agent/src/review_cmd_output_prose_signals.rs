use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::Result;
use csa_session::{ReviewFinding, Severity};

use super::clean_detection::{contains_clean_phrase, detect_prose_clean_conclusion};
use super::text::{
    contains_blocking_issue_signal, severity_counts_from_text, zero_severity_counts,
};
use crate::review_cmd::prose_findings::{
    extract_review_findings_from_prose_with_default, is_findings_header,
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
    let contents = current_round_review_prose_contents(session_dir)?;

    let blocking_summary = contents
        .iter()
        .filter(|(section_id, _)| section_id == "summary")
        .any(|(_, content)| contains_blocking_issue_signal(content));
    let mut signals = ReviewProseSignals {
        severity_counts: zero_severity_counts(),
        blocking_summary,
        unclean_findings_sections: false,
        findings: Vec::new(),
    };
    let review_has_clean_conclusion = contents
        .iter()
        .any(|(_, content)| contains_canonical_clean_conclusion(content));
    let default_unlabeled_severity = blocking_summary.then_some(Severity::Medium);
    for (section_id, content) in contents {
        record_review_prose_signal(
            &mut signals,
            &section_id,
            &content,
            default_unlabeled_severity.clone(),
            review_has_clean_conclusion,
        );
    }

    Ok(signals)
}

fn current_round_review_prose_contents(session_dir: &Path) -> Result<Vec<(String, String)>> {
    let mut latest_summary = None;
    let mut latest_details = None;

    for (section, content) in csa_session::read_all_sections(session_dir)? {
        match section.id.as_str() {
            "summary" => latest_summary = Some(content),
            "details" => latest_details = Some(content),
            _ => {}
        }
    }

    let mut contents = Vec::new();
    if let Some(content) = latest_summary {
        contents.push(("summary".to_string(), content));
    } else if let Some(content) = read_legacy_section_file(session_dir, "summary")? {
        contents.push(("summary".to_string(), content));
    }

    if let Some(content) = latest_details {
        contents.push(("details".to_string(), content));
    } else if let Some(content) = read_legacy_section_file(session_dir, "details")? {
        contents.push(("details".to_string(), content));
    }

    Ok(contents)
}

fn read_legacy_section_file(session_dir: &Path, section_id: &str) -> Result<Option<String>> {
    let path = session_dir.join("output").join(format!("{section_id}.md"));
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(&path)
        .map_err(|error| anyhow::anyhow!("read {}: {error}", path.display()))?;
    Ok(Some(content))
}

fn record_review_prose_signal(
    signals: &mut ReviewProseSignals,
    _section_id: &str,
    content: &str,
    default_unlabeled_severity: Option<Severity>,
    review_has_clean_conclusion: bool,
) {
    signals.unclean_findings_sections |=
        contains_unclean_findings_section(content, review_has_clean_conclusion);
    let findings =
        extract_review_findings_from_prose_with_default(content, default_unlabeled_severity);
    let mut counts = severity_counts_from_review_findings(&findings);
    reconcile_counts_max(&mut counts, &severity_counts_from_text(content));
    merge_severity_counts_add(&mut signals.severity_counts, &counts);
    signals.findings.extend(findings);
}

fn contains_unclean_findings_section(content: &str, review_has_clean_conclusion: bool) -> bool {
    contains_findings_section(content) && !review_has_clean_conclusion
}

fn contains_findings_section(content: &str) -> bool {
    content
        .lines()
        .filter_map(markdown_header_text)
        .any(is_findings_header)
}

fn contains_canonical_clean_conclusion(content: &str) -> bool {
    contains_clean_phrase(content) || detect_prose_clean_conclusion(content)
}

fn markdown_header_text(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('#') {
        return None;
    }
    Some(trimmed.trim_start_matches('#').trim())
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
