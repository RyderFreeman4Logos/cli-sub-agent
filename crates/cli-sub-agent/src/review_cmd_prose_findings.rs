use std::collections::BTreeMap;

use csa_session::{FindingsFile, ReviewFinding, ReviewFindingFileRange, Severity};

pub(in crate::review_cmd) fn findings_file_from_prose(text: &str) -> Option<FindingsFile> {
    let findings = extract_review_findings_from_prose(text);
    if findings.is_empty() {
        None
    } else {
        Some(FindingsFile { findings })
    }
}

pub(in crate::review_cmd) fn extract_review_findings_from_prose(text: &str) -> Vec<ReviewFinding> {
    let default_unlabeled_severity =
        contains_blocking_review_signal(text).then_some(Severity::Medium);
    extract_review_findings_from_prose_with_default(text, default_unlabeled_severity)
}

pub(in crate::review_cmd) fn extract_review_findings_from_prose_with_default(
    text: &str,
    default_unlabeled_severity: Option<Severity>,
) -> Vec<ReviewFinding> {
    let mut findings: Vec<ReviewFinding> = Vec::new();
    let mut in_findings_section = false;
    let mut in_code_fence = false;

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_code_fence = !in_code_fence;
            continue;
        }
        if in_code_fence || trimmed.is_empty() {
            continue;
        }
        if is_findings_header(trimmed) {
            in_findings_section = true;
            continue;
        }
        if in_findings_section && trimmed.starts_with('#') {
            in_findings_section = false;
            continue;
        }

        if let Some(range) = parse_file_reference_line(trimmed) {
            if let Some(last) = findings.last_mut()
                && last.file_ranges.is_empty()
            {
                last.file_ranges.push(range);
            }
            continue;
        }

        let Some(parsed) = parse_finding_line(
            trimmed,
            in_findings_section,
            default_unlabeled_severity.clone(),
        ) else {
            continue;
        };
        findings.push(parsed.into_review_finding(format!("prose-{:03}", findings.len() + 1)));
    }

    findings
}

pub(in crate::review_cmd) fn severity_counts_from_review_findings(
    findings: &[ReviewFinding],
) -> BTreeMap<Severity, u32> {
    let mut counts = zero_severity_counts();
    for finding in findings {
        *counts.entry(finding.severity.clone()).or_insert(0) += 1;
    }
    counts
}

pub(in crate::review_cmd) fn zero_severity_counts() -> BTreeMap<Severity, u32> {
    [
        (Severity::Critical, 0),
        (Severity::High, 0),
        (Severity::Medium, 0),
        (Severity::Low, 0),
    ]
    .into_iter()
    .collect()
}

pub(in crate::review_cmd) fn severity_from_label(level: &str) -> Option<Severity> {
    let normalized = level.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "critical" | "p0" => Some(Severity::Critical),
        "high" | "p1" => Some(Severity::High),
        "medium" | "p2" => Some(Severity::Medium),
        "low" | "info" | "p3" | "p4" => Some(Severity::Low),
        _ => None,
    }
}

pub(in crate::review_cmd) fn contains_blocking_review_signal(text: &str) -> bool {
    text.lines().any(line_has_blocking_review_signal)
}

struct ParsedProseFinding {
    severity: Severity,
    file_range: Option<ReviewFindingFileRange>,
    description: String,
}

impl ParsedProseFinding {
    fn into_review_finding(self, id: String) -> ReviewFinding {
        ReviewFinding {
            id,
            severity: self.severity,
            file_ranges: self.file_range.into_iter().collect(),
            is_regression_of_commit: None,
            suggested_test_scenario: None,
            description: self.description,
        }
    }
}

fn parse_finding_line(
    line: &str,
    in_findings_section: bool,
    default_unlabeled_severity: Option<Severity>,
) -> Option<ParsedProseFinding> {
    let (numbered, body) =
        strip_numbered_prefix(line).map_or((false, line), |body| (true, body.trim_start()));

    if let Some(parsed) = parse_severity_prefixed_finding(body, numbered || in_findings_section) {
        return Some(parsed);
    }

    let severity = default_unlabeled_severity?;
    if !(numbered || in_findings_section) {
        return None;
    }
    parse_path_prefixed_finding(body, severity)
}

fn strip_numbered_prefix(line: &str) -> Option<&str> {
    let (index, rest) = line.split_once('.')?;
    if index.is_empty() || !index.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    Some(rest)
}

