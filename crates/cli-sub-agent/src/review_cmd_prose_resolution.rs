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
    {
        return false;
    }
    tokens
        .iter()
        .enumerate()
        .any(|(index, _)| resolution_token_applies(&tokens, index))
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
