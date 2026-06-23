use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::review_cmd::detect_bounded_clean_verdict_token;
use anyhow::Result;
use chrono::{DateTime, Utc};
use csa_core::types::ReviewDecision;
use csa_session::{ReviewVerdictArtifact, SessionResult};

#[path = "session_observability_legacy_review_pass_clean.rs"]
mod clean_summary;

pub(super) fn recover_legacy_plain_pass_review_sidecars_from_dir(
    session_dir: &Path,
    result: &mut SessionResult,
) -> Result<bool> {
    if review_verdict_artifact_exists(session_dir) {
        return Ok(false);
    }
    let session = match read_session_state(session_dir) {
        Ok(session) => session,
        Err(error) => {
            tracing::debug!(
                path = %session_dir.display(),
                error = %error,
                "skipping legacy review sidecar recovery because session state is unreadable"
            );
            None
        }
    };
    let Some(session) = session else {
        return Ok(false);
    };
    let project_root = PathBuf::from(&session.project_path);
    recover_legacy_plain_pass_review_sidecars_for_session(
        &project_root,
        session_dir,
        result,
        &session,
    )
}

pub(super) fn recover_legacy_plain_pass_review_sidecars(
    project_root: &Path,
    session_id: &str,
    session_dir: &Path,
    result: &mut SessionResult,
) -> Result<bool> {
    if review_verdict_artifact_exists(session_dir) {
        return Ok(false);
    }
    let session = match read_session_state(session_dir) {
        Ok(session) => session,
        Err(error) => {
            tracing::debug!(
                session_id,
                path = %session_dir.display(),
                error = %error,
                "skipping legacy review sidecar recovery because session state is unreadable"
            );
            None
        }
    };
    let Some(session) = session else {
        return Ok(false);
    };
    recover_legacy_plain_pass_review_sidecars_for_session(
        project_root,
        session_dir,
        result,
        &session,
    )
}

fn recover_legacy_plain_pass_review_sidecars_for_session(
    project_root: &Path,
    session_dir: &Path,
    result: &mut SessionResult,
    session: &csa_session::MetaSessionState,
) -> Result<bool> {
    if review_verdict_artifact_exists(session_dir) || !is_review_session(session) {
        return Ok(false);
    }

    let Some(summary) = legacy_review_summary_text(session_dir, result) else {
        return Ok(false);
    };
    let blocking_summary = super::human_review_summary_requires_failed_gate(session_dir, &summary);
    let summary_decision = legacy_plain_review_summary_decision(&summary);
    let Some(decision) =
        recovered_legacy_review_sidecar_decision(summary_decision, blocking_summary)
    else {
        return Ok(false);
    };
    let existing_meta = read_review_meta(session_dir);
    if decision == ReviewDecision::Pass
        && existing_meta
            .as_ref()
            .is_some_and(csa_session::ReviewSessionMeta::requires_fail_closed_verdict)
    {
        return Ok(false);
    }

    let scope = canonical_review_scope(session);
    let head_sha = session
        .vcs_identity
        .as_ref()
        .and_then(|identity| identity.commit_id.clone())
        .or_else(|| session.git_head_at_creation.clone())
        .unwrap_or_default();
    let tool = non_empty(result.tool.as_str())
        .map(ToOwned::to_owned)
        .or_else(|| latest_session_tool(session))
        .unwrap_or_else(|| "unknown".to_string());
    let timestamp = result.completed_at;
    let diff_fingerprint =
        trusted_legacy_plain_review_diff_fingerprint(project_root, &scope, session);

    let legacy_verdict = legacy_verdict_for_review_decision(decision);
    let mut verdict = ReviewVerdictArtifact::from_parts(
        session.meta_session_id.clone(),
        decision,
        legacy_verdict,
        &[],
        Vec::new(),
    );
    verdict.timestamp = timestamp;
    csa_session::write_review_verdict(session_dir, &verdict)?;

    let meta = csa_session::ReviewSessionMeta {
        session_id: session.meta_session_id.clone(),
        head_sha,
        decision: decision.as_str().to_string(),
        verdict: legacy_verdict.to_string(),
        review_mode: None,
        status_reason: None,
        routed_to: None,
        primary_failure: None,
        failure_reason: None,
        tool,
        scope,
        exit_code: crate::verdict_exit_code::exit_code_from_review_decision(decision),
        fix_attempted: false,
        fix_rounds: 0,
        review_iterations: 1,
        timestamp,
        diff_fingerprint,
        fix_convergence: None,
    };
    csa_session::write_review_meta(session_dir, &meta)?;

    Ok(true)
}

