use std::path::Path;
use std::sync::OnceLock;

use csa_core::types::ReviewDecision;
use csa_session::{FindingsFile, ReviewFinding, ReviewVerdictArtifact, Severity};
use regex::{Captures, Regex};

const ARTIFACT_GENERATION_FINDING_ID: &str = "artifact-generation-001";
const TITLE_MAX_CHARS: usize = 180;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReviewFailureContext {
    pub(crate) first_finding: Option<ReviewFailureFinding>,
    pub(crate) diagnostic: Option<String>,
    pub(crate) fix_route: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReviewFailureFinding {
    pub(crate) severity: Severity,
    pub(crate) category: String,
    pub(crate) location: String,
    pub(crate) title: String,
}

pub(crate) fn review_failure_context_lines(session_dir: &Path) -> Vec<String> {
    let Some(context) = review_failure_context(session_dir) else {
        return Vec::new();
    };
    let mut lines = Vec::new();
    if let Some(finding) = context.first_finding {
        lines.push(format!(
            "First finding: severity={} category={} location={} title={}",
            severity_label(&finding.severity),
            finding.category,
            finding.location,
            finding.title
        ));
    }
    if let Some(diagnostic) = context.diagnostic {
        lines.push(format!("Review diagnostic: {diagnostic}"));
    }
    lines.push(format!("Fix route: {}", context.fix_route));
    lines
}

pub(crate) fn print(session_dir: &Path) {
    for line in review_failure_context_lines(session_dir) {
        println!("{line}");
    }
}

pub(crate) fn review_failure_context_json(session_dir: &Path) -> Option<serde_json::Value> {
    let context = review_failure_context(session_dir)?;
    let first_finding = context.first_finding.map(|finding| {
        serde_json::json!({
            "severity": severity_label(&finding.severity),
            "category": finding.category,
            "location": finding.location,
            "title": finding.title,
        })
    });
    Some(serde_json::json!({
        "first_finding": first_finding,
        "diagnostic": context.diagnostic,
        "fix_route": context.fix_route,
    }))
}

pub(crate) fn insert_json(payload: &mut serde_json::Value, session_dir: &Path) {
    if let Some(context) = review_failure_context_json(session_dir) {
        payload["review_failure_context"] = context;
    }
}

fn review_failure_context(session_dir: &Path) -> Option<ReviewFailureContext> {
    let artifact = read_review_verdict_artifact(session_dir)?;
    if artifact.decision != ReviewDecision::Fail {
        return None;
    }
    let findings = read_findings_file(session_dir).unwrap_or_default();
    let first_finding = findings
        .findings
        .iter()
        .find(|finding| !is_artifact_generation_placeholder(finding))
        .map(review_failure_finding);
    let diagnostic = if first_finding.is_none() {
        artifact
            .failure_reason
            .as_deref()
            .map(compact_diagnostic)
            .or_else(|| Some("review-output parser/internal consistency failure".to_string()))
    } else {
        None
    };
    Some(ReviewFailureContext {
        first_finding,
        diagnostic,
        fix_route: fix_route_label(session_dir, &artifact.session_id),
    })
}

fn read_review_verdict_artifact(session_dir: &Path) -> Option<ReviewVerdictArtifact> {
    let raw =
        std::fs::read_to_string(session_dir.join("output").join("review-verdict.json")).ok()?;
    serde_json::from_str(&raw).ok()
}

fn read_findings_file(session_dir: &Path) -> Option<FindingsFile> {
    let raw = std::fs::read_to_string(session_dir.join("output").join("findings.toml")).ok()?;
    toml::from_str(&raw).ok()
}

fn review_failure_finding(finding: &ReviewFinding) -> ReviewFailureFinding {
    ReviewFailureFinding {
        severity: finding.severity.clone(),
        category: finding_category(finding),
        location: finding_location(finding),
        title: finding_title(finding),
    }
}

fn finding_category(finding: &ReviewFinding) -> String {
    let description = finding.description.trim_start();
    if let Some(rest) = description.strip_prefix('[')
        && let Some(end) = rest.find(']')
    {
        let category = rest[..end].trim();
        if !category.is_empty() {
            return sanitize_finding_text(category, 48);
        }
    }
    "review".to_string()
}

fn sanitize_finding_text(raw: &str, max_chars: usize) -> String {
    let text = csa_session::redact_text_content(raw);
    let text = relativize_absolute_paths(&text);
    clip(text.trim(), max_chars)
}

fn finding_location(finding: &ReviewFinding) -> String {
    let Some(range) = finding.file_ranges.first() else {
        return "unscoped".to_string();
    };
    let sanitized_path = sanitize_finding_text(&range.path, 256);
    let start = range.start;
    match range.end {
        Some(end) if end != start => format!("{sanitized_path}:{start}-{end}"),
        _ => format!("{sanitized_path}:{start}"),
    }
}

fn finding_title(finding: &ReviewFinding) -> String {
    let mut title = finding.description.trim();
    if let Some(rest) = title.strip_prefix('[')
        && let Some(end) = rest.find(']')
    {
        title = rest[end + 1..].trim_start_matches([':', '-', ' ']).trim();
    }
    sanitize_finding_text(title, TITLE_MAX_CHARS)
}

fn absolute_path_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(
            r#"(?x)
                (?P<prefix>^|[\s"'`(<\[])
                (?P<path>
                    /[^\s"'`<>()\[\]{}]+
                    |
                    [A-Za-z]:\\[^\s"'`<>()\[\]{}]+
                )
            "#,
        )
        .expect("absolute path redaction regex must compile")
    })
}

fn relativize_absolute_paths(text: &str) -> String {
    absolute_path_pattern()
        .replace_all(text, |captures: &Captures<'_>| {
            let prefix = captures.name("prefix").map_or("", |value| value.as_str());
            let path = captures.name("path").map_or("", |value| value.as_str());
            format!("{prefix}{}", relativize_absolute_path(path))
        })
        .into_owned()
}

fn relativize_absolute_path(path: &str) -> String {
    let (path, trailing_punctuation) = split_trailing_path_punctuation(path);
    let (path, line_suffix) = split_line_suffix(path);
    let normalized = path.replace('\\', "/");
    let relative = strip_current_dir_prefix(&normalized)
        .or_else(|| strip_env_home_prefix(&normalized))
        .or_else(|| strip_common_home_prefix(&normalized))
        .unwrap_or_else(|| strip_absolute_prefix(&normalized));
    format!("{relative}{line_suffix}{trailing_punctuation}")
}

fn split_trailing_path_punctuation(path: &str) -> (&str, &str) {
    let mut end = path.len();
    while let Some(ch) = path[..end].chars().next_back() {
        if matches!(ch, ',' | ';' | '.') {
            end -= ch.len_utf8();
        } else {
            break;
        }
    }
    (&path[..end], &path[end..])
}

fn split_line_suffix(path: &str) -> (&str, &str) {
    let Some((head, tail)) = path.rsplit_once(':') else {
        return (path, "");
    };
    if is_line_suffix(tail) {
        (head, &path[head.len()..])
    } else {
        (path, "")
    }
}

fn is_line_suffix(value: &str) -> bool {
    let Some((start, end)) = value.split_once('-') else {
        return !value.is_empty() && value.chars().all(|ch| ch.is_ascii_digit());
    };
    !start.is_empty()
        && !end.is_empty()
        && start.chars().all(|ch| ch.is_ascii_digit())
        && end.chars().all(|ch| ch.is_ascii_digit())
}

fn strip_current_dir_prefix(path: &str) -> Option<String> {
    let current_dir = std::env::current_dir().ok()?;
    strip_path_prefix(path, &current_dir)
}

fn strip_env_home_prefix(path: &str) -> Option<String> {
    let home = std::env::var_os("HOME")?;
    strip_path_prefix(path, Path::new(&home))
}

fn strip_path_prefix(path: &str, prefix: &Path) -> Option<String> {
    let prefix = prefix.to_string_lossy().replace('\\', "/");
    let rest = path.strip_prefix(&prefix)?;
    let rest = rest.strip_prefix('/')?;
    if rest.is_empty() {
        None
    } else {
        Some(rest.to_string())
    }
}

fn strip_common_home_prefix(path: &str) -> Option<String> {
    ["/home/", "/Users/"].into_iter().find_map(|prefix| {
        let rest = path.strip_prefix(prefix)?;
        let (_, relative) = rest.split_once('/')?;
        if relative.is_empty() {
            None
        } else {
            Some(relative.to_string())
        }
    })
}

fn strip_absolute_prefix(path: &str) -> String {
    if let Some(rest) = path.strip_prefix('/') {
        return rest.to_string();
    }
    if path.len() > 3
        && path.as_bytes()[1] == b':'
        && path.as_bytes()[2] == b'/'
        && path.as_bytes()[0].is_ascii_alphabetic()
    {
        return path[3..].to_string();
    }
    path.to_string()
}

fn is_artifact_generation_placeholder(finding: &ReviewFinding) -> bool {
    finding.id == ARTIFACT_GENERATION_FINDING_ID
}

fn fix_route_label(session_dir: &Path, session_id: &str) -> String {
    if session_is_retired(session_dir) {
        return "review session is retired/immutable; start a fresh fix from the finding, then run a new exact-head review before push/PR".to_string();
    }

    let Some(value) = read_suggestion_toml(session_dir) else {
        return format!(
            "exact --fix-finding route unavailable (missing output/suggestion.toml); use `csa review --session {session_id} --fix` or start a new fix session, then run a fresh exact-head review"
        );
    };
    let suggestion = value.get("suggestion");
    let action = suggestion
        .and_then(|value| value.get("action"))
        .and_then(toml::Value::as_str)
        .unwrap_or_default();
    if action != "confirm_then_fix_finding" {
        return format!(
            "exact --fix-finding route unavailable (suggestion action '{action}'); use `csa review --session {session_id} --fix` or start a new fix session, then run a fresh exact-head review"
        );
    }
    let command = suggestion
        .and_then(|value| value.get("command_template"))
        .and_then(toml::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            format!("csa review --fix-finding --session {session_id} --prompt-file <path>")
        });
    format!("confirm the finding, then run `{command}`; next review must be a fresh session")
}

