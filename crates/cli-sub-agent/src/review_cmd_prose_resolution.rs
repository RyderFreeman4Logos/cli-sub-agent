// Keep this to resolved-state predicates; verification verbs are ambiguous in
// active finding titles unless the surrounding prose states the issue is fixed.
const RESOLUTION_STATE_WORDS: &[&str] = &[
    "addressed",
    "cleared",
    "closed",
    "corrected",
    "eliminated",
    "fixed",
    "gone",
    "mitigated",
    "removed",
    "repaired",
    "resolved",
];
const RESOLUTION_AUXILIARY_WORDS: &[&str] = &[
    "am", "are", "be", "been", "being", "had", "has", "have", "is", "was", "were",
];
const REQUIRED_FIX_WORDS: &[&str] = &[
    "must", "need", "needed", "needs", "require", "required", "requires", "should",
];
const ACTIVE_PROBLEM_WORDS: &[&str] = &[
    "allow",
    "allows",
    "can",
    "cannot",
    "could",
    "fail",
    "failed",
    "failure",
    "incorrect",
    "leak",
    "missing",
    "omit",
    "omits",
    "omitted",
    "panic",
    "race",
    "regression",
    "remain",
    "remaining",
    "remains",
    "risk",
    "still",
    "unsafe",
    "wrong",
];
const CHINESE_RESOLUTION_PHRASES: &[&str] = &[
    "\u{5df2}\u{5b9e}\u{73b0}\u{5e76}\u{6d4b}\u{8bd5}",
    "\u{5df2}\u{6709}\u{76f4}\u{63a5}\u{6d4b}\u{8bd5}",
    "\u{5df2}\u{6709}\u{673a}\u{68b0}\u{6d4b}\u{8bd5}",
    "\u{5df2}\u{8986}\u{76d6}",
];
const CHINESE_ACTIVE_PROBLEM_PHRASES: &[&str] = &[
    "\u{672a}\u{5b9e}\u{73b0}",
    "\u{672a}\u{8986}\u{76d6}",
    "\u{672a}\u{6d4b}\u{8bd5}",
    "\u{672a}\u{7f13}\u{89e3}",
    "\u{5c1a}\u{672a}",
    "\u{4ecd}\u{672a}",
    "\u{4ecd}\u{5b58}\u{5728}",
    "\u{7f3a}\u{5c11}",
    "\u{9057}\u{6f0f}",
    "\u{5931}\u{8d25}",
    "\u{9519}\u{8bef}",
    "\u{98ce}\u{9669}",
    "\u{95ee}\u{9898}\u{4ecd}",
    "\u{8986}\u{76d6}\u{4e0d}\u{8db3}",
];

pub(super) fn review_signal_describes_resolved_issue(
    tokens: &[String],
    signal_index: usize,
    noun_index: usize,
) -> bool {
    let start = signal_index.saturating_sub(3);
    let end = tokens.len().min(noun_index + 8);
    // #2516: If the window contains a non-negated active-problem word,
    // the signal describes a live problem, not a resolution.
    // "no blocking findings remain" is fine because "remain" is negated by "no".
    if (start..end).any(|index| active_problem_token_is_non_negated(tokens, index)) {
        return false;
    }
    (start..end).any(|index| resolution_token_applies(tokens, index))
}

fn active_problem_token_is_non_negated(tokens: &[String], index: usize) -> bool {
    let Some(token) = tokens.get(index) else {
        return false;
    };
    if !ACTIVE_PROBLEM_WORDS.contains(&token.as_str()) {
        return false;
    }
    // Check preceding 3 tokens for negation words
    !tokens[..index]
        .iter()
        .rev()
        .take(3)
        .any(|candidate| matches!(candidate.as_str(), "no" | "not" | "never" | "without"))
}

pub(super) fn finding_text_describes_resolved_issue(text: &str) -> bool {
    let tokens = review_signal_tokens(text);
    if tokens
        .iter()
        .any(|token| ACTIVE_PROBLEM_WORDS.contains(&token.as_str()))
        || chinese_active_problem_phrase_applies(text)
    {
        return false;
    }
    if chinese_resolution_phrase_applies(text) {
        return true;
    }
    tokens
        .iter()
        .enumerate()
        .any(|(index, _)| resolution_token_applies(&tokens, index))
}

fn chinese_resolution_phrase_applies(text: &str) -> bool {
    CHINESE_RESOLUTION_PHRASES
        .iter()
        .any(|phrase| text.contains(phrase))
        || ordered_substrings_apply(text, "\u{5df2}\u{901a}\u{8fc7}", "\u{7f13}\u{89e3}")
}

fn chinese_active_problem_phrase_applies(text: &str) -> bool {
    CHINESE_ACTIVE_PROBLEM_PHRASES
        .iter()
        .any(|phrase| text.contains(phrase))
}

fn ordered_substrings_apply(text: &str, first: &str, second: &str) -> bool {
    text.find(first)
        .and_then(|start| text[start + first.len()..].find(second))
        .is_some()
}

fn resolution_token_applies(tokens: &[String], index: usize) -> bool {
    let Some(token) = tokens.get(index) else {
        return false;
    };
    RESOLUTION_STATE_WORDS.contains(&token.as_str())
        && !resolution_token_is_negated_or_required(tokens, index)
        && resolution_token_has_explicit_state_context(tokens, index)
}

fn resolution_token_has_explicit_state_context(tokens: &[String], index: usize) -> bool {
    resolution_token_has_auxiliary(tokens, index)
        || token_before(tokens, index).is_some_and(|token| token == "as")
}

fn resolution_token_has_auxiliary(tokens: &[String], index: usize) -> bool {
    tokens[..index]
        .iter()
        .rev()
        .take(3)
        .any(|candidate| RESOLUTION_AUXILIARY_WORDS.contains(&candidate.as_str()))
}

fn token_before(tokens: &[String], index: usize) -> Option<&str> {
    index
        .checked_sub(1)
        .and_then(|previous| tokens.get(previous))
        .map(String::as_str)
}

fn resolution_token_is_negated_or_required(tokens: &[String], index: usize) -> bool {
    tokens[..index].iter().rev().take(3).any(|candidate| {
        matches!(candidate.as_str(), "not" | "never" | "unresolved")
            || REQUIRED_FIX_WORDS.contains(&candidate.as_str())
    })
}

fn review_signal_tokens(text: &str) -> Vec<String> {
    text.split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}
