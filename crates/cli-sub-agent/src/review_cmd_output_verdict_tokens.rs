/// Verdict tokens that affirmatively conclude a clean/passing review.
const PASS_VERDICT_TOKENS: &[&str] = &["PASS", "CLEAN"];

/// Verdict tokens that affirmatively conclude a failing review (#1675).
const FAIL_VERDICT_TOKENS: &[&str] = &["FAIL", "HAS_ISSUES", "REJECT"];

/// Verdict tokens that explicitly leave the review outcome unresolved.
const UNCERTAIN_VERDICT_TOKENS: &[&str] = &["UNCERTAIN"];

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

pub(super) fn verdict_token_pass_or_clean(text: &str) -> bool {
    verdict_token_matches(text, PASS_VERDICT_TOKENS, MatchCase::Sensitive)
}

/// Detect an affirmative FAIL verdict token (`FAIL`/`HAS_ISSUES`/`REJECT`),
/// mirroring [`verdict_token_pass_or_clean`]. Matches ONLY bounded verdict tokens
/// (bare line, `Verdict:`-labeled, or `**`/`__`-emphasized) — never the substring
/// "fail" — so prose like "the test no longer fails" is not read as a FAIL
/// conclusion (#1675 precision requirement). Unlike the PASS/CLEAN detector this
/// is case-INsensitive; see [`MatchCase`].
pub(super) fn verdict_token_fail(text: &str) -> bool {
    verdict_token_matches(text, FAIL_VERDICT_TOKENS, MatchCase::Insensitive)
}

/// Detect an affirmative unresolved verdict token (`UNCERTAIN`).
///
/// This intentionally uses the same bounded/labeled verdict-token machinery as
/// FAIL detection, plus a line-heading form (`UNCERTAIN: ...`) that reviewers use
/// as a verdict heading. It does NOT match arbitrary standalone words inside
/// prose, so clean reviews may mention a prior UNCERTAIN verdict bug without
/// triggering a false unresolved-current-verdict signal.
pub(super) fn verdict_token_uncertain(text: &str) -> bool {
    let mut in_fenced_block = false;
    text.lines().any(|line| {
        let trimmed = line.trim();
        if line_is_markdown_fence(trimmed) {
            in_fenced_block = !in_fenced_block;
            return false;
        }
        if in_fenced_block || line_is_quoted_repro(line, trimmed) {
            return false;
        }
        let scan_line = line_without_inline_code_spans(trimmed);
        let scan_line = scan_line.as_ref();

        is_verdict_token(scan_line, UNCERTAIN_VERDICT_TOKENS, MatchCase::Insensitive)
            || has_emphasized_verdict_token_prefix(
                scan_line,
                UNCERTAIN_VERDICT_TOKENS,
                MatchCase::Insensitive,
            )
            || line_has_labeled_verdict_token(
                scan_line,
                UNCERTAIN_VERDICT_TOKENS,
                MatchCase::Insensitive,
            )
            || line_has_verdict_token_heading(
                scan_line,
                UNCERTAIN_VERDICT_TOKENS,
                MatchCase::Insensitive,
            )
    })
}

fn line_is_markdown_fence(trimmed: &str) -> bool {
    trimmed.starts_with("```") || trimmed.starts_with("~~~")
}

fn line_is_quoted_repro(raw_line: &str, trimmed: &str) -> bool {
    raw_line.starts_with("    ")
        || raw_line.starts_with('\t')
        || trimmed.starts_with('>')
        || (trimmed.starts_with('`') && trimmed.ends_with('`') && trimmed.len() > 1)
        || (trimmed.starts_with("**`") && trimmed.ends_with("`**"))
        || (trimmed.starts_with("__`") && trimmed.ends_with("`__"))
}

fn line_without_inline_code_spans(line: &str) -> std::borrow::Cow<'_, str> {
    if !line.contains('`') {
        return std::borrow::Cow::Borrowed(line);
    }

    let bytes = line.as_bytes();
    let mut output = String::with_capacity(line.len());
    let mut cursor = 0;
    let mut index = 0;
    let mut removed = false;

    while index < bytes.len() {
        if bytes[index] != b'`' {
            index += 1;
            continue;
        }

        let marker_len = count_backtick_run(bytes, index);
        if let Some(close_index) = find_matching_backtick_run(bytes, index + marker_len, marker_len)
        {
            output.push_str(&line[cursor..index]);
            index = close_index + marker_len;
            cursor = index;
            removed = true;
        } else {
            break;
        }
    }

    if removed {
        output.push_str(&line[cursor..]);
        std::borrow::Cow::Owned(output)
    } else {
        std::borrow::Cow::Borrowed(line)
    }
}

fn count_backtick_run(bytes: &[u8], start: usize) -> usize {
    bytes[start..]
        .iter()
        .take_while(|byte| **byte == b'`')
        .count()
}

fn find_matching_backtick_run(bytes: &[u8], mut start: usize, marker_len: usize) -> Option<usize> {
    while start < bytes.len() {
        if bytes[start] == b'`' && count_backtick_run(bytes, start) == marker_len {
            return Some(start);
        }
        start += 1;
    }
    None
}

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

fn line_has_verdict_token_heading(line: &str, tokens: &[&str], case: MatchCase) -> bool {
    tokens.iter().any(|token| {
        strip_token_prefix(line, token, case).is_some_and(|rest| {
            rest.trim_start()
                .strip_prefix(':')
                .is_some_and(|after_colon| !after_colon.trim().is_empty())
        })
    })
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

#[cfg(test)]
mod tests {
    use super::{verdict_token_fail, verdict_token_pass_or_clean, verdict_token_uncertain};

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
        assert!(!verdict_token_fail(
            "FAIL  The PR has a prose sentence after the token"
        ));
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
    fn uncertain_verdict_token_is_bounded_not_any_prose_word() {
        assert!(verdict_token_uncertain("Verdict: UNCERTAIN"));
        assert!(verdict_token_uncertain("Review: UNCERTAIN"));
        assert!(verdict_token_uncertain("UNCERTAIN"));
        assert!(verdict_token_uncertain("uncertain: insufficient context"));
        assert!(!verdict_token_uncertain(
            "The prior UNCERTAIN verdict bug is fixed; no blocking findings remain."
        ));
        assert!(!verdict_token_uncertain(
            "This sentence is uncertain about wording but reaches no verdict."
        ));
    }

    #[test]
    fn uncertain_verdict_token_ignores_quoted_repro_wait_output() {
        assert!(!verdict_token_uncertain(
            "Prior wait output:\n```text\nReview verdict: uncertain\nSummary: old run\n```\nNo blocking findings remain."
        ));
        assert!(!verdict_token_uncertain(
            "> Review verdict: uncertain\nNo blocking findings remain."
        ));
        assert!(!verdict_token_uncertain(
            "`Review verdict: uncertain`\nNo blocking findings remain."
        ));
        assert!(!verdict_token_uncertain(
            "    Review verdict: uncertain\nNo blocking findings remain."
        ));
        assert!(!verdict_token_uncertain(
            "Earlier `Review verdict: uncertain` output is the old run; no blocking findings remain."
        ));
        assert!(!verdict_token_uncertain(
            "Earlier ``Review verdict: uncertain`` output is the old run; no blocking findings remain."
        ));
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