fn read_suggestion_toml(session_dir: &Path) -> Option<toml::Value> {
    let raw = std::fs::read_to_string(session_dir.join("output").join("suggestion.toml")).ok()?;
    toml::from_str(&raw).ok()
}

fn session_is_retired(session_dir: &Path) -> bool {
    let raw = std::fs::read_to_string(session_dir.join("state.toml")).ok();
    let Some(raw) = raw else {
        return false;
    };
    let Ok(value) = toml::from_str::<toml::Value>(&raw) else {
        return false;
    };
    value
        .get("phase")
        .and_then(toml::Value::as_str)
        .is_some_and(|phase| phase.eq_ignore_ascii_case("retired"))
}

fn compact_diagnostic(reason: &str) -> String {
    let reason = csa_session::redact_text_content(reason);
    let reason = clip(reason.trim(), 180);
    if reason.is_empty() {
        "review-output parser/internal consistency failure".to_string()
    } else {
        format!("review-output parser/internal consistency failure ({reason})")
    }
}

fn severity_label(severity: &Severity) -> &'static str {
    match severity {
        Severity::Critical => "CRITICAL",
        Severity::High => "HIGH",
        Severity::Medium => "MEDIUM",
        Severity::Low => "LOW",
    }
}

fn clip(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let mut clipped = value
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    clipped.push_str("...");
    clipped
}

