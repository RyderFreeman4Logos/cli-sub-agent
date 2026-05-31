use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::Result;
use csa_session::Severity;

use super::text::{
    contains_blocking_issue_signal, severity_counts_from_text, zero_severity_counts,
};

#[derive(Debug, Clone)]
pub(super) struct ReviewProseSignals {
    pub(super) severity_counts: BTreeMap<Severity, u32>,
    pub(super) blocking_summary: bool,
}

pub(super) fn review_prose_signals(session_dir: &Path) -> Result<ReviewProseSignals> {
    let mut signals = ReviewProseSignals {
        severity_counts: zero_severity_counts(),
        blocking_summary: false,
    };
    let mut saw_summary = false;
    let mut saw_details = false;

    for (section, content) in csa_session::read_all_sections(session_dir)? {
        match section.id.as_str() {
            "summary" => {
                saw_summary = true;
                record_review_prose_signal(&mut signals, "summary", &content);
            }
            "details" => {
                saw_details = true;
                record_review_prose_signal(&mut signals, "details", &content);
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
        record_review_prose_signal(&mut signals, section_id, &content);
    }

    Ok(signals)
}

fn record_review_prose_signal(signals: &mut ReviewProseSignals, section_id: &str, content: &str) {
    if section_id == "summary" && contains_blocking_issue_signal(content) {
        signals.blocking_summary = true;
    }
    let counts = severity_counts_from_text(content);
    merge_severity_counts_add(&mut signals.severity_counts, &counts);
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
