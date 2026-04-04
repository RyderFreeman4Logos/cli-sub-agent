use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context, Result};
use tracing::{debug, info, warn};

/// Default promotion threshold: a candidate must appear in this many reviews
/// before being auto-promoted to the main checklist.
const DEFAULT_PROMOTION_THRESHOLD: usize = 3;

/// Minimum keyword overlap ratio (0.0–1.0) for two findings to be considered
/// duplicates.  60% means at least 60% of keywords in the shorter text must
/// appear in the longer text.
const FUZZY_MATCH_THRESHOLD: f64 = 0.60;

// ── Extraction ──────────────────────────────────────────────────────────────

/// Extract one-line finding summaries from raw review output.
///
/// Recognised patterns (one per line):
///   - Numbered items:  `1. Finding text` / `1) Finding text`
///   - Bullet items:    `- Finding text` / `* Finding text`
///   - Bracketed IDs:   `[R01] Finding text` / `[HIGH] text`
///
/// Each extracted finding is trimmed to at most 120 characters.
pub(crate) fn extract_findings_from_result(result_text: &str) -> Vec<String> {
    let mut findings = Vec::new();

    for line in result_text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let extracted = try_extract_finding(trimmed);
        if let Some(text) = extracted {
            let text = text.trim();
            // Skip very short noise (headers, labels, etc.)
            if text.len() >= 10 {
                let truncated = truncate_finding(text, 120);
                findings.push(truncated);
            }
        }
    }

    findings.dedup();
    findings
}

/// Try to extract a finding body from a single trimmed line.
fn try_extract_finding(line: &str) -> Option<&str> {
    // Numbered: "1. text" or "1) text"
    if let Some(rest) = strip_numbered_prefix(line) {
        return Some(rest);
    }

    // Bullet: "- text" or "* text"
    if let Some(rest) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
        // Avoid markdown headers disguised as bullets
        if rest.starts_with('#') || rest.starts_with('[') && rest.contains("](") {
            return None;
        }
        return Some(rest);
    }

    // Bracketed ID: "[R01] text" / "[HIGH] text"
    if line.starts_with('[')
        && let Some(bracket_end) = line.find("] ")
    {
        let tag = &line[1..bracket_end];
        // Heuristic: tags are short uppercase/digit identifiers
        if tag.len() <= 10
            && tag
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        {
            return Some(&line[bracket_end + 2..]);
        }
    }

    None
}

fn strip_numbered_prefix(line: &str) -> Option<&str> {
    let mut chars = line.chars();
    // Must start with a digit
    let first = chars.next()?;
    if !first.is_ascii_digit() {
        return None;
    }
    // Consume remaining digits
    let rest = chars.as_str();
    let after_digits = rest.trim_start_matches(|c: char| c.is_ascii_digit());
    // Expect ". " or ") "
    let after_sep = after_digits
        .strip_prefix(". ")
        .or_else(|| after_digits.strip_prefix(") "))?;
    Some(after_sep)
}

fn truncate_finding(text: &str, max_len: usize) -> String {
    if text.len() <= max_len {
        text.to_string()
    } else {
        let mut end = max_len;
        // Try to cut at a word boundary
        if let Some(space_pos) = text[..max_len].rfind(' ') {
            end = space_pos;
        }
        format!("{}...", &text[..end])
    }
}

// ── Deduplication ───────────────────────────────────────────────────────────

/// Filter out findings that are already covered by the existing checklist.
///
/// Uses keyword-overlap fuzzy matching: if >= 60% of significant keywords in a
/// finding also appear in any single checklist item, the finding is considered
/// a duplicate and excluded.
pub(crate) fn dedupe_against_checklist(findings: &[String], checklist_path: &Path) -> Vec<String> {
    let checklist_items = match std::fs::read_to_string(checklist_path) {
        Ok(content) => parse_checklist_items(&content),
        Err(_) => {
            // No checklist file — nothing to dedupe against
            return findings.to_vec();
        }
    };

    if checklist_items.is_empty() {
        return findings.to_vec();
    }

    // Pre-compute keyword sets for checklist items
    let checklist_keyword_sets: Vec<HashSet<String>> =
        checklist_items.iter().map(|item| keywords(item)).collect();

    findings
        .iter()
        .filter(|finding| {
            let finding_kw = keywords(finding);
            !checklist_keyword_sets
                .iter()
                .any(|checklist_kw| keyword_overlap_exceeds(&finding_kw, checklist_kw))
        })
        .cloned()
        .collect()
}

