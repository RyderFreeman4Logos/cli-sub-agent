use std::{fs, path::Path};

use anyhow::Result;

use super::text::extract_review_text;

pub(super) fn detect_prose_clean_conclusion(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    [
        "no blocking",
        "no issues found",
        "no issues were found",
        "no actionable findings",
        "ship-ready",
        "ship ready",
        "\u{672a}\u{53d1}\u{73b0}\u{9700}\u{8981}\u{963b}\u{585e}\u{5408}\u{5e76}",
        "\u{672a}\u{53d1}\u{73b0}\u{963b}\u{585e}",
    ]
    .iter()
    .any(|phrase| lower.contains(phrase))
        || (lower.contains("no correctness")
            && [
                "issue", "issues", "problem", "problems", "finding", "findings",
            ]
            .iter()
            .any(|noun| lower.contains(noun)))
        || verdict_token_pass_or_clean(text)
}

/// Verdict tokens that affirmatively conclude a clean/passing review.
const PASS_VERDICT_TOKENS: &[&str] = &["PASS", "CLEAN"];

/// Verdict tokens that affirmatively conclude a failing review (#1675).
const FAIL_VERDICT_TOKENS: &[&str] = &["FAIL", "HAS_ISSUES", "REJECT"];

/// Case-matching policy for verdict tokens.
///
/// The PASS/CLEAN and FAIL detectors deliberately differ — both erring toward
/// FAIL, like the fail-closed/fail-open asymmetry documented on
/// [`review_contains_prose_fail_conclusion`]:
/// - PASS/CLEAN is fail-OPEN — an affirmative clean conclusion SUPPRESSES the
///   #1675 fail-closed path — so it matches [`MatchCase::Sensitive`]
///   (uppercase-only). A prior codex finding showed case-insensitive clean
///   detection misread mixed-case prose (`Verdict: Pass`, "cannot pass yet") as
///   a clean conclusion and unblocked merges on uncertain evidence.
/// - FAIL is fail-CLOSED, so it matches [`MatchCase::Insensitive`] to agree with
///   the CLI's own verdict parser (`review_consensus::contains_verdict_token`,
///   `eq_ignore_ascii_case`): `Verdict: Fail` sets meta=Fail, so this detector
///   must catch it too or the #1675 lost-evidence path reopens for case variants
///   the CLI already recognizes.
#[derive(Clone, Copy)]
enum MatchCase {
    Sensitive,
    Insensitive,
}

impl MatchCase {
    fn token_eq(self, candidate: &str, token: &str) -> bool {
        match self {
            MatchCase::Sensitive => candidate == token,
            MatchCase::Insensitive => candidate.eq_ignore_ascii_case(token),
        }
    }

    fn prefix_eq(self, head: &[u8], token: &[u8]) -> bool {
        match self {
            MatchCase::Sensitive => head == token,
            MatchCase::Insensitive => head.eq_ignore_ascii_case(token),
        }
    }
}

fn verdict_token_pass_or_clean(text: &str) -> bool {
    verdict_token_matches(text, PASS_VERDICT_TOKENS, MatchCase::Sensitive)
}

/// Detect an affirmative FAIL verdict token (`FAIL`/`HAS_ISSUES`/`REJECT`),
/// mirroring [`verdict_token_pass_or_clean`]. Matches ONLY bounded verdict tokens
/// (bare line, `Verdict:`-labeled, or `**`/`__`-emphasized) — never the substring
/// "fail" — so prose like "the test no longer fails" is not read as a FAIL
/// conclusion (#1675 precision requirement). Unlike the PASS/CLEAN detector this
/// is case-INsensitive; see [`MatchCase`].
fn verdict_token_fail(text: &str) -> bool {
    verdict_token_matches(text, FAIL_VERDICT_TOKENS, MatchCase::Insensitive)
}

/// Scan each line for one of `tokens` appearing as a bounded verdict token,
/// either standalone, after a `Verdict:`-style label, or `**`/`__`-emphasized.
/// `case` selects verdict-token case-matching; the surrounding `Verdict:`-style
/// label is always matched case-insensitively (only the token honors `case`).
fn verdict_token_matches(text: &str, tokens: &[&str], case: MatchCase) -> bool {
    text.lines().any(|line| {
        let trimmed = line.trim();
        is_verdict_token(trimmed, tokens, case)
            || has_emphasized_verdict_token_prefix(trimmed, tokens, case)
            || line_has_labeled_verdict_token(trimmed, tokens, case)
    })
}

