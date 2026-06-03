use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::Result;
use csa_session::{ReviewFinding, Severity};

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
    pub(super) actionable_prose_sections: bool,
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
        actionable_prose_sections: false,
        findings: Vec::new(),
    };
    let default_unlabeled_severity = blocking_summary.then_some(Severity::Medium);
    for (section_id, content) in contents {
        record_review_prose_signal(
            &mut signals,
            &section_id,
            &content,
            default_unlabeled_severity.clone(),
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
) {
    signals.actionable_prose_sections |= contains_actionable_review_section(content);
    let findings =
        extract_review_findings_from_prose_with_default(content, default_unlabeled_severity);
    let mut counts = severity_counts_from_review_findings(&findings);
    reconcile_counts_max(&mut counts, &severity_counts_from_text(content));
    merge_severity_counts_add(&mut signals.severity_counts, &counts);
    signals.findings.extend(findings);
}

fn contains_actionable_review_section(content: &str) -> bool {
    let mut active_section = false;
    let mut body = Vec::new();

    for line in content.lines() {
        if let Some(header) = markdown_header_text(line) {
            if active_section && section_body_is_actionable(&body) {
                return true;
            }
            active_section = is_findings_header(header);
            body.clear();
            continue;
        }

        if active_section {
            body.push(line);
        }
    }

    active_section && section_body_is_actionable(&body)
}

fn markdown_header_text(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('#') {
        return None;
    }
    Some(trimmed.trim_start_matches('#').trim())
}

fn section_body_is_actionable(lines: &[&str]) -> bool {
    let normalized = lines
        .iter()
        .map(|line| normalize_actionable_section_line(line))
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();

    !normalized.is_empty()
        && normalized
            .iter()
            .any(|line| !is_no_actionable_section_sentinel(line))
}

fn normalize_actionable_section_line(line: &str) -> String {
    let mut trimmed = line.trim();
    if matches!(trimmed.as_bytes().first(), Some(b'-' | b'*')) {
        trimmed = trimmed[1..].trim_start();
    } else if let Some(numbered) = strip_numbered_prefix(trimmed) {
        trimmed = numbered.trim_start();
    }
    trimmed
        .trim_matches(|ch: char| ch.is_ascii_punctuation())
        .trim()
        .to_ascii_lowercase()
}

fn strip_numbered_prefix(line: &str) -> Option<&str> {
    let (index, rest) = line.split_once('.')?;
    if index.is_empty() || !index.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    Some(rest)
}

fn is_no_actionable_section_sentinel(line: &str) -> bool {
    matches!(
        line,
        "none"
            | "n/a"
            | "na"
            | "not applicable"
            | "no findings"
            | "no findings found"
            | "no blocking findings"
            | "no blocking findings found"
            | "no actionable findings"
            | "no actionable findings found"
            | "no recommended actions"
            | "no action required"
            | "no actions required"
    )
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