fn parse_checklist_items(content: &str) -> Vec<String> {
    content
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            // Match "- [ ] text" or "- [x] text" or "- text"
            let text = trimmed
                .strip_prefix("- [ ] ")
                .or_else(|| trimmed.strip_prefix("- [x] "))
                .or_else(|| trimmed.strip_prefix("- [X] "))
                .or_else(|| trimmed.strip_prefix("- "));
            text.map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
        })
        .collect()
}

/// Extract significant keywords from text (lowercase, >= 3 chars, no stopwords).
fn keywords(text: &str) -> HashSet<String> {
    static STOPWORDS: &[&str] = &[
        "the", "and", "for", "are", "but", "not", "you", "all", "can", "has", "her", "was", "one",
        "our", "out", "use", "with", "that", "this", "from", "have", "been", "will", "each",
        "make", "when", "must", "should", "check", "verify",
    ];

    text.split(|c: char| !c.is_alphanumeric() && c != '_')
        .map(|w| w.to_lowercase())
        .filter(|w| w.len() >= 3 && !STOPWORDS.contains(&w.as_str()))
        .collect()
}

/// Returns true if keyword overlap ratio exceeds the threshold.
///
/// Ratio = |intersection| / |smaller set|
fn keyword_overlap_exceeds(set_a: &HashSet<String>, set_b: &HashSet<String>) -> bool {
    if set_a.is_empty() || set_b.is_empty() {
        return false;
    }
    let intersection = set_a.intersection(set_b).count();
    let smaller = set_a.len().min(set_b.len());
    (intersection as f64 / smaller as f64) >= FUZZY_MATCH_THRESHOLD
}

// ── Candidate Persistence ───────────────────────────────────────────────────

/// Parsed candidate entry from the candidates file.
struct CandidateEntry {
    count: usize,
    text: String,
}

/// Append new findings to the candidates file, incrementing counts for
/// fuzzy-matched existing candidates.
pub(crate) fn append_candidates(new_findings: &[String], candidates_path: &Path) -> Result<()> {
    if new_findings.is_empty() {
        return Ok(());
    }

    let mut candidates = load_candidates(candidates_path);

    for finding in new_findings {
        if let Some(existing) = candidates
            .iter_mut()
            .find(|c| keyword_overlap_exceeds(&keywords(&c.text), &keywords(finding)))
        {
            existing.count += 1;
            debug!(
                finding = %finding,
                existing = %existing.text,
                new_count = existing.count,
                "Incremented existing candidate count"
            );
        } else {
            candidates.push(CandidateEntry {
                count: 1,
                text: finding.clone(),
            });
            debug!(finding = %finding, "Added new candidate");
        }
    }

    write_candidates(&candidates, candidates_path)
}

/// Promote candidates that have reached the threshold count to the main
/// checklist.  Promoted candidates are removed from the candidates file.
///
/// Returns the list of promoted finding texts.
pub(crate) fn promote_candidates(
    candidates_path: &Path,
    checklist_path: &Path,
    threshold: Option<usize>,
) -> Result<Vec<String>> {
    let threshold = threshold.unwrap_or(DEFAULT_PROMOTION_THRESHOLD);
    let candidates = load_candidates(candidates_path);

    let (promoted, remaining): (Vec<_>, Vec<_>) =
        candidates.into_iter().partition(|c| c.count >= threshold);

    if promoted.is_empty() {
        return Ok(Vec::new());
    }

    let promoted_texts: Vec<String> = promoted.iter().map(|c| c.text.clone()).collect();

    // Append promoted items to checklist
    append_to_checklist(&promoted_texts, checklist_path)?;

    // Rewrite candidates without promoted items
    write_candidates(&remaining, candidates_path)?;

    for text in &promoted_texts {
        info!(finding = %text, "Promoted finding to review checklist");
    }

    Ok(promoted_texts)
}

