use csa_core::types::ReviewDecision;

use super::{
    is_legacy_decision_prefix_char, is_review_decision_token_char,
    pass_like_decision_has_compound_word_suffix, review_decision_token,
};

pub(super) fn unlabeled_clean_decision_with_clean_explanation(line: &str) -> bool {
    let value = line.trim_start_matches(is_legacy_decision_prefix_char);
    let token_end = value
        .char_indices()
        .find_map(|(index, ch)| (!is_review_decision_token_char(ch)).then_some(index))
        .unwrap_or(value.len());
    let Some(token) = value.get(..token_end).filter(|token| !token.is_empty()) else {
        return false;
    };
    let rest = value.get(token_end..).unwrap_or_default();
    if !matches!(review_decision_token(token), Some(ReviewDecision::Pass))
        || pass_like_decision_has_compound_word_suffix(token, rest)
    {
        return false;
    }
    let rest = rest.trim_start();
    !rest.is_empty() && clean_explanation_has_no_findings(rest)
}

fn clean_explanation_has_no_findings(rest: &str) -> bool {
    let lower = rest.to_ascii_lowercase();
    [
        "no blocking",
        "no issues found",
        "no issues were found",
        "no actionable findings",
        "no findings",
        "no blockers",
    ]
    .iter()
    .any(|phrase| lower.contains(phrase))
        || contains_positive_no_issue_clause(&lower)
}

fn contains_positive_no_issue_clause(lower: &str) -> bool {
    let tokens: Vec<&str> = lower
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .collect();
    tokens.iter().enumerate().any(|(no_index, token)| {
        *token == "no"
            && tokens
                .get(no_index + 1..(no_index + 7).min(tokens.len()))
                .and_then(|window| {
                    window
                        .iter()
                        .position(|candidate| {
                            matches!(
                                *candidate,
                                "issue"
                                    | "issues"
                                    | "finding"
                                    | "findings"
                                    | "concern"
                                    | "concerns"
                            )
                        })
                        .map(|relative| no_index + 1 + relative)
                })
                .is_some_and(|noun_index| {
                    tokens[noun_index + 1..(noun_index + 5).min(tokens.len())]
                        .iter()
                        .any(|candidate| {
                            matches!(
                                *candidate,
                                "found" | "identified" | "detected" | "introduced"
                            )
                        })
                })
    })
}
