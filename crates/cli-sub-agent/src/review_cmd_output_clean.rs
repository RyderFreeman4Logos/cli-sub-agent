use std::{fs, path::Path};

use anyhow::Result;

use super::text::extract_review_text;

#[path = "review_cmd_output_verdict_tokens.rs"]
mod verdict_tokens;

pub(super) fn detect_prose_clean_conclusion(text: &str) -> bool {
    let current_prose = current_reviewer_prose_without_quoted_repro(text);
    let lower = current_prose.to_ascii_lowercase();
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
        || verdict_tokens::verdict_token_pass_or_clean(&current_prose)
}

fn current_reviewer_prose_without_quoted_repro(text: &str) -> String {
    let mut filtered = String::new();
    let mut in_fenced_block = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if line_is_markdown_fence(trimmed) {
            in_fenced_block = !in_fenced_block;
            continue;
        }
        if in_fenced_block || line_is_quoted_repro(line, trimmed) {
            continue;
        }
        let line = line_without_inline_code_spans(trimmed);
        filtered.push_str(line.as_ref());
        filtered.push('\n');
    }
    filtered
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

pub(crate) fn detect_bounded_clean_verdict_token(text: &str) -> bool {
    verdict_tokens::verdict_token_pass_or_clean(text)
}

pub(super) fn review_contains_prose_clean_conclusion(session_dir: &Path) -> Result<bool> {
    if let Some(current_sections) = current_round_review_sections(session_dir)? {
        for (_, content) in current_sections {
            if detect_prose_clean_conclusion(&content) {
                return Ok(true);
            }
        }
        return Ok(false);
    }

    if let Some(review_text) =
        crate::review_cmd::findings_toml::load_canonical_review_text(session_dir)?
        && detect_prose_clean_conclusion(&review_text)
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

pub(super) fn current_round_review_sections(
    session_dir: &Path,
) -> Result<Option<Vec<(String, String)>>> {
    let mut sections = Vec::new();
    for (section, content) in csa_session::read_all_sections(session_dir)? {
        if matches!(section.id.as_str(), "summary" | "details") {
            sections.push((section.id, content));
        }
    }

    let Some(last_index) = sections.len().checked_sub(1) else {
        return Ok(None);
    };
    let start_index = if sections[last_index].0 == "details"
        && last_index > 0
        && sections[last_index - 1].0 == "summary"
    {
        last_index - 1
    } else {
        last_index
    };

    Ok(Some(sections.split_off(start_index)))
}

/// Detect whether review prose AFFIRMATIVELY concludes FAIL via a bounded verdict
/// token (`FAIL`/`HAS_ISSUES`/`REJECT`). Unlike [`detect_prose_clean_conclusion`],
/// this matches ONLY verdict tokens (bare/labeled/emphasized) — never the substring
/// "fail" — so benign prose like "the test no longer fails" is not misread as a FAIL
/// verdict (#1675). Used to fail-closed when a real prose FAIL lost its structured
/// findings.
pub(super) fn detect_prose_fail_conclusion(text: &str) -> bool {
    verdict_tokens::verdict_token_fail(text)
}

pub(super) fn detect_prose_uncertain_conclusion(text: &str) -> bool {
    let current_prose = current_reviewer_prose_without_quoted_repro(text);
    verdict_tokens::verdict_token_uncertain(&current_prose)
        || prose_has_current_uncertain_conclusion(&current_prose)
}

fn prose_has_current_uncertain_conclusion(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    [
        "cannot conclude pass",
        "cannot conclude clean",
        "can't conclude pass",
        "can't conclude clean",
        "did not have enough context to conclude pass",
        "does not have enough context to conclude pass",
        "insufficient context to conclude pass",
        "insufficient context to call this pass",
    ]
    .iter()
    .any(|phrase| lower.contains(phrase))
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
