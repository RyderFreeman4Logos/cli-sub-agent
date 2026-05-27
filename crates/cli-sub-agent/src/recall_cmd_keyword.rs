//! Cross-session keyword search for `csa xurl recall --keyword`.
//!
//! Supports three modes selected by the caller:
//! 1. Cross-session AND search: split the keyword string on whitespace and
//!    list threads whose transcript contains EVERY term.
//! 2. In-session AND search: when a session selector is provided, restrict
//!    the search to that session's transcript and render line context.
//! 3. Single-keyword search: a one-term `--keyword` degrades to the original
//!    single-keyword behavior (no extra rendering cost).

use std::path::Path;

use anyhow::Result;
use tracing::debug;

use super::{
    RECALL_PROVIDERS, SessionRef, provider_roots, render_session_markdown, resolve_session_ref,
    thread_belongs_to_project, truncate_display,
};

/// Width of the context snippet shown in the PREVIEW column (chars).
const PREVIEW_WIDTH: usize = 80;
/// Context-line radius used for in-session keyword display.
const SEARCH_CONTEXT_LINES: usize = 2;
/// Over-fetch factor when filtering by multiple keywords.
///
/// `xurl_core::ThreadQuery::limit` caps results per provider after the
/// primary keyword matches. Some candidates will be excluded by the AND
/// filter, so we widen the initial fetch to keep enough survivors.
const AND_OVERFETCH_FACTOR: usize = 4;

struct Hit {
    provider: xurl_core::ProviderKind,
    thread_id: String,
    thread_source: String,
    updated_at: Option<String>,
    preview: Option<String>,
}

pub(super) fn handle_recall_keyword(
    keyword: &str,
    session: Option<&str>,
    all: bool,
    limit: usize,
) -> Result<()> {
    let trimmed = keyword.trim();
    if trimmed.is_empty() {
        anyhow::bail!("--keyword must not be empty");
    }
    if limit == 0 {
        anyhow::bail!("--limit must be greater than 0");
    }
    let keywords: Vec<&str> = trimmed.split_whitespace().collect();
    if keywords.is_empty() {
        anyhow::bail!("--keyword must contain at least one non-whitespace term");
    }

    if let Some(sel) = session {
        return handle_in_session_search(sel, &keywords);
    }

    let project_root = crate::pipeline::determine_project_root(None)?;
    let roots = provider_roots()?;
    let mut hits = collect_hits(&keywords, all, limit, &project_root, &roots);

    if hits.is_empty() {
        let scope = if all {
            "any project".to_string()
        } else {
            format!("project {}", project_root.display())
        };
        let kw_display = keywords.join(" ");
        println!("No matches for keyword '{kw_display}' in {scope}.");
        return Ok(());
    }

    hits.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    print_hits(&hits);
    Ok(())
}

fn collect_hits(
    keywords: &[&str],
    all: bool,
    limit: usize,
    project_root: &Path,
    roots: &xurl_core::ProviderRoots,
) -> Vec<Hit> {
    // `keywords` is non-empty (validated by caller); safe to split.
    let primary = keywords[0];
    let extras = &keywords[1..];
    let provider_limit = if extras.is_empty() {
        limit
    } else {
        limit.saturating_mul(AND_OVERFETCH_FACTOR).max(limit)
    };

    let mut hits: Vec<Hit> = Vec::new();
    for &provider in RECALL_PROVIDERS {
        let query = xurl_core::ThreadQuery {
            uri: format!("{provider}://"),
            provider,
            role: None,
            q: Some(primary.to_string()),
            limit: provider_limit,
            ignored_params: Vec::new(),
        };
        let result = match xurl_core::query_threads(&query, roots) {
            Ok(r) => r,
            Err(err) => {
                debug!(
                    provider = %provider,
                    error = %err,
                    "recall keyword: skipping provider"
                );
                continue;
            }
        };
        for item in result.items {
            if !all && !thread_belongs_to_project(&item.thread_source, project_root, provider) {
                continue;
            }
            // Render the session once when we need (a) AND-filter validation or
            // (b) a clean snippet because the upstream preview truncated away
            // the primary keyword.
            let need_render_for_and = !extras.is_empty();
            let upstream_preview = item.matched_preview.as_deref();
            let upstream_has_keyword = upstream_preview
                .map(|p| contains_ci(p, primary))
                .unwrap_or(false);
            let need_render_for_snippet = !upstream_has_keyword;
            let rendered = if need_render_for_and || need_render_for_snippet {
                let session_ref = SessionRef {
                    sid: item.thread_id.clone(),
                    provider: provider.to_string(),
                };
                match render_session_markdown(&session_ref) {
                    Ok(c) => Some(c),
                    Err(err) => {
                        debug!(
                            provider = %provider,
                            sid = %item.thread_id,
                            error = %err,
                            "recall keyword: skip (render failed)"
                        );
                        if need_render_for_and {
                            continue;
                        }
                        None
                    }
                }
            } else {
                None
            };
            if need_render_for_and {
                // Safe: we only reach here when render succeeded (errors `continue`).
                let content = rendered.as_deref().unwrap_or("");
                if !all_keywords_present(content, extras) {
                    continue;
                }
            }
            let preview = build_preview(rendered.as_deref(), upstream_preview, primary);
            hits.push(Hit {
                provider,
                thread_id: item.thread_id,
                thread_source: item.thread_source,
                updated_at: item.updated_at,
                preview,
            });
        }
    }
    hits
}

