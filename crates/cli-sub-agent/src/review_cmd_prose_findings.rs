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

pub(in crate::review_cmd) fn findings_file_from_explicit_findings_sections(
    text: &str,
) -> Option<FindingsFile> {
    let mut findings = Vec::new();
    for body in findings_section_bodies(text) {
        let parser_input = format!("Findings\n{}", body.as_str());
        for mut finding in extract_review_findings_from_prose_with_default(&parser_input, None) {
            if findings
                .iter()
                .any(|existing| review_finding_payload_eq(existing, &finding))
            {
                continue;
            }
            finding.id = format!("prose-{:03}", findings.len() + 1);
            findings.push(finding);
        }
    }
    (!findings.is_empty()).then_some(FindingsFile { findings })
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

pub(in crate::review_cmd) struct FindingsSectionBody {
    body: String,
    is_markdown_heading: bool,
}

impl FindingsSectionBody {
    pub(in crate::review_cmd) fn as_str(&self) -> &str {
        &self.body
    }

    pub(in crate::review_cmd) fn is_markdown_heading(&self) -> bool {
        self.is_markdown_heading
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::review_cmd) enum FindingsSectionParse {
    Clean,
    Findings(Vec<ReviewFinding>),
    Unparseable,
}

pub(in crate::review_cmd) fn classify_findings_section_body(
    body: &str,
    default_unlabeled_severity: Option<Severity>,
    clean_conclusion: impl Fn(&str) -> bool,
) -> FindingsSectionParse {
    let parser_input = format!("Findings\n{body}");
    let findings =
        extract_review_findings_from_prose_with_default(&parser_input, default_unlabeled_severity);
    if !findings.is_empty() {
        return FindingsSectionParse::Findings(findings);
    }

    if findings_section_body_has_unparseable_text(body, &clean_conclusion) {
        FindingsSectionParse::Unparseable
    } else {
        FindingsSectionParse::Clean
    }
}

pub(in crate::review_cmd) fn findings_section_bodies(text: &str) -> Vec<FindingsSectionBody> {
    let mut bodies = Vec::new();
    let mut current = None::<(String, bool)>;
    let mut in_code_fence = false;

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            if let Some((body, _)) = current.as_mut() {
                body.push_str(line);
                body.push('\n');
            }
            in_code_fence = !in_code_fence;
            continue;
        }

        if !in_code_fence && is_findings_header(trimmed) {
            if let Some((body, is_markdown_heading)) = current.take() {
                bodies.push(FindingsSectionBody {
                    body,
                    is_markdown_heading,
                });
            }
            current = Some((String::new(), trimmed.starts_with('#')));
            continue;
        }

        let Some((body, _)) = current.as_mut() else {
            continue;
        };
        if !in_code_fence && trimmed.starts_with('#') {
            if let Some((body, is_markdown_heading)) = current.take() {
                bodies.push(FindingsSectionBody {
                    body,
                    is_markdown_heading,
                });
            }
            continue;
        }

        body.push_str(line);
        body.push('\n');
    }

    if let Some((body, is_markdown_heading)) = current {
        bodies.push(FindingsSectionBody {
            body,
            is_markdown_heading,
        });
    }

    bodies
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
        "critical" => Some(Severity::Critical),
        "high" => Some(Severity::High),
        "medium" => Some(Severity::Medium),
        "low" | "info" => Some(Severity::Low),
        _ => priority_severity_from_label(&normalized),
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
    let (structured_entry, body) =
        structured_finding_body(line).map_or((false, line), |body| (true, body.trim_start()));

    if structured_entry && let Some(parsed) = parse_bracketed_finding(body) {
        return Some(parsed);
    }

    if let Some(parsed) =
        parse_severity_prefixed_finding(body, structured_entry || in_findings_section)
    {
        return Some(parsed);
    }

    let severity = default_unlabeled_severity?;
    if !(structured_entry || in_findings_section) {
        return None;
    }
    parse_path_prefixed_finding(body, severity)
}

pub(in crate::review_cmd) fn structured_bracketed_finding_severity(line: &str) -> Option<Severity> {
    bracketed_finding_severity(structured_finding_body(line)?.trim_start())
}

fn bracketed_finding_severity(body: &str) -> Option<Severity> {
    let (label, _) = parse_bracketed_prefix(body.trim_start())?;
    severity_from_label(label)
}

fn structured_finding_body(line: &str) -> Option<&str> {
    strip_numbered_prefix(line).or_else(|| strip_unordered_finding_prefix(line))
}

