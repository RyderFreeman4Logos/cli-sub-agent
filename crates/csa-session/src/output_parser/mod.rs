//! Parser for CSA section markers in output.log.
//!
//! Scans output text for `<!-- CSA:SECTION:<id> -->` / `<!-- CSA:SECTION:<id>:END -->`
//! delimiter pairs and extracts structured [`OutputSection`]s.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use crate::output_section::{OutputIndex, OutputSection};

mod return_packet;

pub use return_packet::{parse_return_packet, validate_return_packet_path};

/// Marker prefix and suffix for section delimiters.
const MARKER_PREFIX: &str = "<!-- CSA:SECTION:";
const MARKER_END_SUFFIX: &str = ":END -->";
const MARKER_SUFFIX: &str = " -->";

/// Estimate token count from content using a simple word-based heuristic.
pub fn estimate_tokens(content: &str) -> usize {
    // ~4 chars per token on average; approximate via word count * 4/3
    content.split_whitespace().count() * 4 / 3
}

/// A raw marker detected during scanning.
#[derive(Debug)]
enum Marker {
    /// Section start: `<!-- CSA:SECTION:<id> -->`
    Start { id: String, line: usize },
    /// Section end: `<!-- CSA:SECTION:<id>:END -->`
    End { id: String, line: usize },
}

/// Scan lines for CSA section markers.
fn scan_markers(lines: &[&str]) -> Vec<Marker> {
    let mut markers = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix(MARKER_PREFIX) {
            if let Some(id) = rest.strip_suffix(MARKER_END_SUFFIX) {
                let id = id.trim();
                if !id.is_empty() {
                    markers.push(Marker::End {
                        id: id.to_string(),
                        line: i,
                    });
                }
            } else if let Some(id) = rest.strip_suffix(MARKER_SUFFIX) {
                let id = id.trim();
                if !id.is_empty() {
                    markers.push(Marker::Start {
                        id: id.to_string(),
                        line: i,
                    });
                }
            }
        }
    }
    markers
}

/// Parse output text into structured sections.
///
/// If no markers are found, returns a single "full" section covering the entire output.
pub fn parse_sections(output: &str) -> Vec<OutputSection> {
    let lines: Vec<&str> = output.lines().collect();
    let total_lines = lines.len();

    if total_lines == 0 {
        return vec![];
    }

    let markers = scan_markers(&lines);

    // No markers: single "full" section
    if markers.is_empty() {
        return vec![OutputSection {
            id: "full".to_string(),
            title: "Full Output".to_string(),
            line_start: 1,
            line_end: total_lines,
            token_estimate: estimate_tokens(output),
            file_path: Some("full.md".to_string()),
        }];
    }

    // Collect open sections from Start markers.
    // Sections are non-overlapping: a new Start closes all currently open sections.
    let mut sections = Vec::new();
    let mut open_start: Option<(String, usize)> = None; // (id, line_index)

    for marker in &markers {
        match marker {
            Marker::Start { id, line } => {
                // Close any currently open section at the line before this start marker
                if let Some((prev_id, start_line)) = open_start.take() {
                    let content_start = start_line + 1;
                    let content_end = line.saturating_sub(1);
                    let content = extract_content(&lines, content_start, content_end);
                    sections.push(build_section(
                        &prev_id,
                        content_start,
                        content_end,
                        &content,
                    ));
                }
                open_start = Some((id.clone(), *line));
            }
            Marker::End { id, line } => {
                if let Some((ref open_id, start_line)) = open_start {
                    if open_id == id {
                        let content_start = start_line + 1;
                        let content_end = line.saturating_sub(1);
                        let content = extract_content(&lines, content_start, content_end);
                        sections.push(build_section(id, content_start, content_end, &content));
                        open_start = None;
                    }
                }
                // Orphan or mismatched END marker: silently ignore
            }
        }
    }

    // Close remaining open section at EOF
    if let Some((id, start_line)) = open_start {
        let content_start = start_line + 1;
        let content_end = total_lines.saturating_sub(1);
        let content = extract_content(&lines, content_start, content_end);
        sections.push(build_section(&id, content_start, content_end, &content));
    }

    // Sort by line_start for deterministic order
    sections.sort_by_key(|s| s.line_start);

    // Fallback: if markers existed but all were orphaned/mismatched (no sections
    // produced), create a single "full" section so downstream consumers don't
    // lose the actual output content.
    if sections.is_empty() {
        return vec![OutputSection {
            id: "full".to_string(),
            title: "Full Output".to_string(),
            line_start: 1,
            line_end: total_lines,
            token_estimate: estimate_tokens(output),
            file_path: Some("full.md".to_string()),
        }];
    }

    // Deduplicate file paths: when the same section ID appears multiple times,
    // append a numeric suffix to avoid later writes overwriting earlier content.
    deduplicate_file_paths(&mut sections);

    sections
}

