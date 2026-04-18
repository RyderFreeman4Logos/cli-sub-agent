use std::fs;
use std::path::Path;

use anyhow::Result;
use csa_session::output_parser::parse_sections;
use csa_session::{OutputIndex, OutputSection};

use super::{derive_review_result_summary, has_structured_review_content, sanitize_review_output};

pub(crate) fn is_edit_restriction_summary(summary: &str) -> bool {
    summary.starts_with(super::EDIT_RESTRICTION_SUMMARY_PREFIX)
}

pub(crate) fn truncate_review_result_summary(line: &str) -> String {
    line.chars()
        .take(super::REVIEW_RESULT_SUMMARY_MAX_CHARS)
        .collect()
}

fn current_run_has_summary_section(output: &str) -> bool {
    let sanitized = sanitize_review_output(output);
    let sections = parse_sections(&sanitized);
    sections.iter().rev().any(|section| {
        section.id == "summary"
            && !extract_section_content(&sanitized, section)
                .trim()
                .is_empty()
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

pub(crate) fn ensure_review_summary_artifact(session_dir: &Path, output: &str) -> Result<()> {
    if !has_structured_review_content(output) {
        return Ok(());
    }

    let output_dir = session_dir.join("output");
    fs::create_dir_all(&output_dir)
        .map_err(|error| anyhow::anyhow!("create {}: {error}", output_dir.display()))?;
    let summary_path = output_dir.join("summary.md");
    let summary = if current_run_has_summary_section(output) {
        let Some(summary) = fs::read_to_string(&summary_path)
            .ok()
            .filter(|summary| !summary.trim().is_empty())
        else {
            return Ok(());
        };
        summary
    } else {
        let Some(summary) = derive_review_result_summary(output) else {
            return Ok(());
        };
        if summary.trim().is_empty() {
            return Ok(());
        }
        fs::write(&summary_path, &summary)
            .map_err(|error| anyhow::anyhow!("write {}: {error}", summary_path.display()))?;
        summary
    };

    let mut index = csa_session::load_output_index(session_dir)?.unwrap_or(OutputIndex {
        sections: Vec::new(),
        total_tokens: 0,
        total_lines: sanitize_review_output(output).lines().count(),
    });
    let token_estimate = csa_session::estimate_tokens(&summary);
    if let Some(section) = index
        .sections
        .iter_mut()
        .find(|section| section.id == "summary")
    {
        section.title = "Summary".to_string();
        section.file_path = Some("summary.md".to_string());
        section.token_estimate = token_estimate;
    } else {
        index.sections.insert(
            0,
            OutputSection {
                id: "summary".to_string(),
                title: "Summary".to_string(),
                line_start: 0,
                line_end: 0,
                token_estimate,
                file_path: Some("summary.md".to_string()),
            },
        );
    }
    index.total_tokens = index
        .sections
        .iter()
        .map(|section| section.token_estimate)
        .sum();
    let index_path = output_dir.join("index.toml");
    fs::write(
        &index_path,
        toml::to_string_pretty(&index)
            .map_err(|error| anyhow::anyhow!("serialize {}: {error}", index_path.display()))?,
    )
    .map_err(|error| anyhow::anyhow!("write {}: {error}", index_path.display()))?;

    Ok(())
}