#[cfg(test)]
mod tests {
    use super::*;
    use csa_session::{ReviewFindingFileRange, write_findings_toml, write_review_verdict};

    #[test]
    fn failure_context_renders_first_finding_and_exact_fix_route() {
        let temp = tempfile::tempdir().expect("tempdir");
        let artifact = ReviewVerdictArtifact::from_parts(
            "01TESTFAILCTX",
            ReviewDecision::Fail,
            "HAS_ISSUES",
            &[],
            Vec::new(),
        );
        write_review_verdict(temp.path(), &artifact).expect("write verdict");
        write_findings_toml(
            temp.path(),
            &FindingsFile {
                findings: vec![ReviewFinding {
                    id: "F1".to_string(),
                    severity: Severity::High,
                    file_ranges: vec![ReviewFindingFileRange {
                        path: "src/lib.rs".to_string(),
                        start: 42,
                        end: None,
                    }],
                    is_regression_of_commit: None,
                    suggested_test_scenario: None,
                    description: "[correctness] parser accepts stale PASS evidence".to_string(),
                }],
            },
        )
        .expect("write findings");
        std::fs::write(
            temp.path().join("output").join("suggestion.toml"),
            "[suggestion]\naction = \"confirm_then_fix_finding\"\ncommand_template = \"csa review --fix-finding --session 01TESTFAILCTX --prompt-file <path>\"\n",
        )
        .expect("write suggestion");

        let lines = review_failure_context_lines(temp.path());

        assert!(lines.iter().any(|line| line.contains("severity=HIGH")));
        assert!(
            lines
                .iter()
                .any(|line| line.contains("category=correctness"))
        );
        assert!(
            lines
                .iter()
                .any(|line| line.contains("location=src/lib.rs:42"))
        );
        assert!(
            lines
                .iter()
                .any(|line| line.contains("csa review --fix-finding --session 01TESTFAILCTX"))
        );
    }

