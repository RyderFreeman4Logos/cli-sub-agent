//! Design context extraction from markdown documents.
//!
//! Extracts Key Decisions, Constraints, and Threats sections from a design
//! document and formats them as `<design-context>` XML tags for prompt injection.

/// Maximum character budget for design context injection (~2000 tokens).
const MAX_DESIGN_CONTEXT_CHARS: usize = 6000;

/// Section headings to extract from design.md (case-insensitive match).
const DESIGN_SECTIONS: &[&str] = &["Key Decisions", "Constraints", "Threats"];

/// Extract relevant sections from a design document's markdown content.
///
/// Looks for headings matching [`DESIGN_SECTIONS`] (case-insensitive, any heading level).
/// Each matched section includes content until the next heading of equal or higher level.
/// The result is truncated to `max_chars` (default [`MAX_DESIGN_CONTEXT_CHARS`]).
///
/// Returns `None` if no matching sections are found.
pub fn extract_design_sections(content: &str, max_chars: Option<usize>) -> Option<String> {
    let limit = max_chars.unwrap_or(MAX_DESIGN_CONTEXT_CHARS);
    let mut result = String::new();

    let lines: Vec<&str> = content.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];

        // Detect markdown heading: count leading '#' chars.
        if let Some((level, title)) = parse_heading(line) {
            let title_lower = title.to_lowercase();
            let is_target = DESIGN_SECTIONS
                .iter()
                .any(|s| title_lower.contains(&s.to_lowercase()));

            if is_target {
                // Include the heading itself.
                if !result.is_empty() {
                    result.push('\n');
                }
                result.push_str(line);
                result.push('\n');
                i += 1;

                // Collect body until next heading of equal or higher level.
                while i < lines.len() {
                    if let Some((next_level, _)) = parse_heading(lines[i])
                        && next_level <= level
                    {
                        break;
                    }
                    result.push_str(lines[i]);
                    result.push('\n');
                    i += 1;
                }
                continue;
            }
        }
        i += 1;
    }

    let trimmed = result.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Truncate to character budget, breaking at last newline within limit.
    let output = if trimmed.len() <= limit {
        trimmed.to_string()
    } else {
        let truncated = &trimmed[..limit];
        // Find last newline to avoid cutting mid-line.
        if let Some(pos) = truncated.rfind('\n') {
            format!("{}\n[...truncated]", &truncated[..pos])
        } else {
            format!("{}\n[...truncated]", truncated)
        }
    };

    Some(output)
}

/// Wrap extracted design sections in `<design-context>` tags for prompt injection.
pub fn format_design_context(branch: &str, sections: &str) -> String {
    format!("<design-context branch=\"{branch}\">\n{sections}\n</design-context>\n\n")
}

/// Parse a markdown heading line, returning (level, title text).
///
/// Returns `None` if the line is not a heading.
fn parse_heading(line: &str) -> Option<(usize, &str)> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('#') {
        return None;
    }
    let level = trimmed.bytes().take_while(|&b| b == b'#').count();
    if level == 0 || level > 6 {
        return None;
    }
    // Must have a space after the '#' characters (standard markdown).
    let rest = &trimmed[level..];
    if !rest.starts_with(' ') {
        return None;
    }
    Some((level, rest[1..].trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_design_sections_finds_all_target_sections() {
        let content = "\
# Design Document

## Overview
Some overview text.

## Key Decisions
- Use Arc instead of Rc for thread safety
- Prefer trait objects over generics at boundaries

## Implementation Notes
Details about implementation.

## Constraints
- Token budget <= 2000
- Must not break existing API

## Threats
- Race condition in session cleanup
- OOM on large codebases
";
        let result = extract_design_sections(content, None).unwrap();
        assert!(result.contains("## Key Decisions"));
        assert!(result.contains("Arc instead of Rc"));
        assert!(result.contains("## Constraints"));
        assert!(result.contains("Token budget"));
        assert!(result.contains("## Threats"));
        assert!(result.contains("Race condition"));
        // Should NOT include non-target sections.
        assert!(!result.contains("## Overview"));
        assert!(!result.contains("## Implementation Notes"));
    }

    #[test]
    fn test_extract_design_sections_case_insensitive() {
        let content = "\
## key decisions
- Decision A

## CONSTRAINTS
- Constraint B
";
        let result = extract_design_sections(content, None).unwrap();
        assert!(result.contains("Decision A"));
        assert!(result.contains("Constraint B"));
    }

    #[test]
    fn test_extract_design_sections_none_when_no_match() {
        let content = "\
## Overview
Just an overview.

## Implementation
Some code details.
";
        assert!(extract_design_sections(content, None).is_none());
    }

    #[test]
    fn test_extract_design_sections_truncates_to_budget() {
        let mut content = String::from("## Key Decisions\n");
        // Generate content exceeding 6000 chars.
        for i in 0..200 {
            content.push_str(&format!(
                "- Decision item number {i} with extra padding text\n"
            ));
        }
        let result = extract_design_sections(&content, Some(500)).unwrap();
        assert!(result.len() <= 520); // 500 + "[...truncated]\n" overhead
        assert!(result.contains("[...truncated]"));
    }

    #[test]
    fn test_extract_design_sections_handles_mixed_heading_levels() {
        let content = "\
# Top Level
## Key Decisions
- Decision A
### Sub-decisions
- Sub-decision B
## Next Section
Unrelated.
";
        let result = extract_design_sections(content, None).unwrap();
        assert!(result.contains("Decision A"));
        assert!(result.contains("Sub-decision B"));
        assert!(!result.contains("Unrelated"));
    }

    #[test]
    fn test_extract_design_sections_partial_match() {
        let content = "\
## Constraints
- Only constraint here.
";
        let result = extract_design_sections(content, None).unwrap();
        assert!(result.contains("## Constraints"));
        assert!(result.contains("Only constraint here"));
    }

    #[test]
    fn test_format_design_context_wraps_correctly() {
        let output = format_design_context("feat/my-feature", "## Key Decisions\n- Use Arc");
        assert!(output.contains("<design-context branch=\"feat/my-feature\">"));
        assert!(output.contains("## Key Decisions"));
        assert!(output.contains("</design-context>"));
    }

    #[test]
    fn test_parse_heading_valid() {
        assert_eq!(parse_heading("# Title"), Some((1, "Title")));
        assert_eq!(parse_heading("## Section"), Some((2, "Section")));
        assert_eq!(parse_heading("### Sub"), Some((3, "Sub")));
        assert_eq!(parse_heading("  ## Indented"), Some((2, "Indented")));
    }

    #[test]
    fn test_parse_heading_invalid() {
        assert!(parse_heading("Not a heading").is_none());
        assert!(parse_heading("#NoSpace").is_none());
        assert!(parse_heading("").is_none());
        assert!(parse_heading("####### Seven levels").is_none());
    }
}