/// Build a PREVIEW column entry.
///
/// Prefers a snippet extracted from the rendered transcript (always shows the
/// keyword in context). Falls back to the upstream `matched_preview` snippet
/// (truncated to the matching line, may omit the keyword for very long lines)
/// when the rendered content is unavailable.
fn build_preview(
    rendered: Option<&str>,
    upstream_preview: Option<&str>,
    primary_keyword: &str,
) -> Option<String> {
    if let Some(content) = rendered
        && let Some(snippet) = snippet_from_content(content, primary_keyword, PREVIEW_WIDTH)
    {
        return Some(snippet);
    }
    upstream_preview.and_then(|p| build_snippet(p, primary_keyword, PREVIEW_WIDTH))
}

/// Find the first non-empty line containing `keyword` (case-insensitive) and
/// build a centered snippet of `window` chars from it.
fn snippet_from_content(content: &str, keyword: &str, window: usize) -> Option<String> {
    let kw_lower = keyword.to_lowercase();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.to_lowercase().contains(&kw_lower) {
            return build_snippet(trimmed, keyword, window);
        }
    }
    None
}

fn contains_ci(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

fn print_hits(hits: &[Hit]) {
    println!(
        "{:<10} {:<36} {:<20} PREVIEW",
        "PROVIDER", "SESSION", "UPDATED"
    );
    println!("{}", "-".repeat(160));
    for hit in hits {
        let updated = hit.updated_at.as_deref().unwrap_or("-");
        let updated_short: String = updated.chars().take(19).collect();
        let preview = hit.preview.as_deref().unwrap_or("");
        let preview_display = truncate_chars(preview, PREVIEW_WIDTH);
        println!(
            "{:<10} {:<36} {:<20} {}",
            hit.provider.to_string(),
            truncate_display(&hit.thread_id, 36),
            updated_short,
            preview_display,
        );
        debug!(source = %hit.thread_source, "recall keyword hit");
    }
    println!("\nTotal matches: {}", hits.len());
}

fn handle_in_session_search(session: &str, keywords: &[&str]) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(None)?;
    let session_ref = resolve_session_ref(session, &project_root)?;
    let content = render_session_markdown(&session_ref)?;
    let lines: Vec<&str> = content.lines().collect();
    let ranges = matching_ranges_any(&lines, keywords, SEARCH_CONTEXT_LINES);

    if ranges.is_empty() {
        let kw_display = keywords.join(" ");
        println!(
            "No matches for '{}' in session {}.",
            kw_display, session_ref.sid
        );
        return Ok(());
    }

    println!(
        "Matches in session {} ({})",
        session_ref.sid, session_ref.provider
    );
    for (start, end) in ranges {
        println!("\n-- lines {}-{} --", start + 1, end + 1);
        for (idx, line) in lines.iter().enumerate().take(end + 1).skip(start) {
            let hit = keywords.iter().any(|kw| line_contains_ci(line, kw));
            let marker = if hit { ">" } else { " " };
            println!("{marker} {:>5} {line}", idx + 1);
        }
    }
    Ok(())
}

fn matching_ranges_any(lines: &[&str], keywords: &[&str], context: usize) -> Vec<(usize, usize)> {
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        if !keywords.iter().any(|kw| line_contains_ci(line, kw)) {
            continue;
        }
        let start = idx.saturating_sub(context);
        let end = (idx + context).min(lines.len().saturating_sub(1));
        if let Some((_, prev_end)) = ranges.last_mut()
            && start <= *prev_end + 1
        {
            *prev_end = (*prev_end).max(end);
            continue;
        }
        ranges.push((start, end));
    }
    ranges
}