fn strip_numbered_prefix(line: &str) -> Option<&str> {
    let (index, rest) = line.split_once('.')?;
    if index.is_empty() || !index.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    Some(rest)
}

fn strip_unordered_finding_prefix(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let marker = trimmed.as_bytes().first()?;
    if !matches!(marker, b'-' | b'*') {
        return None;
    }
    let rest = &trimmed[1..];
    rest.chars()
        .next()
        .is_some_and(char::is_whitespace)
        .then_some(rest)
}

fn parse_bracketed_finding(body: &str) -> Option<ParsedProseFinding> {
    let (label, mut rest) = parse_bracketed_prefix(body.trim_start())?;
    let severity = severity_from_label(label)?;
    rest = rest.trim_start();

    if let Some((category, after_category)) = parse_bracketed_prefix(rest)
        && severity_from_label(category).is_none()
        && !category.contains(':')
    {
        rest = after_category.trim_start();
    }

    rest = rest
        .trim_start_matches(|ch: char| ch == ':' || ch == '-' || ch.is_whitespace())
        .trim_start();
    Some(ParsedProseFinding {
        severity,
        file_range: parse_embedded_file_range(rest),
        description: non_empty_or_fallback(rest, body),
    })
}

fn parse_bracketed_prefix(text: &str) -> Option<(&str, &str)> {
    let rest = text.strip_prefix('[')?;
    let end = rest.find(']')?;
    Some((&rest[..end], &rest[end + 1..]))
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

fn parse_embedded_file_range(body: &str) -> Option<ReviewFindingFileRange> {
    body.split(char::is_whitespace)
        .map(|token| token.trim_matches(['`', '(', ')', '[', ']', ',', '.', ';']))
        .find_map(|token| {
            parse_file_line_token(token).map(|(path, start)| ReviewFindingFileRange {
                path,
                start,
                end: None,
            })
        })
}

fn parse_file_line_token(token: &str) -> Option<(String, u32)> {
    let (path, line) = token.rsplit_once(':')?;
    if path.is_empty() || !looks_like_file_path(path) {
        return None;
    }
    let line = line.parse::<u32>().ok()?;
    Some((path.to_string(), line))
}

fn looks_like_file_path(path: &str) -> bool {
    if path.chars().any(char::is_whitespace) {
        return false;
    }
    path.contains('/')
        || path.contains('.')
        || path.eq_ignore_ascii_case("justfile")
        || path == "Makefile"
        || path == "Dockerfile"
}

fn non_empty_or_fallback(value: &str, fallback: &str) -> String {
    if value.trim().is_empty() {
        fallback.trim().to_string()
    } else {
        value.trim().to_string()
    }
}

fn review_finding_payload_eq(left: &ReviewFinding, right: &ReviewFinding) -> bool {
    left.severity == right.severity
        && left.file_ranges == right.file_ranges
        && left.is_regression_of_commit == right.is_regression_of_commit
        && left.suggested_test_scenario == right.suggested_test_scenario
        && left.description == right.description
}

pub(in crate::review_cmd) fn is_findings_header(line: &str) -> bool {
    let normalized = line.trim_start_matches('#').trim();
    let normalized = normalized
        .split_once('(')
        .map_or(normalized, |(heading, _)| heading.trim_end());
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

fn findings_section_body_has_unparseable_text(
    body: &str,
    clean_conclusion: &impl Fn(&str) -> bool,
) -> bool {
    let mut in_code_fence = false;
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_code_fence = !in_code_fence;
            continue;
        }
        if in_code_fence || trimmed.is_empty() {
            continue;
        }
        if clean_conclusion(trimmed) {
            continue;
        }
        if line_has_unparsed_finding_like_structure(trimmed) {
            return true;
        }
    }
    false
}

fn line_has_unparsed_finding_like_structure(line: &str) -> bool {
    let body = structured_finding_body(line).unwrap_or(line).trim_start();
    if body.starts_with('`') {
        return false;
    }
    structured_finding_body(line).is_some()
        || bracketed_finding_severity(body).is_some()
        || leading_severity_from_title(body).is_some()
}

fn priority_severity_from_label(label: &str) -> Option<Severity> {
    let priority = label.strip_prefix('p')?;
    if priority.is_empty() || !priority.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }

    match priority.parse::<u32>().ok()? {
        0 => Some(Severity::Critical),
        1 => Some(Severity::High),
        2 => Some(Severity::Medium),
        _ => Some(Severity::Low),
    }
}