fn load_candidates(path: &Path) -> Vec<CandidateEntry> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    content
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            // Format: "- [count:N] text"
            let rest = trimmed.strip_prefix("- [count:")?;
            let bracket_end = rest.find(']')?;
            let count: usize = rest[..bracket_end].parse().ok()?;
            let text = rest[bracket_end + 1..].trim().to_string();
            if text.is_empty() {
                return None;
            }
            Some(CandidateEntry { count, text })
        })
        .collect()
}

fn write_candidates(candidates: &[CandidateEntry], path: &Path) -> Result<()> {
    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory for {}", path.display()))?;
    }

    let mut content = String::from("# Review Findings Candidates\n");
    for entry in candidates {
        content.push_str(&format!("- [count:{}] {}\n", entry.count, entry.text));
    }

    std::fs::write(path, &content)
        .with_context(|| format!("Failed to write candidates file: {}", path.display()))
}

fn append_to_checklist(items: &[String], checklist_path: &Path) -> Result<()> {
    let mut content = match std::fs::read_to_string(checklist_path) {
        Ok(c) => c,
        Err(_) => {
            // Create new checklist if it doesn't exist
            String::from(
                "# Project Review Checklist\n\nCommon pitfalls and patterns to verify during code review:\n\n",
            )
        }
    };

    // Ensure trailing newline before appending
    if !content.ends_with('\n') {
        content.push('\n');
    }

    for item in items {
        content.push_str(&format!("- [ ] {item}\n"));
    }

    // Ensure parent directory exists
    if let Some(parent) = checklist_path.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!(
                "Failed to create directory for {}",
                checklist_path.display()
            )
        })?;
    }

    std::fs::write(checklist_path, &content)
        .with_context(|| format!("Failed to write checklist: {}", checklist_path.display()))
}

// ── Public Orchestrator ─────────────────────────────────────────────────────

/// Run the full findings accumulation pipeline after a Fail verdict.
///
/// 1. Extract findings from review output
/// 2. Deduplicate against existing checklist
/// 3. Append new candidates
/// 4. Promote candidates that exceed the threshold
///
/// This is a best-effort operation: failures are logged but do not propagate.
pub(crate) fn accumulate_findings(project_root: &Path, review_output: &str) {
    let csa_dir = project_root.join(".csa");
    let checklist_path = csa_dir.join("review-checklist.md");
    let candidates_path = csa_dir.join("review-findings-candidates.md");

    let findings = extract_findings_from_result(review_output);
    if findings.is_empty() {
        debug!("No findings extracted from review output");
        return;
    }
    debug!(
        count = findings.len(),
        "Extracted findings from review output"
    );

    let new_findings = dedupe_against_checklist(&findings, &checklist_path);
    if new_findings.is_empty() {
        debug!("All extracted findings already covered by checklist");
        return;
    }
    debug!(
        count = new_findings.len(),
        "New findings after deduplication"
    );

    if let Err(err) = append_candidates(&new_findings, &candidates_path) {
        warn!(error = %err, "Failed to append review findings candidates");
        return;
    }

    match promote_candidates(&candidates_path, &checklist_path, None) {
        Ok(promoted) if !promoted.is_empty() => {
            info!(
                count = promoted.len(),
                "Promoted {} finding(s) to review checklist",
                promoted.len()
            );
        }
        Err(err) => {
            warn!(error = %err, "Failed to promote review findings candidates");
        }
        _ => {}
    }
}

#[cfg(test)]
#[path = "review_findings_tests.rs"]
mod tests;
