use std::collections::BTreeMap;

use csa_session::Severity;

pub(super) fn format_nonblocking_counts_suffix(_counts: &BTreeMap<Severity, u32>) -> String {
    String::new()
}

pub(super) fn has_review_severity_counts(counts: &BTreeMap<Severity, u32>) -> bool {
    counts.values().any(|count| *count > 0)
}

pub(super) fn zero_severity_counts() -> BTreeMap<Severity, u32> {
    [
        (Severity::Critical, 0),
        (Severity::High, 0),
        (Severity::Medium, 0),
        (Severity::Low, 0),
    ]
    .into_iter()
    .collect()
}
