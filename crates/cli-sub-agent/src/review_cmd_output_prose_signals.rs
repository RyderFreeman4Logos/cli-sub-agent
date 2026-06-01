use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::Result;
use csa_session::{ReviewFinding, Severity};

use super::text::{
    contains_blocking_issue_signal, severity_counts_from_text, zero_severity_counts,
};
use crate::review_cmd::prose_findings::{
    extract_review_findings_from_prose_with_default, severity_counts_from_review_findings,
};

#[derive(Debug, Clone)]
pub(super) struct ReviewProseSignals {
    pub(super) severity_counts: BTreeMap<Severity, u32>,
    pub(super) blocking_summary: bool,
    pub(super) findings: Vec<ReviewFinding>,
}

pub(super) fn review_prose_signals(session_dir: &Path) -> Result<ReviewProseSignals> {
    let mut saw_summary = false;
    let mut saw_details = false;
    let mut contents = Vec::new();

    for (section, content) in csa_session::read_all_sections(session_dir)? {
        match section.id.as_str() {
            "summary" => {
                saw_summary = true;
                contents.push(("summary".to_string(), content));
            }
            "details" => {
                saw_details = true;
                contents.push(("details".to_string(), content));
            }
            _ => {}
        }
    }

    for (section_id, saw_section) in [("summary", saw_summary), ("details", saw_details)] {
        if saw_section {
            continue;
        }
        let path = session_dir.join("output").join(format!("{section_id}.md"));
        if !path.exists() {
            continue;
        }
        let content = fs::read_to_string(&path)
            .map_err(|error| anyhow::anyhow!("read {}: {error}", path.display()))?;
        contents.push((section_id.to_string(), content));
    }

    let blocking_summary = contents
        .iter()
        .filter(|(section_id, _)| section_id == "summary")
        .any(|(_, content)| contains_blocking_issue_signal(content));
    let mut signals = ReviewProseSignals {
        severity_counts: zero_severity_counts(),
        blocking_summary,
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

fn record_review_prose_signal(
    signals: &mut ReviewProseSignals,
    _section_id: &str,
    content: &str,
    default_unlabeled_severity: Option<Severity>,
) {
    let findings =
        extract_review_findings_from_prose_with_default(content, default_unlabeled_severity);
    let mut counts = severity_counts_from_review_findings(&findings);
    reconcile_counts_max(&mut counts, &severity_counts_from_text(content));
    merge_severity_counts_add(&mut signals.severity_counts, &counts);
    signals.findings.extend(findings);
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