/// Extract content lines between start (inclusive) and end (inclusive), both 0-indexed.
fn extract_content(lines: &[&str], start: usize, end: usize) -> String {
    if start > end || start >= lines.len() {
        return String::new();
    }
    let end = end.min(lines.len() - 1);
    lines[start..=end].join("\n")
}

/// Sanitize a section ID to prevent path traversal.
///
/// Only allows alphanumeric, `-`, `_`, and `.` characters.
/// Any other character (including `/`, `\`, and `..` sequences) is replaced with `_`.
fn sanitize_section_id(id: &str) -> String {
    let sanitized: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    // Reject `..` sequences that survived character-level filtering
    sanitized.replace("..", "_")
}

/// Build an OutputSection from parsed data.
fn build_section(
    id: &str,
    content_start: usize,
    content_end: usize,
    content: &str,
) -> OutputSection {
    let safe_id = sanitize_section_id(id);
    // Convert 0-indexed to 1-based line numbers.
    // When content_end < content_start the section is empty (adjacent markers);
    // preserve an empty span by keeping line_end = line_start - 1.
    let line_start = content_start + 1;
    let line_end = if content_end < content_start {
        // Empty section: line_end < line_start signals zero content
        line_start.saturating_sub(1)
    } else {
        content_end + 1
    };
    let title = id_to_title(&safe_id);

    OutputSection {
        id: safe_id.clone(),
        title,
        line_start,
        line_end,
        token_estimate: estimate_tokens(content),
        file_path: Some(format!("{safe_id}.md")),
    }
}