fn is_verdict_token(text: &str, tokens: &[&str], case: MatchCase) -> bool {
    // Trim ASCII punctuation and whitespace so a standalone verdict token survives
    // surrounding punctuation — `FAIL:`, `PASS.`, `(CLEAN)` — which the CLI's
    // consensus parser already counts as a blocking/clean verdict (missing `:`
    // reopened the #1675 lost-evidence path for colon-terminated verdict lines).
    // The trim set is restricted to ASCII punctuation/whitespace, NOT the consensus
    // parser's full `!is_ascii_alphanumeric()` delimiter class: trimming every
    // non-ASCII char would corrupt multilingual prose — e.g. `PASS нет`
    // (Russian "no") would lose `нет` and reduce to a false PASS verdict
    // (cloud-review finding). A multi-word line ("The test PASS rate is 100%")
    // never reduces to a bare token, so exact equality after trimming stays the
    // precision guard. Underscore is NOT trimmed (the consensus parser treats it
    // as a word char): `HAS_ISSUES` stays one token and `CLEAN_UP` is not a CLEAN
    // verdict.
    let trimmed =
        text.trim_matches(|c: char| (c.is_ascii_punctuation() || c.is_whitespace()) && c != '_');
    tokens.iter().any(|token| case.token_eq(trimmed, token))
}

fn line_has_labeled_verdict_token(line: &str, tokens: &[&str], case: MatchCase) -> bool {
    const LABELS: &[&str] = &["Verdict:", "Decision:", "Status:", "Result:", "Review:"];

    let bytes = line.as_bytes();
    for index in 0..bytes.len() {
        if index > 0 && is_ascii_word_byte(bytes[index - 1]) {
            continue;
        }

        for label in LABELS {
            let label_bytes = label.as_bytes();
            if bytes[index..].len() < label_bytes.len() {
                continue;
            }
            if !bytes[index..index + label_bytes.len()].eq_ignore_ascii_case(label_bytes) {
                continue;
            }

            let rest = &line[index + label_bytes.len()..];
            if is_verdict_token(rest, tokens, case)
                || has_emphasized_verdict_token_prefix(rest.trim_start(), tokens, case)
                || has_verdict_token_prefix(
                    rest.trim_start_matches(|c: char| c.is_whitespace() || c == '*' || c == '_'),
                    tokens,
                    case,
                )
            {
                return true;
            }
        }
    }

    false
}

fn has_verdict_token_prefix(text: &str, tokens: &[&str], case: MatchCase) -> bool {
    tokens
        .iter()
        .any(|token| strip_token_prefix(text, token, case).is_some_and(verdict_token_is_bounded))
}

/// [`str::strip_prefix`] for an ASCII verdict `token`, honoring `case`.
///
/// Verdict tokens are uppercase ASCII, so a matched prefix is exactly
/// `token.len()` bytes and lands on a char boundary. Returns the remainder after
/// the token, or `None` when `text` does not start with `token` under `case`.
fn strip_token_prefix<'a>(text: &'a str, token: &str, case: MatchCase) -> Option<&'a str> {
    let head = text.as_bytes().get(..token.len())?;
    if case.prefix_eq(head, token.as_bytes()) {
        // The token is ASCII, so a matched head means `token.len()` lands on a
        // char boundary; slicing here cannot panic. (Taken only on match, so the
        // index is never evaluated for a non-boundary split.)
        Some(&text[token.len()..])
    } else {
        None
    }
}

fn has_emphasized_verdict_token_prefix(text: &str, tokens: &[&str], case: MatchCase) -> bool {
    ["**", "__"].iter().any(|marker| {
        tokens.iter().any(|token| {
            text.strip_prefix(marker)
                .and_then(|rest| strip_token_prefix(rest, token, case))
                .and_then(|rest| rest.strip_prefix(marker))
                .is_some_and(verdict_token_is_bounded)
        })
    })
}