fn recovered_legacy_review_sidecar_decision(
    summary_decision: Option<LegacyPlainReviewSummaryDecision>,
    blocking_summary: bool,
) -> Option<ReviewDecision> {
    if blocking_summary {
        return Some(ReviewDecision::Fail);
    }

    match summary_decision {
        Some(LegacyPlainReviewSummaryDecision::Decision(decision)) => Some(decision),
        Some(LegacyPlainReviewSummaryDecision::InvalidRecognizedLabel) => {
            // A recognized-but-ambiguous legacy decision is still a review
            // candidate. Persist it as fail-closed metadata so its timestamp
            // can supersede older PASS evidence in check-verdict ordering.
            Some(ReviewDecision::Uncertain)
        }
        None => None,
    }
}

fn legacy_verdict_for_review_decision(decision: ReviewDecision) -> &'static str {
    match decision {
        ReviewDecision::Pass => "CLEAN",
        ReviewDecision::Fail => "HAS_ISSUES",
        ReviewDecision::Skip => "SKIP",
        ReviewDecision::Uncertain => "UNCERTAIN",
        ReviewDecision::Unavailable => "UNAVAILABLE",
    }
}

fn review_verdict_artifact_exists(session_dir: &Path) -> bool {
    session_dir
        .join("output")
        .join("review-verdict.json")
        .is_file()
}

fn read_session_state(session_dir: &Path) -> Result<Option<csa_session::MetaSessionState>> {
    let path = session_dir.join("state.toml");
    if !path.is_file() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path)?;
    Ok(Some(toml::from_str(&raw)?))
}

fn read_review_meta(session_dir: &Path) -> Option<csa_session::ReviewSessionMeta> {
    fs::read_to_string(session_dir.join("review_meta.json"))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
}

pub(super) fn is_review_session_dir(session_dir: &Path) -> bool {
    read_session_state(session_dir)
        .ok()
        .flatten()
        .is_some_and(|session| is_review_session(&session))
}

pub(super) fn is_review_session(session: &csa_session::MetaSessionState) -> bool {
    if matches!(
        session.task_context.task_type.as_deref().map(str::trim),
        Some("review" | "reviewer_sub_session")
    ) {
        return true;
    }

    session
        .description
        .as_deref()
        .is_some_and(is_legacy_review_description)
}

fn is_legacy_review_description(description: &str) -> bool {
    let description = description.trim();
    description.eq_ignore_ascii_case("review")
        || description.eq_ignore_ascii_case("initializing daemon review")
        || has_ascii_prefix_ignore_case(description, "review:")
        || strip_multi_reviewer_scope(description).is_some()
        || has_ascii_prefix_ignore_case(description, "code-review:")
}

fn canonical_review_scope(session: &csa_session::MetaSessionState) -> String {
    session
        .description
        .as_deref()
        .and_then(extract_review_scope)
        .or_else(|| {
            session
                .description
                .as_deref()
                .map(str::trim)
                .filter(|description| !description.is_empty())
        })
        .unwrap_or("unknown")
        .to_string()
}

fn extract_review_scope(description: &str) -> Option<&str> {
    let description = description.trim();
    strip_ascii_prefix_ignore_case(description, "review:")
        .or_else(|| strip_multi_reviewer_scope(description))
        .or_else(|| strip_ascii_prefix_ignore_case(description, "code-review:"))
        .map(str::trim)
        .filter(|scope| !scope.is_empty())
}

fn strip_multi_reviewer_scope(description: &str) -> Option<&str> {
    let rest = strip_ascii_prefix_ignore_case(description, "review[")?;
    let close_bracket = rest.find(']')?;
    let reviewer_index = &rest[..close_bracket];
    if reviewer_index.is_empty() || !reviewer_index.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let after_bracket = rest.get(close_bracket + 1..)?.trim_start();
    after_bracket.strip_prefix(':').map(str::trim)
}

fn has_ascii_prefix_ignore_case(value: &str, prefix: &str) -> bool {
    strip_ascii_prefix_ignore_case(value, prefix).is_some()
}

fn strip_ascii_prefix_ignore_case<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    let candidate = value.get(..prefix.len())?;
    if candidate.eq_ignore_ascii_case(prefix) {
        value.get(prefix.len()..)
    } else {
        None
    }
}

fn latest_session_tool(session: &csa_session::MetaSessionState) -> Option<String> {
    session
        .tools
        .iter()
        .max_by_key(|(_, state)| state.updated_at)
        .map(|(tool, _)| tool.clone())
}

