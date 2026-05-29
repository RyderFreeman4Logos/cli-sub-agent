//! Rendering and extraction of structured review sections (`summary`/`details`).
//!
//! These helpers parse CSA `<!-- CSA:SECTION:* -->` markers out of raw review
//! output and re-render only the substantive summary/details content, so caller-
//! facing review output is not polluted by unrelated provider noise.

use csa_session::OutputSection;
use csa_session::output_parser::parse_sections;

use super::truncate_review_result_summary;

/// Prefer structured review sections (summary/details) when available to avoid
/// leaking unrelated provider noise into caller-facing review output.
pub(crate) fn sanitize_review_output(output: &str) -> String {
    let sections = parse_sections(output);
    if sections.is_empty() {
        return output.to_string();
    }

    let summary = last_non_empty_section_content(output, &sections, "summary");
    let details = last_non_empty_section_content(output, &sections, "details");
    if summary.is_none() && details.is_none() {
        return output.to_string();
    }

    let mut rendered = String::new();
    if let Some(content) = summary {
        rendered.push_str("<!-- CSA:SECTION:summary -->\n");
        rendered.push_str(&content);
        if !content.ends_with('\n') {
            rendered.push('\n');
        }
        rendered.push_str("<!-- CSA:SECTION:summary:END -->\n");
    }
    if let Some(content) = details {
        if !rendered.is_empty() && !rendered.ends_with('\n') {
            rendered.push('\n');
        }
        rendered.push_str("<!-- CSA:SECTION:details -->\n");
        rendered.push_str(&content);
        if !content.ends_with('\n') {
            rendered.push('\n');
        }
        rendered.push_str("<!-- CSA:SECTION:details:END -->\n");
    }
    rendered
}

pub(crate) fn has_structured_review_content(output: &str) -> bool {
    let sanitized = sanitize_review_output(output);
    let sections = parse_sections(&sanitized);
    ["summary", "details"].into_iter().any(|section_id| {
        last_non_empty_section_content(&sanitized, &sections, section_id).is_some()
    })
}

pub(crate) fn derive_review_result_summary(output: &str) -> Option<String> {
    let sanitized = sanitize_review_output(output);
    let sections = parse_sections(&sanitized);
    let content = last_non_empty_section_content(&sanitized, &sections, "summary")
        .or_else(|| last_non_empty_section_content(&sanitized, &sections, "details"))?;

    content
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(truncate_review_result_summary)
}

fn last_non_empty_section_content(
    output: &str,
    sections: &[OutputSection],
    section_id: &str,
) -> Option<String> {
    sections
        .iter()
        .rev()
        .filter(|section| section.id == section_id)
        .find_map(|section| {
            let content = extract_section_content(output, section);
            if content.trim().is_empty() {
                None
            } else {
                Some(content)
            }
        })
}

fn extract_section_content(output: &str, section: &OutputSection) -> String {
    if section.line_start == 0 || section.line_end < section.line_start {
        return String::new();
    }

    let lines: Vec<&str> = output.lines().collect();
    if lines.is_empty() || section.line_start > lines.len() {
        return String::new();
    }

    let start = section.line_start - 1;
    let end_exclusive = section.line_end.min(lines.len());
    lines[start..end_exclusive].join("\n")
}
