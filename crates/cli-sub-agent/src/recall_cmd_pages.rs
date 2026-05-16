//! Compact-event pagination helpers for `csa recall read --page` and `csa recall pages`.

use std::io::{BufRead, BufReader};
use std::path::Path;

/// Heading suffix that xurl-core renders for compact timeline entries.
const COMPACT_HEADING_SUFFIX: &str = "Context Compacted";

/// Returns true if `line` is a rendered compact heading of the form `## N. Context Compacted`.
pub(super) fn is_compact_heading(line: &str) -> bool {
    let trimmed = line.trim_end();
    let Some(rest) = trimmed.strip_prefix("## ") else {
        return false;
    };
    let Some(dot_pos) = rest.find(". ") else {
        return false;
    };
    let (num, title) = rest.split_at(dot_pos);
    num.chars().all(|c| c.is_ascii_digit())
        && title.trim_start_matches(". ") == COMPACT_HEADING_SUFFIX
}

/// Splits rendered markdown into pages at each `## N. Context Compacted` heading.
///
/// Page 1 = everything before the first compact heading.
/// Page N+1 = from the Nth compact heading to (but not including) the next one.
/// Returns a single page containing the full content when no compact headings exist.
pub(super) fn split_markdown_pages(content: &str) -> Vec<String> {
    let mut pages: Vec<String> = Vec::new();
    let mut current_start = 0usize;

    for (byte_offset, line) in line_byte_offsets(content) {
        if is_compact_heading(line) && byte_offset > 0 {
            pages.push(content[current_start..byte_offset].to_string());
            current_start = byte_offset;
        }
    }
    pages.push(content[current_start..].to_string());
    pages
}

/// Iterates over `(byte_start_of_line, line_str)` pairs in `content`.
fn line_byte_offsets(content: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut offset = 0;
    content.lines().map(move |line| {
        let start = offset;
        // lines() strips the newline; advance past the line content then the newline.
        let after = offset + line.len();
        let newline_len = if content.get(after..after + 2) == Some("\r\n") {
            2
        } else if content.get(after..after + 1) == Some("\n") {
            1
        } else {
            0
        };
        offset = after + newline_len;
        (start, line)
    })
}

/// Converts a signed page number to a 0-based index into a slice of `total` pages.
///
/// Positive: 1-based from start. Negative: -1 = last, -2 = second to last.
/// Returns `None` if out of range or zero.
pub(super) fn resolve_page_index(page: i32, total: usize) -> Option<usize> {
    if page == 0 || total == 0 {
        return None;
    }
    if page > 0 {
        let idx = (page as usize).checked_sub(1)?;
        if idx < total { Some(idx) } else { None }
    } else {
        let offset = (-page) as usize;
        total.checked_sub(offset)
    }
}

/// Reads the JSONL file at `path` and returns one timestamp entry per page:
/// index 0 = first event timestamp (page 1), index N = Nth compact_boundary timestamp (page N+1).
pub(super) fn extract_jsonl_compact_timestamps(path: &Path) -> Vec<Option<String>> {
    let mut timestamps: Vec<Option<String>> = Vec::new();
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return vec![None],
    };

    let reader = BufReader::new(file);
    let mut first_seen = false;
    for line in reader.lines() {
        let Ok(line) = line else { continue };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let ts = value
            .get("timestamp")
            .and_then(|v| v.as_str())
            .map(String::from);

        if !first_seen {
            timestamps.push(ts.clone());
            first_seen = true;
        }

        let is_compact = value.get("type").and_then(|v| v.as_str()) == Some("system")
            && value.get("subtype").and_then(|v| v.as_str()) == Some("compact_boundary");
        if is_compact {
            timestamps.push(ts);
        }
    }

    if !first_seen {
        timestamps.push(None);
    }
    timestamps
}