fn non_empty(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

fn legacy_review_summary_text(session_dir: &Path, result: &SessionResult) -> Option<String> {
    fs::read_to_string(session_dir.join("output").join("summary.md"))
        .ok()
        .filter(|summary| !summary.trim().is_empty())
        .or_else(|| non_empty(result.summary.as_str()).map(ToOwned::to_owned))
}

/// Plain legacy review summaries do not carry machine-readable diff identity.
/// Never stamp the current `main...HEAD` fingerprint onto them unless the
/// persisted session identity plus git reflog prove the checked-out HEAD and the
/// base ref still match what the session saw at creation time. Without this
/// proof, an old legacy verdict could be recovered after `main` advanced and
/// falsely affect the current check-verdict fingerprint guard.
fn trusted_legacy_plain_review_diff_fingerprint(
    project_root: &Path,
    scope: &str,
    session: &csa_session::MetaSessionState,
) -> Option<String> {
    let session_head = session_head_sha(session)?;
    let current_head = csa_session::detect_git_head(project_root)?;
    if session_head != current_head {
        return None;
    }

    let base_ref = review_scope_base_ref(scope)?;
    let base_ref_proven = base_ref_unchanged_since_before_session_creation(
        project_root,
        base_ref,
        session.created_at,
    );
    if !base_ref_proven {
        return None;
    }

    crate::review_cmd::compute_review_diff_fingerprint(project_root, scope)
}

fn session_head_sha(session: &csa_session::MetaSessionState) -> Option<String> {
    session
        .vcs_identity
        .as_ref()
        .and_then(|identity| identity.commit_id.as_deref().and_then(non_empty))
        .or_else(|| session.git_head_at_creation.as_deref().and_then(non_empty))
        .map(ToOwned::to_owned)
}

fn review_scope_base_ref(scope: &str) -> Option<&str> {
    let scope = scope.trim();
    let range = scope.strip_prefix("range:")?;
    range_base_ref(range)
}

fn range_base_ref(range: &str) -> Option<&str> {
    let range = range.trim();
    let (base, head) = range.split_once("...").or_else(|| range.split_once(".."))?;
    let base = base.trim();
    let head = head.trim();
    (!base.is_empty() && head == "HEAD").then_some(base)
}

fn base_ref_unchanged_since_before_session_creation(
    project_root: &Path,
    base_ref: &str,
    session_created_at: DateTime<Utc>,
) -> bool {
    let Some(current_base) = git_commit_at(project_root, base_ref) else {
        return false;
    };
    let Some(latest_reflog_entry) = latest_ref_reflog_entry(project_root, base_ref) else {
        return false;
    };

    // Git reflog selectors and entries have second-granularity update timestamps.
    // A ref update in the same second as session creation may have happened
    // before or after the review session was created, so equality is ambiguous.
    // Only trust legacy PASS backfill when the latest base-ref update is strictly
    // before the session's creation second and therefore could not be a
    // same-second advance.
    latest_reflog_entry.commit == current_base
        && latest_reflog_entry.timestamp_secs < session_created_at.timestamp()
}

struct ReflogEntry {
    commit: String,
    timestamp_secs: i64,
}

fn latest_ref_reflog_entry(project_root: &Path, ref_name: &str) -> Option<ReflogEntry> {
    let output = Command::new("git")
        .args([
            "reflog",
            "show",
            "-n",
            "1",
            "--date=unix",
            "--format=%H%x00%gD",
            "--end-of-options",
            ref_name,
        ])
        .current_dir(project_root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    let line = stdout.trim_end_matches(&['\r', '\n'][..]);
    let (commit, reflog_selector) = line.split_once('\0')?;
    let commit = non_empty(commit)?.to_owned();
    let timestamp_secs = parse_unix_reflog_selector_timestamp_secs(reflog_selector)?;
    Some(ReflogEntry {
        commit,
        timestamp_secs,
    })
}

fn parse_unix_reflog_selector_timestamp_secs(reflog_selector: &str) -> Option<i64> {
    let reflog_selector = reflog_selector.trim();
    let reflog_selector = reflog_selector.strip_suffix('}')?;
    let (_, timestamp_secs) = reflog_selector.rsplit_once("@{")?;
    timestamp_secs.trim().parse().ok()
}

fn git_commit_at(project_root: &Path, rev: &str) -> Option<String> {
    let output = Command::new("git")
        .args([
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{rev}^{{commit}}"),
        ])
        .current_dir(project_root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    non_empty(stdout.as_str()).map(ToOwned::to_owned)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LegacyPlainReviewSummaryDecision {
    Decision(ReviewDecision),
    InvalidRecognizedLabel,
}

fn legacy_plain_review_summary_decision(summary: &str) -> Option<LegacyPlainReviewSummaryDecision> {
    let mut pass = false;
    for line in summary
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let Some(decision) = labeled_review_decision(line) else {
            if detect_bounded_clean_verdict_token(line) {
                if has_ambiguous_or_compound_clean_verdict_phrase(line) {
                    return Some(LegacyPlainReviewSummaryDecision::InvalidRecognizedLabel);
                }
                pass = true;
            } else if clean_summary::unlabeled_clean_decision_with_clean_explanation(line) {
                pass = true;
            }
            continue;
        };
        match decision {
            LegacyPlainReviewSummaryDecision::Decision(ReviewDecision::Pass) => pass = true,
            LegacyPlainReviewSummaryDecision::Decision(decision) => {
                return Some(LegacyPlainReviewSummaryDecision::Decision(decision));
            }
            LegacyPlainReviewSummaryDecision::InvalidRecognizedLabel => {
                return Some(LegacyPlainReviewSummaryDecision::InvalidRecognizedLabel);
            }
        }
    }
    pass.then_some(LegacyPlainReviewSummaryDecision::Decision(
        ReviewDecision::Pass,
    ))
}

fn labeled_review_decision(line: &str) -> Option<LegacyPlainReviewSummaryDecision> {
    // Keep the generic labels aligned with the bounded verdict-token parser
    // (`Verdict:`, `Decision:`, `Status:`, `Result:`, `Review:`), while still
    // accepting older more-specific legacy labels.
    const LABELS: &[&str] = &[
        "review result",
        "review verdict",
        "review decision",
        "final verdict",
        "final decision",
        "verdict",
        "decision",
        "status",
        "result",
        "review",
    ];

    let line = line
        .trim_start_matches(|ch: char| ch == '#' || ch == '*' || ch == '-' || ch.is_whitespace())
        .trim_start();
    let lower = line.to_ascii_lowercase();
    for label in LABELS {
        let Some(rest) = lower.strip_prefix(label) else {
            continue;
        };
        let Some(original_rest) = line.get(label.len()..) else {
            continue;
        };
        let trimmed = rest.trim_start();
        if !(trimmed.starts_with(':') || trimmed.starts_with('=')) {
            continue;
        }
        let value = original_rest
            .trim_start()
            .trim_start_matches(&[':', '='][..])
            .trim_start();
        let parsed_decision =
            review_decision_value(value).map(LegacyPlainReviewSummaryDecision::Decision);
        if matches!(
            parsed_decision,
            Some(LegacyPlainReviewSummaryDecision::Decision(
                ReviewDecision::Pass
            ))
        ) && has_ambiguous_or_compound_clean_verdict_phrase(line)
        {
            return Some(LegacyPlainReviewSummaryDecision::InvalidRecognizedLabel);
        }
        let parsed_decision = parsed_decision.or_else(|| {
            (detect_bounded_clean_verdict_token(line)
                && !has_ambiguous_or_compound_clean_verdict_phrase(line))
            .then_some(LegacyPlainReviewSummaryDecision::Decision(
                ReviewDecision::Pass,
            ))
        });
        return Some(
            parsed_decision.unwrap_or(LegacyPlainReviewSummaryDecision::InvalidRecognizedLabel),
        );
    }
    None
}

fn has_ambiguous_or_compound_clean_verdict_phrase(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    ["clean-up", "clean up", "pass-through", "pass through"]
        .iter()
        .any(|phrase| lower.contains(phrase))
        || contains_pass_fail_separator_phrase(&lower)
}

fn contains_pass_fail_separator_phrase(lower: &str) -> bool {
    for (index, _) in lower.match_indices("pass") {
        let Some(after_pass) = lower.get(index + "pass".len()..) else {
            continue;
        };
        let after_pass = after_pass.trim_start();
        let mut chars = after_pass.chars();
        let Some(separator) = chars.next() else {
            continue;
        };
        if !is_compound_decision_separator(separator) {
            continue;
        }
        let Some(after_separator) = after_pass.get(separator.len_utf8()..) else {
            continue;
        };
        if after_separator.trim_start().starts_with("fail") {
            return true;
        }
    }
    false
}

fn review_decision_value(value: &str) -> Option<ReviewDecision> {
    let value = value
        .trim_end()
        .trim_start_matches(is_legacy_decision_prefix_char);
    let token_end = value
        .char_indices()
        .find_map(|(index, ch)| (!is_review_decision_token_char(ch)).then_some(index))
        .unwrap_or(value.len());
    let token = value.get(..token_end).filter(|token| !token.is_empty())?;
    let rest = value.get(token_end..).unwrap_or_default();
    let decision = review_decision_token(token)?;
    if !has_standalone_decision_suffix(rest) {
        return None;
    }

    Some(decision)
}

fn review_decision_token(token: &str) -> Option<ReviewDecision> {
    if token.eq_ignore_ascii_case("pass") || token.eq_ignore_ascii_case("clean") {
        Some(ReviewDecision::Pass)
    } else if token.eq_ignore_ascii_case("fail") || token.eq_ignore_ascii_case("has_issues") {
        Some(ReviewDecision::Fail)
    } else if token.eq_ignore_ascii_case("uncertain") {
        Some(ReviewDecision::Uncertain)
    } else if token.eq_ignore_ascii_case("unavailable") {
        Some(ReviewDecision::Unavailable)
    } else if token.eq_ignore_ascii_case("skip") {
        Some(ReviewDecision::Skip)
    } else {
        None
    }
}

fn is_legacy_decision_prefix_char(ch: char) -> bool {
    ch.is_whitespace() || matches!(ch, '*' | '`' | '"' | '\'' | '(' | '[' | '{')
}

fn is_review_decision_token_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

fn has_standalone_decision_suffix(rest: &str) -> bool {
    if rest.trim().is_empty() {
        return true;
    }
    if has_attached_compound_decision_suffix(rest) {
        return false;
    }
    if has_explanatory_separator_decision_suffix(rest) {
        return true;
    }
    if rest.chars().next().is_some_and(|ch| ch.is_whitespace()) {
        return false;
    }
    rest.chars()
        .next()
        .is_some_and(is_legacy_standalone_decision_terminator)
}

fn has_attached_compound_decision_suffix(rest: &str) -> bool {
    let mut chars = rest.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    is_compound_decision_separator(first) && chars.next().is_some_and(is_review_decision_token_char)
}

fn has_explanatory_separator_decision_suffix(rest: &str) -> bool {
    let trimmed = rest.trim_start();
    let mut chars = trimmed.chars();
    let Some(separator) = chars.next() else {
        return false;
    };
    if !is_compound_decision_separator(separator) {
        return false;
    }
    let after_separator = &trimmed[separator.len_utf8()..];
    if after_separator
        .chars()
        .next()
        .is_some_and(is_review_decision_token_char)
    {
        return false;
    }
    !starts_with_non_passing_review_decision(after_separator)
}

fn starts_with_non_passing_review_decision(value: &str) -> bool {
    let value = value.trim_start_matches(is_legacy_decision_prefix_char);
    let token_end = value
        .char_indices()
        .find_map(|(index, ch)| (!is_review_decision_token_char(ch)).then_some(index))
        .unwrap_or(value.len());
    let Some(token) = value.get(..token_end).filter(|token| !token.is_empty()) else {
        return false;
    };
    let rest = value.get(token_end..).unwrap_or_default();
    if pass_like_decision_has_compound_word_suffix(token, rest) {
        return true;
    }
    !matches!(
        review_decision_token(token),
        Some(ReviewDecision::Pass) | None
    )
}

fn pass_like_decision_has_compound_word_suffix(token: &str, rest: &str) -> bool {
    (token.eq_ignore_ascii_case("clean") && rest_starts_with_word(rest, "up"))
        || (token.eq_ignore_ascii_case("pass") && rest_starts_with_word(rest, "through"))
}

fn rest_starts_with_word(rest: &str, word: &str) -> bool {
    let rest = rest.trim_start();
    let Some(head) = rest.get(..word.len()) else {
        return false;
    };
    if !head.eq_ignore_ascii_case(word) {
        return false;
    }
    let after_word = rest.get(word.len()..).unwrap_or_default();
    after_word
        .chars()
        .next()
        .is_none_or(|ch| !is_review_decision_token_char(ch))
}

fn is_compound_decision_separator(ch: char) -> bool {
    matches!(
        ch,
        '-' | '/' | '\\' | '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}'
    )
}

fn is_legacy_standalone_decision_terminator(ch: char) -> bool {
    matches!(
        ch,
        '.' | '!' | '?' | '*' | '`' | '"' | '\'' | ')' | ']' | '}'
    )
}

#[cfg(test)]
#[path = "session_observability_legacy_review_pass_tests.rs"]
mod tests;