/// Convert a kebab-case or snake_case id to a title-case string.
fn id_to_title(id: &str) -> String {
    id.split(['-', '_'])
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => {
                    let upper: String = first.to_uppercase().collect();
                    format!("{upper}{}", chars.as_str())
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Deduplicate file paths for sections with the same sanitized ID.
///
/// First occurrence keeps `<id>.md`, subsequent occurrences get `<id>-2.md`, `<id>-3.md`, etc.
fn deduplicate_file_paths(sections: &mut [OutputSection]) {
    let mut seen: HashMap<String, u32> = HashMap::new();
    for section in sections.iter_mut() {
        let count = seen.entry(section.id.clone()).or_insert(0);
        *count += 1;
        if *count > 1 {
            let deduped_file = format!("{}-{}.md", section.id, count);
            section.file_path = Some(deduped_file);
        }
    }
}

/// Parse output.log and persist structured sections to the session's output/ directory.
///
/// - Parses markers from `output_log` content
/// - Writes each section to `{session_dir}/output/<id>.md`
/// - Writes the index to `{session_dir}/output/index.toml`
/// - Returns the [`OutputIndex`]
pub fn persist_structured_output(session_dir: &Path, output_log: &str) -> Result<OutputIndex> {
    let sections = parse_sections(output_log);

    let output_dir = session_dir.join("output");
    fs::create_dir_all(&output_dir)
        .with_context(|| format!("Failed to create output dir: {}", output_dir.display()))?;

    let lines: Vec<&str> = output_log.lines().collect();
    let total_lines = lines.len();

    // Write each section file
    for section in &sections {
        if let Some(ref file_path) = section.file_path {
            // Extract content from the original output using line ranges (1-based to 0-based)
            let start_idx = section.line_start.saturating_sub(1);
            let end_idx = section.line_end.min(total_lines);
            let content = if start_idx < total_lines {
                lines[start_idx..end_idx].join("\n")
            } else {
                String::new()
            };

            let section_path = output_dir.join(file_path);
            fs::write(&section_path, &content).with_context(|| {
                format!("Failed to write section file: {}", section_path.display())
            })?;
        }
    }

    let total_tokens = sections.iter().map(|s| s.token_estimate).sum();

    let index = OutputIndex {
        sections,
        total_tokens,
        total_lines,
    };

    let index_path = output_dir.join("index.toml");
    let index_toml = toml::to_string_pretty(&index).context("Failed to serialize output index")?;
    fs::write(&index_path, &index_toml)
        .with_context(|| format!("Failed to write index: {}", index_path.display()))?;

    Ok(index)
}

/// Load the structured output index from a session directory.
///
/// Returns `Ok(None)` if no `output/index.toml` exists.
pub fn load_output_index(session_dir: &Path) -> Result<Option<OutputIndex>> {
    let index_path = session_dir.join("output").join("index.toml");
    if !index_path.is_file() {
        return Ok(None);
    }
    let content = fs::read_to_string(&index_path)
        .with_context(|| format!("Failed to read {}", index_path.display()))?;
    let index: OutputIndex =
        toml::from_str(&content).with_context(|| "Failed to parse output/index.toml")?;
    Ok(Some(index))
}

/// Read a specific section's content by ID from the session's output directory.
///
/// Returns `Ok(None)` if no index exists or section ID is not found.
pub fn read_section(session_dir: &Path, section_id: &str) -> Result<Option<String>> {
    let Some(index) = load_output_index(session_dir)? else {
        return Ok(None);
    };
    let section = index.sections.iter().find(|s| s.id == section_id);
    let Some(section) = section else {
        return Ok(None);
    };
    let Some(ref file_path) = section.file_path else {
        return Ok(None);
    };
    let section_path = session_dir.join("output").join(file_path);
    if !section_path.is_file() {
        return Ok(None);
    }
    let content = fs::read_to_string(&section_path)
        .with_context(|| format!("Failed to read section file: {}", section_path.display()))?;
    Ok(Some(content))
}

/// Read all sections' content in index order.
///
/// Returns a vec of `(OutputSection, content)` pairs. Returns empty vec if no index exists.
pub fn read_all_sections(session_dir: &Path) -> Result<Vec<(OutputSection, String)>> {
    let Some(index) = load_output_index(session_dir)? else {
        return Ok(vec![]);
    };
    let mut results = Vec::with_capacity(index.sections.len());
    for section in &index.sections {
        let content = if let Some(ref file_path) = section.file_path {
            let section_path = session_dir.join("output").join(file_path);
            if section_path.is_file() {
                fs::read_to_string(&section_path).with_context(|| {
                    format!("Failed to read section file: {}", section_path.display())
                })?
            } else {
                String::new()
            }
        } else {
            String::new()
        };
        results.push((section.clone(), content));
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_parse_no_markers_returns_full_section() {
        let output = "line 1\nline 2\nline 3\n";
        let sections = parse_sections(output);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].id, "full");
        assert_eq!(sections[0].line_start, 1);
        assert_eq!(sections[0].line_end, 3);
        assert!(sections[0].token_estimate > 0);
    }

    #[test]
    fn test_parse_empty_output() {
        let sections = parse_sections("");
        assert!(sections.is_empty());
    }

    #[test]
    fn test_parse_single_section_with_end_marker() {
        let output = "preamble\n\
                       <!-- CSA:SECTION:summary -->\n\
                       This is the summary.\n\
                       It has two lines.\n\
                       <!-- CSA:SECTION:summary:END -->\n\
                       postamble";
        let sections = parse_sections(output);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].id, "summary");
        assert_eq!(sections[0].title, "Summary");
        assert_eq!(sections[0].line_start, 3);
        assert_eq!(sections[0].line_end, 4);
    }

    #[test]
    fn test_parse_multiple_sections() {
        let output = "<!-- CSA:SECTION:intro -->\n\
                       Hello world\n\
                       <!-- CSA:SECTION:intro:END -->\n\
                       <!-- CSA:SECTION:details -->\n\
                       Some details here\n\
                       <!-- CSA:SECTION:details:END -->";
        let sections = parse_sections(output);
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].id, "intro");
        assert_eq!(sections[1].id, "details");
    }

    #[test]
    fn test_parse_missing_end_marker_extends_to_eof() {
        let output = "<!-- CSA:SECTION:analysis -->\n\
                       Line A\n\
                       Line B\n\
                       Line C";
        let sections = parse_sections(output);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].id, "analysis");
        assert_eq!(sections[0].line_start, 2);
        assert_eq!(sections[0].line_end, 4);
    }

    #[test]
    fn test_parse_missing_end_closes_at_next_start() {
        let output = "<!-- CSA:SECTION:first -->\n\
                       content first\n\
                       <!-- CSA:SECTION:second -->\n\
                       content second";
        let sections = parse_sections(output);
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].id, "first");
        assert_eq!(sections[0].line_start, 2);
        assert_eq!(sections[0].line_end, 2);
        assert_eq!(sections[1].id, "second");
        assert_eq!(sections[1].line_start, 4);
        assert_eq!(sections[1].line_end, 4);
    }

    #[test]
    fn test_parse_orphan_end_marker_falls_back_to_full() {
        let output = "some text\n\
                       <!-- CSA:SECTION:ghost:END -->\n\
                       more text";
        let sections = parse_sections(output);
        // Orphaned END markers produce no matched sections, so the parser falls
        // back to a single "full" section to avoid losing the output content.
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].id, "full");
        assert_eq!(sections[0].line_start, 1);
        assert_eq!(sections[0].line_end, 3);
    }

    #[test]
    fn test_parse_duplicate_start_closes_first() {
        let output = "<!-- CSA:SECTION:dup -->\n\
                       first content\n\
                       <!-- CSA:SECTION:dup -->\n\
                       second content\n\
                       <!-- CSA:SECTION:dup:END -->";
        let sections = parse_sections(output);
        assert_eq!(sections.len(), 2);
        // First occurrence closed at second start
        assert_eq!(sections[0].line_start, 2);
        assert_eq!(sections[0].line_end, 2);
        // Second occurrence closed by END
        assert_eq!(sections[1].line_start, 4);
        assert_eq!(sections[1].line_end, 4);
        // Deduplicated file paths: first keeps original, second gets suffix
        assert_eq!(sections[0].file_path.as_deref(), Some("dup.md"));
        assert_eq!(sections[1].file_path.as_deref(), Some("dup-2.md"));
    }

    #[test]
    fn test_parse_whitespace_around_markers() {
        let output = "  <!-- CSA:SECTION:padded -->  \n\
                       content\n\
                       \t<!-- CSA:SECTION:padded:END -->\t";
        let sections = parse_sections(output);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].id, "padded");
    }

    #[test]
    fn test_estimate_tokens() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("hello world foo bar"), 4 * 4 / 3); // 5 (integer division)
    }

    #[test]
    fn test_id_to_title() {
        assert_eq!(id_to_title("summary"), "Summary");
        assert_eq!(id_to_title("exec-plan"), "Exec Plan");
        assert_eq!(id_to_title("code_review"), "Code Review");
        assert_eq!(id_to_title("a-b_c"), "A B C");
    }

    #[test]
    fn test_persist_structured_output_no_markers() {
        let tmp = tempfile::tempdir().unwrap();
        let output = "Hello world\nSecond line\n";
        let index = persist_structured_output(tmp.path(), output).unwrap();

        assert_eq!(index.sections.len(), 1);
        assert_eq!(index.sections[0].id, "full");
        assert_eq!(index.total_lines, 2);

        // Verify index.toml written
        let index_path = tmp.path().join("output/index.toml");
        assert!(index_path.exists());
        let loaded: OutputIndex =
            toml::from_str(&fs::read_to_string(&index_path).unwrap()).unwrap();
        assert_eq!(loaded, index);

        // Verify section file written
        let section_path = tmp.path().join("output/full.md");
        assert!(section_path.exists());
    }

    #[test]
    fn test_persist_structured_output_with_markers() {
        let tmp = tempfile::tempdir().unwrap();
        let output = "preamble\n\
                       <!-- CSA:SECTION:summary -->\n\
                       Summary content here.\n\
                       <!-- CSA:SECTION:summary:END -->\n\
                       <!-- CSA:SECTION:details -->\n\
                       Detail line 1.\n\
                       Detail line 2.\n\
                       <!-- CSA:SECTION:details:END -->";
        let index = persist_structured_output(tmp.path(), output).unwrap();

        assert_eq!(index.sections.len(), 2);
        assert_eq!(index.sections[0].id, "summary");
        assert_eq!(index.sections[1].id, "details");

        // Verify section files
        let summary_content = fs::read_to_string(tmp.path().join("output/summary.md")).unwrap();
        assert!(summary_content.contains("Summary content"));
        let details_content = fs::read_to_string(tmp.path().join("output/details.md")).unwrap();
        assert!(details_content.contains("Detail line 1"));

        // Verify index round-trip
        let index_toml = fs::read_to_string(tmp.path().join("output/index.toml")).unwrap();
        let loaded: OutputIndex = toml::from_str(&index_toml).unwrap();
        assert_eq!(loaded.sections.len(), 2);
    }

    #[test]
    fn test_persist_structured_output_duplicate_ids() {
        let tmp = tempfile::tempdir().unwrap();
        let output = "<!-- CSA:SECTION:review -->\n\
                       First review content\n\
                       <!-- CSA:SECTION:review:END -->\n\
                       <!-- CSA:SECTION:review -->\n\
                       Second review content\n\
                       <!-- CSA:SECTION:review:END -->";
        let index = persist_structured_output(tmp.path(), output).unwrap();

        assert_eq!(index.sections.len(), 2);
        assert_eq!(index.sections[0].file_path.as_deref(), Some("review.md"));
        assert_eq!(index.sections[1].file_path.as_deref(), Some("review-2.md"));

        // Both files exist and have distinct content
        let first = fs::read_to_string(tmp.path().join("output/review.md")).unwrap();
        let second = fs::read_to_string(tmp.path().join("output/review-2.md")).unwrap();
        assert!(first.contains("First review content"));
        assert!(second.contains("Second review content"));
    }

    #[test]
    fn test_persist_structured_output_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let index = persist_structured_output(tmp.path(), "").unwrap();

        assert!(index.sections.is_empty());
        assert_eq!(index.total_tokens, 0);
        assert_eq!(index.total_lines, 0);

        let index_path = tmp.path().join("output/index.toml");
        assert!(index_path.exists());
    }

    // ── load_output_index tests ───────────────────────────────────────

    #[test]
    fn test_load_output_index_returns_none_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let result = load_output_index(tmp.path()).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_load_output_index_returns_persisted_index() {
        let tmp = tempfile::tempdir().unwrap();
        let output = "<!-- CSA:SECTION:summary -->\nHello\n<!-- CSA:SECTION:summary:END -->";
        persist_structured_output(tmp.path(), output).unwrap();

        let loaded = load_output_index(tmp.path()).unwrap().unwrap();
        assert_eq!(loaded.sections.len(), 1);
        assert_eq!(loaded.sections[0].id, "summary");
    }

    // ── read_section tests ────────────────────────────────────────────

    #[test]
    fn test_read_section_returns_none_when_no_index() {
        let tmp = tempfile::tempdir().unwrap();
        let result = read_section(tmp.path(), "summary").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_read_section_returns_none_for_unknown_id() {
        let tmp = tempfile::tempdir().unwrap();
        let output = "<!-- CSA:SECTION:summary -->\nHello\n<!-- CSA:SECTION:summary:END -->";
        persist_structured_output(tmp.path(), output).unwrap();

        let result = read_section(tmp.path(), "nonexistent").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_read_section_returns_content_for_valid_id() {
        let tmp = tempfile::tempdir().unwrap();
        let output =
            "<!-- CSA:SECTION:summary -->\nSummary content here\n<!-- CSA:SECTION:summary:END -->";
        persist_structured_output(tmp.path(), output).unwrap();

        let content = read_section(tmp.path(), "summary").unwrap().unwrap();
        assert!(content.contains("Summary content here"));
    }

    #[test]
    fn test_read_section_with_multiple_sections() {
        let tmp = tempfile::tempdir().unwrap();
        let output = "<!-- CSA:SECTION:intro -->\nIntro text\n<!-- CSA:SECTION:intro:END -->\n\
                       <!-- CSA:SECTION:details -->\nDetail text\n<!-- CSA:SECTION:details:END -->";
        persist_structured_output(tmp.path(), output).unwrap();

        let intro = read_section(tmp.path(), "intro").unwrap().unwrap();
        assert!(intro.contains("Intro text"));

        let details = read_section(tmp.path(), "details").unwrap().unwrap();
        assert!(details.contains("Detail text"));
    }

    // ── read_all_sections tests ───────────────────────────────────────

    #[test]
    fn test_read_all_sections_returns_empty_when_no_index() {
        let tmp = tempfile::tempdir().unwrap();
        let result = read_all_sections(tmp.path()).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_read_all_sections_returns_all_in_order() {
        let tmp = tempfile::tempdir().unwrap();
        let output = "<!-- CSA:SECTION:summary -->\nSummary\n<!-- CSA:SECTION:summary:END -->\n\
                       <!-- CSA:SECTION:details -->\nDetails\n<!-- CSA:SECTION:details:END -->\n\
                       <!-- CSA:SECTION:conclusion -->\nConclusion\n<!-- CSA:SECTION:conclusion:END -->";
        persist_structured_output(tmp.path(), output).unwrap();

        let sections = read_all_sections(tmp.path()).unwrap();
        assert_eq!(sections.len(), 3);
        assert_eq!(sections[0].0.id, "summary");
        assert!(sections[0].1.contains("Summary"));
        assert_eq!(sections[1].0.id, "details");
        assert!(sections[1].1.contains("Details"));
        assert_eq!(sections[2].0.id, "conclusion");
        assert!(sections[2].1.contains("Conclusion"));
    }

    #[test]
    fn test_read_all_sections_with_no_markers_returns_full() {
        let tmp = tempfile::tempdir().unwrap();
        let output = "Line 1\nLine 2\nLine 3";
        persist_structured_output(tmp.path(), output).unwrap();

        let sections = read_all_sections(tmp.path()).unwrap();
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].0.id, "full");
        assert!(sections[0].1.contains("Line 1"));
    }

    // ── empty section tests ────────────────────────────────────────

    #[test]
    fn test_parse_empty_section_preserves_empty_span() {
        // Adjacent start/end markers with no content between them
        let output = "preamble\n\
                       <!-- CSA:SECTION:empty -->\n\
                       <!-- CSA:SECTION:empty:END -->\n\
                       postamble";
        let sections = parse_sections(output);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].id, "empty");
        // Empty section: line_end < line_start
        assert!(
            sections[0].line_end < sections[0].line_start,
            "Empty section should have line_end < line_start, got start={} end={}",
            sections[0].line_start,
            sections[0].line_end,
        );
        assert_eq!(sections[0].token_estimate, 0);
    }

    // ── section ID sanitization tests ──────────────────────────────

    #[test]
    fn test_sanitize_section_id_allows_safe_chars() {
        assert_eq!(sanitize_section_id("summary"), "summary");
        assert_eq!(sanitize_section_id("exec-plan"), "exec-plan");
        assert_eq!(sanitize_section_id("code_review"), "code_review");
        assert_eq!(sanitize_section_id("v1.2.3"), "v1.2.3");
    }

    #[test]
    fn test_sanitize_section_id_blocks_path_traversal() {
        let sanitized = sanitize_section_id("../../outside");
        assert!(
            !sanitized.contains('/'),
            "Sanitized ID should not contain '/': {sanitized}"
        );
        assert!(
            !sanitized.contains(".."),
            "Sanitized ID should not contain '..': {sanitized}"
        );
    }

    #[test]
    fn test_sanitize_section_id_blocks_backslash() {
        let sanitized = sanitize_section_id("..\\..\\outside");
        assert!(
            !sanitized.contains('\\'),
            "Sanitized ID should not contain backslash: {sanitized}"
        );
        assert!(
            !sanitized.contains(".."),
            "Sanitized ID should not contain '..': {sanitized}"
        );
    }

    #[test]
    fn test_persist_structured_output_sanitizes_section_ids() {
        let tmp = tempfile::tempdir().unwrap();
        let output =
            "<!-- CSA:SECTION:../../escape -->\nmalicious\n<!-- CSA:SECTION:../../escape:END -->";
        let index = persist_structured_output(tmp.path(), output).unwrap();

        assert_eq!(index.sections.len(), 1);
        let section = &index.sections[0];
        // ID should be sanitized
        assert!(
            !section.id.contains('/'),
            "Section ID should be sanitized: {}",
            section.id
        );
        assert!(
            !section.id.contains(".."),
            "Section ID should not contain '..': {}",
            section.id
        );
        // File should be written inside output dir, not escaped
        if let Some(ref fp) = section.file_path {
            let section_path = tmp.path().join("output").join(fp);
            assert!(
                section_path.exists(),
                "Section file should exist at safe path"
            );
        }
    }
}