fn is_verdict_token_continuation(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn verdict_token_is_bounded(rest: &str) -> bool {
    let mut chars = rest.chars();
    match chars.next() {
        None => true,
        Some(c) if is_verdict_token_continuation(c) => false,
        Some('-') | Some('/') => chars
            .next()
            .is_none_or(|next| !is_verdict_token_continuation(next)),
        _ => true,
    }
}

fn is_ascii_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

pub(super) fn review_contains_prose_clean_conclusion(session_dir: &Path) -> Result<bool> {
    if let Some(summary) = csa_session::read_section(session_dir, "summary")?
        && detect_prose_clean_conclusion(&summary)
    {
        return Ok(true);
    }

    let full_output_path = session_dir.join("output").join("full.md");
    if !full_output_path.exists() {
        return Ok(false);
    }

    let raw_output = fs::read_to_string(&full_output_path)
        .map_err(|error| anyhow::anyhow!("read {}: {error}", full_output_path.display()))?;
    let review_text = extract_review_text(&raw_output).unwrap_or(raw_output);
    Ok(detect_prose_clean_conclusion(&review_text))
}

/// Detect whether review prose AFFIRMATIVELY concludes FAIL via a bounded verdict
/// token (`FAIL`/`HAS_ISSUES`/`REJECT`). Unlike [`detect_prose_clean_conclusion`],
/// this matches ONLY verdict tokens (bare/labeled/emphasized) — never the substring
/// "fail" — so benign prose like "the test no longer fails" is not misread as a FAIL
/// verdict (#1675). Used to fail-closed when a real prose FAIL lost its structured
/// findings.
pub(super) fn detect_prose_fail_conclusion(text: &str) -> bool {
    verdict_token_fail(text)
}

/// Whether the review's persisted prose affirmatively concludes FAIL.
///
/// Scans every place a reviewer might record a FAIL verdict in two passes:
///
/// 1. ALL persisted `summary` and `details` sections, via
///    [`csa_session::read_all_sections`] rather than [`csa_session::read_section`]
///    (the latter returns only the FIRST section per id). Duplicate section ids
///    persist their later copies as suffixed files (`details-2.md`, …) and
///    caller-facing sanitization treats the last-non-empty copy as authoritative,
///    so a FAIL verdict in a *later* duplicate must still fail closed; reading only
///    the first copy could hide it (#1675 review finding).
/// 2. The canonical review prose resolved by
///    [`crate::review_cmd::findings_toml::load_canonical_review_text`] — the SAME
///    loader the findings extractor uses (`full.md` → `output.log` → `details.md`
///    precedence). Reusing it keeps the fail-closed detector's source set identical
///    to the extractor's: a FAIL verdict that survives only in the raw `output.log`
///    (full.md absent, sections neutral, findings.toml synthetic-empty) must still
///    fail closed, and the two can never drift apart again (the #1675 review rounds
///    were repeatedly a source-set divergence between detector and extractor).
///
/// This is intentionally MORE thorough than [`review_contains_prose_clean_conclusion`]
/// (which reads only `summary` + `full.md`): a missed FAIL signal silently merges
/// blocking findings, so the fail-closed path errs toward catching FAIL wherever it
/// appears. Both asymmetries err toward FAIL.
pub(super) fn review_contains_prose_fail_conclusion(session_dir: &Path) -> Result<bool> {
    for (section, content) in csa_session::read_all_sections(session_dir)? {
        if matches!(section.id.as_str(), "summary" | "details")
            && detect_prose_fail_conclusion(&content)
        {
            return Ok(true);
        }
    }

    if let Some(review_text) =
        crate::review_cmd::findings_toml::load_canonical_review_text(session_dir)?
        && detect_prose_fail_conclusion(&review_text)
    {
        return Ok(true);
    }

    Ok(false)
}

pub(super) fn contains_clean_phrase(output: &str) -> bool {
    let lower = output.to_ascii_lowercase();
    [
        "no issues found",
        "no issues were found",
        "no blocking issues",
        "no findings",
        "none.",
        "\u{672a}\u{53d1}\u{73b0}\u{95ee}\u{9898}",
        "\u{6ca1}\u{6709}\u{53d1}\u{73b0}\u{95ee}\u{9898}",
        "\u{65e0}\u{963b}\u{585e}\u{95ee}\u{9898}",
    ]
    .iter()
    .any(|phrase| lower.contains(phrase))
        || contains_positive_no_issue_clause(&lower)
}

/// Check whether review output contains substantive content beyond prompt guards.
///
/// Returns `true` when the raw output is empty or contains only CSA prompt
/// injection markers / hook output and whitespace — indicating the review tool
/// produced no actual findings.
pub(in crate::review_cmd) fn is_review_output_empty(raw_output: &str) -> bool {
    strip_prompt_guards(raw_output).trim().is_empty()
}

/// Remove non-review content: prompt injection blocks, hook markers, and section wrappers.
pub(super) fn strip_prompt_guards(text: &str) -> String {
    let mut result = String::new();
    let mut in_guard = false;
    for line in text.lines() {
        if line.contains("<csa-caller-prompt-injection") {
            in_guard = true;
            continue;
        }
        if line.contains("</csa-caller-prompt-injection>") {
            in_guard = false;
            continue;
        }
        if in_guard {
            continue;
        }
        if line.trim_start().starts_with("[csa-hook]") {
            continue;
        }
        if line.trim_start().starts_with("[csa-heartbeat]") {
            continue;
        }
        // Strip CSA section markers (empty wrappers are not substantive content)
        if line.trim_start().starts_with("<!-- CSA:SECTION:") {
            continue;
        }
        result.push_str(line);
        result.push('\n');
    }
    result
}

fn contains_positive_no_issue_clause(lower: &str) -> bool {
    const NOUNS: &[&str] = &[
        "issue", "issues", "finding", "findings", "concern", "concerns",
    ];
    const TAIL_VERBS: &[&str] = &["found", "identified", "detected", "introduced"];
    const MAX_TOKENS_BEFORE_NOUN: usize = 6;
    const MAX_TOKENS_AFTER_NOUN: usize = 4;

    let tokens: Vec<&str> = lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .collect();

    for (index, token) in tokens.iter().enumerate() {
        if *token != "no" && *token != "without" {
            continue;
        }

        let noun_index = ((index + 1)..tokens.len()).find(|candidate| {
            candidate.saturating_sub(index + 1) <= MAX_TOKENS_BEFORE_NOUN
                && NOUNS.contains(&tokens[*candidate])
        });
        let Some(noun_index) = noun_index else {
            continue;
        };

        let verb_matches = ((noun_index + 1)..tokens.len()).any(|candidate| {
            candidate.saturating_sub(noun_index + 1) <= MAX_TOKENS_AFTER_NOUN
                && TAIL_VERBS.contains(&tokens[candidate])
        });
        if verb_matches || noun_index == tokens.len() - 1 {
            return true;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::{verdict_token_fail, verdict_token_pass_or_clean};

    #[test]
    fn fail_detection_case_insensitive_clean_detection_case_sensitive() {
        // Intentional asymmetry (both err toward FAIL): FAIL is fail-closed so it
        // matches case-insensitively (agreeing with the CLI's verdict parser);
        // PASS/CLEAN is fail-open so it stays case-sensitive (a prior codex
        // finding showed case-insensitive clean detection unblocked merges on
        // uncertain evidence). A shared case-insensitive helper (the #1675 round-3
        // regression) breaks this — lock both directions in one test.
        assert!(verdict_token_fail("Verdict: Fail"));
        assert!(verdict_token_fail("HAS_ISSUES"));
        assert!(!verdict_token_pass_or_clean("Verdict: Pass"));
        assert!(!verdict_token_pass_or_clean("Status: Clean"));
    }

    #[test]
    fn verdict_token_trailing_punctuation_matches() {
        // codex finding: the bare-line path trimmed only {ws,*,_,.}, so a verdict
        // token followed by a delimiter the consensus parser splits on — most
        // importantly `:` — was missed, reopening the #1675 lost-evidence path for
        // colon-terminated verdict lines. Internal `_` is preserved so `HAS_ISSUES`
        // still matches.
        assert!(verdict_token_fail("FAIL:"));
        assert!(verdict_token_fail("HAS_ISSUES:"));
        assert!(verdict_token_fail("REJECT;"));
        assert!(verdict_token_fail("(FAIL)"));
        assert!(verdict_token_pass_or_clean("PASS:"));
        assert!(verdict_token_pass_or_clean("CLEAN!"));
    }

    #[test]
    fn verdict_token_trailing_punctuation_keeps_precision() {
        // Trimming surrounding punctuation must NOT let a multi-word line reduce to
        // a bare token: exact equality after trimming stays the precision guard.
        assert!(!verdict_token_pass_or_clean("PASS: rate is 100%"));
        assert!(!verdict_token_fail("FAIL_SAFE:"));
        assert!(!verdict_token_pass_or_clean("CLEAN_UP:"));
    }

    #[test]
    fn verdict_token_non_ascii_neighbors_are_not_trimmed() {
        // The boundary-trim set is restricted to ASCII punctuation/whitespace, so a
        // non-ASCII word adjacent to a verdict token is NOT stripped away. Trimming
        // the consensus parser's full `!is_ascii_alphanumeric()` class would reduce
        // `PASS нет` ("PASS" + Russian "no") to a bare `PASS` and emit a FALSE clean
        // verdict (cloud-review HIGH finding). Cyrillic stands in for any non-ASCII
        // script (CJK, Greek, accented Latin); multilingual prose must stay a
        // multi-word line that never reduces to a bare token.
        assert!(!verdict_token_pass_or_clean("PASS нет"));
        assert!(!verdict_token_pass_or_clean("Оценка: PASS неверна"));
        assert!(!verdict_token_fail("нет FAIL"));
        // ASCII-only verdict lines stay matchable after the narrowed trim set.
        assert!(verdict_token_pass_or_clean("PASS"));
        assert!(verdict_token_fail("FAIL:"));
    }

    #[test]
    fn verdict_token_uppercase_pass_matches() {
        assert!(verdict_token_pass_or_clean("Verdict: PASS"));
        assert!(verdict_token_pass_or_clean("PASS"));
    }

    #[test]
    fn verdict_token_uppercase_clean_matches() {
        assert!(verdict_token_pass_or_clean("Verdict: CLEAN"));
        assert!(verdict_token_pass_or_clean("Status: CLEAN."));
        assert!(verdict_token_pass_or_clean("CLEAN"));
    }

    #[test]
    fn verdict_token_label_is_case_insensitive() {
        assert!(verdict_token_pass_or_clean("decision: PASS"));
        assert!(verdict_token_pass_or_clean("RESULT: CLEAN"));
        assert!(verdict_token_pass_or_clean("Review: PASS"));
    }

    #[test]
    fn verdict_token_standalone_pass_line_matches() {
        assert!(verdict_token_pass_or_clean("details\nPASS\nnotes"));
    }

    #[test]
    fn verdict_token_markdown_emphasized_pass_matches() {
        assert!(verdict_token_pass_or_clean("**PASS**"));
        assert!(verdict_token_pass_or_clean("__CLEAN__"));
        assert!(verdict_token_pass_or_clean("**PASS** — clean fix"));
        assert!(verdict_token_pass_or_clean("Verdict: **PASS**"));
    }

    #[test]
    fn verdict_token_labeled_underscore_emphasis_matches() {
        assert!(verdict_token_pass_or_clean("Verdict: __PASS__"));
        assert!(verdict_token_pass_or_clean("Status: __CLEAN__"));
        assert!(verdict_token_pass_or_clean("Verdict: __PASS__ - all good"));
    }

    #[test]
    fn verdict_token_labeled_clean_with_punctuation_matches() {
        assert!(verdict_token_pass_or_clean("Status: CLEAN."));
        assert!(verdict_token_pass_or_clean("PASS."));
    }

    #[test]
    fn verdict_token_lowercase_negative_prose_does_not_match() {
        // codex finding: prior eq_ignore_ascii_case impl misclassified these
        // as clean conclusions and unblocked merges on uncertain evidence.
        assert!(!verdict_token_pass_or_clean("cannot pass yet"));
        assert!(!verdict_token_pass_or_clean(
            "review incomplete, cannot pass yet"
        ));
        assert!(!verdict_token_pass_or_clean("result is not clean"));
        assert!(!verdict_token_pass_or_clean(
            "I'll pass on judging this until tests run"
        ));
    }

    #[test]
    fn verdict_token_mixed_case_does_not_match() {
        assert!(!verdict_token_pass_or_clean("Verdict: Pass"));
        assert!(!verdict_token_pass_or_clean("Status: Clean"));
    }

    #[test]
    fn verdict_token_hyphenated_bypass_does_not_match() {
        assert!(!verdict_token_pass_or_clean("BY-PASS"));
    }

    #[test]
    fn verdict_token_hyphenated_pass_fail_does_not_match() {
        assert!(!verdict_token_pass_or_clean("PASS-FAIL criteria"));
    }

    #[test]
    fn verdict_token_unlabeled_pass_sentence_does_not_match() {
        assert!(!verdict_token_pass_or_clean("The review is PASS."));
        assert!(!verdict_token_pass_or_clean("The test PASS rate is 100%"));
    }

    #[test]
    fn verdict_token_unlabeled_clean_imperative_does_not_match() {
        assert!(!verdict_token_pass_or_clean(
            "Please CLEAN the build directory"
        ));
    }

    #[test]
    fn verdict_token_labeled_compound_does_not_match() {
        assert!(!verdict_token_pass_or_clean("Verdict: PASS-FAIL"));
        assert!(!verdict_token_pass_or_clean("Status: CLEAN_UP"));
    }

    #[test]
    fn verdict_token_labeled_separator_hyphen_matches() {
        assert!(verdict_token_pass_or_clean(
            "Verdict: PASS - all tests passed"
        ));
    }

    #[test]
    fn verdict_token_labeled_separator_slash_matches() {
        assert!(verdict_token_pass_or_clean(
            "Verdict: PASS/ something - separator-not-compound"
        ));
    }

    #[test]
    fn verdict_token_labeled_slash_compound_does_not_match() {
        // gemini round-2 finding: PASS/FAIL list-of-criteria phrasing must not
        // be treated as a verdict declaration.
        assert!(!verdict_token_pass_or_clean("Verdict: PASS/FAIL"));
        assert!(!verdict_token_pass_or_clean("Status: CLEAN/DIRTY"));
    }
}