fn all_keywords_present(content: &str, keywords: &[&str]) -> bool {
    let lower = content.to_lowercase();
    keywords.iter().all(|kw| lower.contains(&kw.to_lowercase()))
}

fn line_contains_ci(line: &str, keyword: &str) -> bool {
    contains_ci(line, keyword)
}

/// Build a fixed-width context snippet around the first occurrence of `keyword`.
///
/// Returns up to `window` characters of context centered on the keyword, with
/// the matched span wrapped in square brackets. Adds leading/trailing `…`
/// when the snippet is shorter than the source. When the keyword is absent
/// (e.g. matched on a portion outside the upstream-truncated preview),
/// falls back to head truncation of `source`.
fn build_snippet(source: &str, keyword: &str, window: usize) -> Option<String> {
    if source.is_empty() || keyword.is_empty() || window == 0 {
        return None;
    }
    let source_lower = source.to_lowercase();
    let kw_lower = keyword.to_lowercase();

    let Some(kw_byte) = source_lower.find(&kw_lower) else {
        return Some(truncate_chars(source, window));
    };

    let kw_char_start = source_lower[..kw_byte].chars().count();
    let kw_char_len = kw_lower.chars().count();
    let total_chars = source.chars().count();
    let context = window.saturating_sub(kw_char_len);
    let left_ctx = context / 2;
    let right_ctx = context - left_ctx;

    let start = kw_char_start.saturating_sub(left_ctx);
    let end = (kw_char_start + kw_char_len + right_ctx).min(total_chars);

    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    for (i, ch) in source.chars().enumerate() {
        if i < start {
            continue;
        }
        if i >= end {
            break;
        }
        if i == kw_char_start {
            out.push('[');
        }
        out.push(ch);
        if i + 1 == kw_char_start + kw_char_len {
            out.push(']');
        }
    }
    if end < total_chars {
        out.push('…');
    }
    Some(out)
}

