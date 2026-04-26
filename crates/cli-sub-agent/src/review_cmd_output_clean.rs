use std::{fs, path::Path};

use anyhow::Result;

use super::extract_review_text;

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

fn verdict_token_pass_or_clean(text: &str) -> bool {
    text.lines().any(|line| {
        let trimmed = line.trim();
        is_verdict_token(trimmed)
            || has_emphasized_verdict_token_prefix(trimmed)
            || line_has_labeled_verdict_token(trimmed)
    })
}

fn is_verdict_token(text: &str) -> bool {
    let trimmed =
        text.trim_matches(|c: char| c.is_whitespace() || c == '*' || c == '_' || c == '.');
    matches!(trimmed, "PASS" | "CLEAN")
}

fn line_has_labeled_verdict_token(line: &str) -> bool {
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
            if is_verdict_token(rest)
                || has_emphasized_verdict_token_prefix(rest.trim_start())
                || has_verdict_token_prefix(rest.trim_start())
                || has_verdict_token_prefix(
                    rest.trim_start_matches(|c: char| c.is_whitespace() || c == '*' || c == '_'),
                )
            {
                return true;
            }
        }
    }

    false
}

fn has_verdict_token_prefix(text: &str) -> bool {
    ["PASS", "CLEAN"].iter().any(|token| {
        let Some(rest) = text.strip_prefix(token) else {
            return false;
        };
        verdict_token_is_bounded(rest)
    })
}

fn has_emphasized_verdict_token_prefix(text: &str) -> bool {
    ["**", "__"].iter().any(|marker| {
        ["PASS", "CLEAN"].iter().any(|token| {
            text.strip_prefix(marker)
                .and_then(|rest| rest.strip_prefix(token))
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

pub(super) fn contains_clean_phrase(output: &str) -> bool {
    let lower = output.to_ascii_lowercase();
    [
        "no issues found",
        "no issues were found",
        "no blocking issues",
        "no findings",
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
    use super::verdict_token_pass_or_clean;

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