fn parse_severity_prefixed_finding(
    body: &str,
    allow_description_only: bool,
) -> Option<ParsedProseFinding> {
    let (label, rest) = body.split_once(':')?;
    let severity = severity_from_label(label).or_else(|| leading_severity_from_title(label))?;
    let rest = rest.trim();
    if let Some((file_range, description)) = parse_leading_file_range(rest) {
        return Some(ParsedProseFinding {
            severity,
            file_range: Some(file_range),
            description: non_empty_or_fallback(&description, body),
        });
    }
    allow_description_only.then(|| ParsedProseFinding {
        severity,
        file_range: None,
        description: severity_prefixed_description(label, rest),
    })
}

fn parse_path_prefixed_finding(body: &str, severity: Severity) -> Option<ParsedProseFinding> {
    let (file_range, description) = parse_leading_file_range(body)?;
    Some(ParsedProseFinding {
        severity,
        file_range: Some(file_range),
        description: non_empty_or_fallback(&description, body),
    })
}

fn parse_file_reference_line(line: &str) -> Option<ReviewFindingFileRange> {
    let line = strip_unordered_list_prefix(line);
    let (label, rest) = line.split_once(':')?;
    if !label.trim().eq_ignore_ascii_case("file") {
        return None;
    }
    parse_leading_file_range(rest.trim()).map(|(range, _)| range)
}

fn strip_unordered_list_prefix(line: &str) -> &str {
    let trimmed = line.trim_start();
    if matches!(trimmed.as_bytes().first(), Some(b'-' | b'*')) {
        trimmed[1..].trim_start()
    } else {
        trimmed
    }
}

fn leading_severity_from_title(title: &str) -> Option<Severity> {
    let first_word = title
        .trim_start()
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .find(|word| !word.is_empty())?;
    severity_from_label(first_word)
}

fn severity_prefixed_description(label: &str, rest: &str) -> String {
    if leading_severity_from_title(label).is_some() && severity_from_label(label).is_none() {
        non_empty_or_fallback(&format!("{}: {}", label.trim(), rest), rest)
    } else {
        rest.to_string()
    }
}

fn parse_leading_file_range(body: &str) -> Option<(ReviewFindingFileRange, String)> {
    let trimmed = body.trim_start_matches(['`', '(', '[']).trim_start();
    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let token = parts.next()?.trim_matches(['`', ',', '.', ')', ']']);
    let description = parts
        .next()
        .unwrap_or_default()
        .trim_start_matches(['-', ':'])
        .trim()
        .to_string();
    let (path, line) = parse_file_line_token(token)?;
    Some((
        ReviewFindingFileRange {
            path,
            start: line,
            end: None,
        },
        description,
    ))
}

fn parse_file_line_token(token: &str) -> Option<(String, u32)> {
    let (path, line) = token.rsplit_once(':')?;
    if path.is_empty() || !(path.contains('/') || path.contains('.')) {
        return None;
    }
    let line = line.parse::<u32>().ok()?;
    Some((path.to_string(), line))
}

fn non_empty_or_fallback(value: &str, fallback: &str) -> String {
    if value.trim().is_empty() {
        fallback.trim().to_string()
    } else {
        value.trim().to_string()
    }
}

fn is_findings_header(line: &str) -> bool {
    let normalized = line.trim_start_matches('#').trim();
    normalized.eq_ignore_ascii_case("findings")
        || normalized.eq_ignore_ascii_case("review findings")
}

fn line_has_blocking_review_signal(line: &str) -> bool {
    const ISSUE_NOUNS: &[&str] = &[
        "issue",
        "issues",
        "finding",
        "findings",
        "problem",
        "problems",
        "bug",
        "bugs",
        "defect",
        "defects",
        "violation",
        "violations",
    ];
    const NEGATIONS: &[&str] = &["no", "non", "nonblocking", "not", "none", "without"];
    const MAX_TOKENS_AFTER_BLOCKING: usize = 8;
    const MAX_NEGATION_LOOKBACK: usize = 3;

    let tokens = line
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>();

    for (index, token) in tokens.iter().enumerate() {
        if token == "nonblocking" {
            continue;
        }
        if token != "blocking" {
            continue;
        }
        if tokens[..index]
            .iter()
            .rev()
            .take(MAX_NEGATION_LOOKBACK)
            .any(|candidate| NEGATIONS.contains(&candidate.as_str()))
        {
            continue;
        }
        if ((index + 1)..tokens.len()).any(|candidate| {
            candidate - index <= MAX_TOKENS_AFTER_BLOCKING
                && ISSUE_NOUNS.contains(&tokens[candidate].as_str())
        }) {
            return true;
        }
    }

    false
}