    #[test]
    fn failure_context_points_retired_session_to_fresh_fix_path() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut artifact = ReviewVerdictArtifact::from_parts(
            "01TESTRETIREDCTX",
            ReviewDecision::Fail,
            "HAS_ISSUES",
            &[],
            Vec::new(),
        );
        artifact.failure_reason = Some("fail_verdict_empty_findings_artifact".to_string());
        write_review_verdict(temp.path(), &artifact).expect("write verdict");
        std::fs::write(temp.path().join("state.toml"), "phase = \"retired\"\n")
            .expect("write state");

        let lines = review_failure_context_lines(temp.path());

        assert!(
            lines
                .iter()
                .any(|line| line.contains("review-output parser/internal consistency failure"))
        );
        assert!(lines.iter().any(|line| line.contains("retired/immutable")));
        assert!(lines.iter().all(|line| !line.contains("pr-bot")));
    }

    #[test]
    fn issue_2516_finding_title_redacts_embedded_credential() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_failed_review_context(
            temp.path(),
            "[security] command leaks OPENAI_API_KEY=providerfixture12345 into logs",
        );

        let title = json_finding_title(temp.path());

        assert!(title.contains("[REDACTED]"));
        assert!(!title.contains("providerfixture12345"));
    }

    #[test]
    fn issue_2516_finding_title_shortens_absolute_paths_to_relative_paths() {
        let temp = tempfile::tempdir().expect("tempdir");
        let current_dir = std::env::current_dir().expect("current dir");
        let absolute_path = current_dir
            .join("crates/cli-sub-agent/src/review_failure_context.rs")
            .display()
            .to_string();
        write_failed_review_context(
            temp.path(),
            &format!("[security] leaked path {absolute_path}:144 in review output"),
        );

        let title = json_finding_title(temp.path());

        assert_eq!(
            title,
            "leaked path crates/cli-sub-agent/src/review_failure_context.rs:144 in review output"
        );
        assert!(!title.contains(&current_dir.display().to_string()));
        assert!(!title.contains("/home/"));
    }

    #[test]
    fn issue_2516_normal_finding_title_passes_through_unchanged() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_failed_review_context(
            temp.path(),
            "[correctness] parser accepts stale PASS evidence",
        );

        let title = json_finding_title(temp.path());

        assert_eq!(title, "parser accepts stale PASS evidence");
    }

    #[test]
    fn issue_2516_terminal_and_json_titles_use_same_sanitized_value() {
        let temp = tempfile::tempdir().expect("tempdir");
        let current_dir = std::env::current_dir().expect("current dir");
        let absolute_path = current_dir
            .join("crates/cli-sub-agent/src/review_failure_context.rs")
            .display()
            .to_string();
        write_failed_review_context(
            temp.path(),
            &format!(
                "[security] leaked OPENAI_API_KEY=providerfixture12345 at {absolute_path}:144"
            ),
        );

        let terminal_title = terminal_finding_title(temp.path());
        let json_title = json_finding_title(temp.path());

        assert_eq!(terminal_title, json_title);
        assert_eq!(
            json_title,
            "leaked [REDACTED] at crates/cli-sub-agent/src/review_failure_context.rs:144"
        );
    }

    fn write_failed_review_context(session_dir: &Path, description: &str) {
        let artifact = ReviewVerdictArtifact::from_parts(
            "01TESTISSUE2516",
            ReviewDecision::Fail,
            "HAS_ISSUES",
            &[],
            Vec::new(),
        );
        write_review_verdict(session_dir, &artifact).expect("write verdict");
        write_findings_toml(
            session_dir,
            &FindingsFile {
                findings: vec![ReviewFinding {
                    id: "F1".to_string(),
                    severity: Severity::High,
                    file_ranges: vec![ReviewFindingFileRange {
                        path: "src/lib.rs".to_string(),
                        start: 42,
                        end: None,
                    }],
                    is_regression_of_commit: None,
                    suggested_test_scenario: None,
                    description: description.to_string(),
                }],
            },
        )
        .expect("write findings");
    }

    fn json_finding_title(session_dir: &Path) -> String {
        review_failure_context_json(session_dir)
            .expect("review failure context")
            .pointer("/first_finding/title")
            .and_then(serde_json::Value::as_str)
            .expect("json finding title")
            .to_string()
    }

    fn terminal_finding_title(session_dir: &Path) -> String {
        let line = review_failure_context_lines(session_dir)
            .into_iter()
            .find(|line| line.starts_with("First finding:"))
            .expect("terminal finding line");
        line.split_once(" title=")
            .map(|(_, title)| title.to_string())
            .expect("terminal finding title")
    }
}