fn truncate_chars(input: &str, max_chars: usize) -> String {
    let total = input.chars().count();
    if total <= max_chars {
        return input.to_string();
    }
    let mut out: String = input.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_snippet_brackets_keyword_in_short_source() {
        let source = "the quick brown fox jumps over the lazy dog";
        let snippet = build_snippet(source, "fox", 80).expect("snippet");
        assert!(snippet.contains("[fox]"), "snippet={snippet}");
        // Source fits in window; no ellipsis padding required.
        assert!(!snippet.starts_with('…'));
        assert!(!snippet.ends_with('…'));
    }

    #[test]
    fn build_snippet_preserves_original_case_when_matching_case_insensitively() {
        let source = "Module-As-Software concept anchor";
        let snippet = build_snippet(source, "module-as-software", 80).expect("snippet");
        assert!(
            snippet.contains("[Module-As-Software]"),
            "snippet={snippet}"
        );
    }

    #[test]
    fn build_snippet_centers_window_for_long_source() {
        let prefix = "a".repeat(200);
        let suffix = "b".repeat(200);
        let source = format!("{prefix}match{suffix}");
        let snippet = build_snippet(&source, "match", 40).expect("snippet");
        assert!(
            snippet.starts_with('…'),
            "leading ellipsis missing: snippet={snippet}"
        );
        assert!(
            snippet.ends_with('…'),
            "trailing ellipsis missing: snippet={snippet}"
        );
        assert!(snippet.contains("[match]"), "snippet={snippet}");
    }

    #[test]
    fn build_snippet_falls_back_when_keyword_absent_from_source() {
        let source = "no match here";
        let snippet = build_snippet(source, "absent", 40).expect("snippet");
        assert!(!snippet.contains('['), "snippet={snippet}");
        assert!(!snippet.contains(']'), "snippet={snippet}");
    }

    #[test]
    fn build_snippet_returns_none_for_empty_inputs() {
        assert!(build_snippet("", "x", 10).is_none());
        assert!(build_snippet("source", "", 10).is_none());
        assert!(build_snippet("source", "x", 0).is_none());
    }

    #[test]
    fn build_snippet_handles_multibyte_chars() {
        // Multi-byte (Greek) keyword surrounded by enough context to require centering.
        let source = "abcdefghijαβkeyword-γδεklmnopqrst";
        let snippet = build_snippet(source, "αβ", 20).expect("snippet");
        assert!(snippet.contains("[αβ]"), "snippet={snippet}");
    }

    #[test]
    fn all_keywords_present_returns_true_when_every_term_appears() {
        let content = "alpha beta gamma";
        assert!(all_keywords_present(content, &["alpha", "beta"]));
        assert!(all_keywords_present(content, &["ALPHA", "GAMMA"]));
    }

    #[test]
    fn all_keywords_present_returns_false_when_any_term_missing() {
        let content = "alpha beta gamma";
        assert!(!all_keywords_present(content, &["alpha", "delta"]));
    }

    #[test]
    fn all_keywords_present_allows_empty_keyword_slice() {
        assert!(all_keywords_present("anything", &[]));
    }

    #[test]
    fn matching_ranges_any_marks_lines_that_contain_any_keyword() {
        let lines = vec!["line 0", "alpha line 1", "line 2", "beta line 3", "line 4"];
        let ranges = matching_ranges_any(&lines, &["alpha", "beta"], 1);
        // alpha matches line 1, beta matches line 3 — adjacent context ranges merge.
        assert_eq!(ranges, vec![(0, 4)]);
    }

    #[test]
    fn matching_ranges_any_returns_empty_when_no_keyword_matches() {
        let lines = vec!["line 0", "line 1"];
        let ranges = matching_ranges_any(&lines, &["absent"], 1);
        assert!(ranges.is_empty());
    }

    #[test]
    fn matching_ranges_any_case_insensitive() {
        let lines = vec!["nothing", "Module-As-Software here"];
        let ranges = matching_ranges_any(&lines, &["module-as-software"], 0);
        assert_eq!(ranges, vec![(1, 1)]);
    }

    #[test]
    fn truncate_chars_returns_input_when_short() {
        assert_eq!(truncate_chars("abc", 10), "abc");
    }

    #[test]
    fn truncate_chars_appends_ellipsis_when_truncating() {
        let out = truncate_chars("abcdefghij", 5);
        let chars: Vec<char> = out.chars().collect();
        assert_eq!(chars.len(), 5);
        assert_eq!(chars.last(), Some(&'…'));
    }

    #[test]
    fn snippet_from_content_finds_first_matching_line() {
        let content = "header line\n\nrandom prose\nthe Module-As-Software anchor pattern\nfooter";
        let snippet = snippet_from_content(content, "module-as-software", 80).expect("snippet");
        assert!(
            snippet.contains("[Module-As-Software]"),
            "snippet={snippet}"
        );
    }

    #[test]
    fn snippet_from_content_skips_blank_lines() {
        let content = "\n\n   \nfirst match: keyword here\n";
        let snippet = snippet_from_content(content, "keyword", 40).expect("snippet");
        assert!(snippet.contains("[keyword]"), "snippet={snippet}");
    }

    #[test]
    fn snippet_from_content_returns_none_when_keyword_missing() {
        let content = "nothing matches\nstill nothing\n";
        assert!(snippet_from_content(content, "absent", 40).is_none());
    }

    #[test]
    fn build_preview_prefers_rendered_snippet_over_upstream_truncation() {
        let rendered = "user message line\nthe target keyword is here\n";
        let upstream = "{\"role\":\"user\",\"content\":\"...truncated long json...\"}";
        let preview = build_preview(Some(rendered), Some(upstream), "keyword").expect("preview");
        assert!(preview.contains("[keyword]"), "preview={preview}");
    }

    #[test]
    fn build_preview_falls_back_to_upstream_when_no_rendered_content() {
        let upstream = "short line with keyword inside";
        let preview = build_preview(None, Some(upstream), "keyword").expect("preview");
        assert!(preview.contains("[keyword]"), "preview={preview}");
    }

    #[test]
    fn build_preview_returns_none_when_both_sources_lack_keyword() {
        // Both inputs lack the keyword. Rendered tries first (None), then upstream
        // returns head-truncated source without brackets.
        let preview = build_preview(Some("no match"), Some("no match either"), "absent");
        assert!(
            preview.is_some(),
            "fallback should produce truncated preview"
        );
        let preview = preview.unwrap();
        assert!(
            !preview.contains('['),
            "no keyword bracket: preview={preview}"
        );
    }

    #[test]
    fn contains_ci_matches_case_insensitively() {
        assert!(contains_ci("HELLO world", "hello"));
        assert!(contains_ci("foo Bar baz", "BAR"));
        assert!(!contains_ci("foo bar", "baz"));
    }
}
