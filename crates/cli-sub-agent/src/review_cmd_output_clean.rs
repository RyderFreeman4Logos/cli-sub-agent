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