/// Truncates an ISO 8601 timestamp to 19 characters (`YYYY-MM-DDTHH:MM:SS`).
pub(super) fn format_timestamp_short(ts: &str) -> String {
    ts.chars().take(19).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compact_heading(n: usize) -> String {
        format!("## {n}. Context Compacted")
    }

    #[test]
    fn is_compact_heading_detects_valid_headings() {
        assert!(is_compact_heading("## 1. Context Compacted"));
        assert!(is_compact_heading("## 42. Context Compacted"));
        assert!(is_compact_heading("## 1. Context Compacted   "));
    }

    #[test]
    fn is_compact_heading_rejects_invalid_headings() {
        assert!(!is_compact_heading("## 1. User"));
        assert!(!is_compact_heading("## 1. Assistant"));
        assert!(!is_compact_heading("# 1. Context Compacted"));
        assert!(!is_compact_heading("## Context Compacted"));
        assert!(!is_compact_heading("## abc. Context Compacted"));
        assert!(!is_compact_heading("Some text ## 1. Context Compacted"));
    }

    fn make_markdown(sections: &[&str]) -> String {
        let header = "# Thread\n\n## Timeline\n\n";
        let body: String = sections
            .iter()
            .enumerate()
            .map(|(i, s)| {
                format!(
                    "## {}. {}\n\n{}\n\n",
                    i + 1,
                    s,
                    s.to_lowercase().replace(' ', "-")
                )
            })
            .collect();
        format!("{header}{body}")
    }

    #[test]
    fn split_markdown_pages_no_compact_returns_single_page() {
        let md = make_markdown(&["User", "Assistant", "User", "Assistant"]);
        let pages = split_markdown_pages(&md);
        assert_eq!(pages.len(), 1, "no compact headings → single page");
        assert_eq!(pages[0], md);
    }

    #[test]
    fn split_markdown_pages_one_compact_returns_two_pages() {
        let content = concat!(
            "# Thread\n\n",
            "## Timeline\n\n",
            "## 1. User\n\nhello\n\n",
            "## 2. Assistant\n\nworld\n\n",
            "## 3. Context Compacted\n\n[summary]\n\n",
            "## 4. User\n\ncontinue\n\n",
        );
        let pages = split_markdown_pages(content);
        assert_eq!(pages.len(), 2, "one compact heading → two pages");
        assert!(pages[0].contains("## 1. User"), "page 1 has first user msg");
        assert!(
            !pages[0].contains("Context Compacted"),
            "page 1 has no compact"
        );
        assert!(
            pages[1].contains("## 3. Context Compacted"),
            "page 2 starts with compact heading"
        );
        assert!(
            pages[1].contains("## 4. User"),
            "page 2 has post-compact user msg"
        );
    }

    #[test]
    fn split_markdown_pages_two_compacts_returns_three_pages() {
        let content = concat!(
            "## 1. User\n\nhello\n\n",
            "## 2. Context Compacted\n\n[compact1]\n\n",
            "## 3. User\n\ncontinue\n\n",
            "## 4. Context Compacted\n\n[compact2]\n\n",
            "## 5. Assistant\n\ndone\n\n",
        );
        let pages = split_markdown_pages(content);
        assert_eq!(pages.len(), 3);
        assert!(!pages[0].contains("Context Compacted"));
        assert!(pages[1].starts_with("## 2. Context Compacted"));
        assert!(pages[2].starts_with("## 4. Context Compacted"));
    }

    #[test]
    fn resolve_page_index_positive() {
        assert_eq!(resolve_page_index(1, 3), Some(0));
        assert_eq!(resolve_page_index(2, 3), Some(1));
        assert_eq!(resolve_page_index(3, 3), Some(2));
        assert_eq!(resolve_page_index(4, 3), None, "out of range");
    }

    #[test]
    fn resolve_page_index_negative() {
        assert_eq!(resolve_page_index(-1, 3), Some(2), "-1 = last");
        assert_eq!(resolve_page_index(-2, 3), Some(1), "-2 = second to last");
        assert_eq!(resolve_page_index(-3, 3), Some(0), "-3 = first");
        assert_eq!(resolve_page_index(-4, 3), None, "too negative");
    }

    #[test]
    fn resolve_page_index_zero_is_invalid() {
        assert_eq!(resolve_page_index(0, 3), None, "page 0 is invalid");
    }

    #[test]
    fn resolve_page_index_empty_returns_none() {
        assert_eq!(resolve_page_index(1, 0), None);
        assert_eq!(resolve_page_index(-1, 0), None);
    }

    #[test]
    fn extract_jsonl_compact_timestamps_from_real_events() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let path = temp_dir.path().join("session.jsonl");

        let events = [
            r#"{"type":"user","timestamp":"2026-05-15T10:00:00.000Z","message":{}}"#,
            r#"{"type":"assistant","timestamp":"2026-05-15T10:01:00.000Z","message":{}}"#,
            r#"{"type":"system","subtype":"compact_boundary","timestamp":"2026-05-15T14:30:00.000Z"}"#,
            r#"{"type":"user","timestamp":"2026-05-15T14:31:00.000Z","isCompactSummary":true,"message":{}}"#,
            r#"{"type":"system","subtype":"compact_boundary","timestamp":"2026-05-15T18:00:00.000Z"}"#,
        ];
        std::fs::write(&path, events.join("\n")).expect("write jsonl");

        let ts = extract_jsonl_compact_timestamps(&path);
        assert_eq!(ts.len(), 3, "first event + two compact boundaries");
        assert_eq!(
            ts[0].as_deref(),
            Some("2026-05-15T10:00:00.000Z"),
            "page 1 ts"
        );
        assert_eq!(
            ts[1].as_deref(),
            Some("2026-05-15T14:30:00.000Z"),
            "page 2 ts"
        );
        assert_eq!(
            ts[2].as_deref(),
            Some("2026-05-15T18:00:00.000Z"),
            "page 3 ts"
        );
    }

    #[test]
    fn extract_jsonl_compact_timestamps_missing_file_returns_fallback() {
        let ts = extract_jsonl_compact_timestamps(Path::new("/nonexistent/path.jsonl"));
        assert_eq!(ts, vec![None], "missing file → single None entry");
    }

    #[test]
    fn format_timestamp_short_truncates_correctly() {
        assert_eq!(
            format_timestamp_short("2026-05-15T10:00:00.000Z"),
            "2026-05-15T10:00:00"
        );
        assert_eq!(format_timestamp_short("2026-05-15"), "2026-05-15");
    }

    #[test]
    fn compact_heading_fn_produces_valid_heading() {
        assert!(is_compact_heading(&compact_heading(1)));
        assert!(is_compact_heading(&compact_heading(100)));
    }
}
