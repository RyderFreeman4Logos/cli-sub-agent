//! Streaming two-pass parser for large output.log files.
//!
//! Avoids loading the entire file into memory, which is critical for
//! multi-GB output files from long codex sessions.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::output_section::{OutputIndex, OutputSection};

use super::{
    MARKER_END_SUFFIX, MARKER_PREFIX, MARKER_SUFFIX, Marker, deduplicate_file_paths,
    estimate_tokens, id_to_title, persist_structured_output, sanitize_section_id,
};

/// Maximum number of sections to parse from a single output file.
///
/// Prevents file-descriptor exhaustion from malformed or adversarial output
/// containing thousands of section markers.
const MAX_SECTIONS: usize = 64;

#[derive(Debug, Deserialize)]
struct TranscriptEvent {
    #[serde(rename = "type")]
    event_type: String,
    #[serde(default)]
    item: Option<TranscriptItem>,
}

#[derive(Debug, Deserialize)]
struct TranscriptItem {
    #[serde(rename = "type")]
    item_type: String,
    #[serde(default)]
    text: Option<String>,
}

/// Parse output.log from a file path using streaming reads.
///
/// Two-pass approach:
/// 1. Scan the file line-by-line for section markers.
/// 2. Re-read the file and write each section's content to disk.
pub fn persist_structured_output_from_file(
    session_dir: &Path,
    output_log_path: &Path,
) -> Result<OutputIndex> {
    if let Some(flattened) = flatten_transcript_for_sections(output_log_path)? {
        return persist_structured_output(session_dir, &flattened);
    }

    let file = fs::File::open(output_log_path)
        .with_context(|| format!("Failed to open {}", output_log_path.display()))?;
    let reader = BufReader::new(file);

    // Pass 1: scan markers and count lines.
    let mut markers = Vec::new();
    let mut total_lines = 0usize;
    for line_result in reader.lines() {
        let line = line_result.context("Failed to read line from output.log")?;
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix(MARKER_PREFIX) {
            if let Some(id) = rest.strip_suffix(MARKER_END_SUFFIX) {
                let id = id.trim();
                if !id.is_empty() {
                    markers.push(Marker::End {
                        id: id.to_string(),
                        line: total_lines,
                    });
                }
            } else if let Some(id) = rest.strip_suffix(MARKER_SUFFIX) {
                let id = id.trim();
                if !id.is_empty() {
                    markers.push(Marker::Start {
                        id: id.to_string(),
                        line: total_lines,
                    });
                }
            }
        }
        total_lines += 1;
    }

    if total_lines == 0 {
        let output_dir = session_dir.join("output");
        fs::create_dir_all(&output_dir)?;
        let index = OutputIndex {
            sections: vec![],
            total_tokens: 0,
            total_lines: 0,
        };
        let index_path = output_dir.join("index.toml");
        let index_toml = toml::to_string_pretty(&index)?;
        fs::write(&index_path, &index_toml)?;
        return Ok(index);
    }

    // Build sections from markers (same logic as parse_sections).
    let mut sections = Vec::new();
    let mut open_start: Option<(String, usize)> = None;

    if markers.is_empty() {
        // No markers: single "full" section.
        sections.push(OutputSection {
            id: "full".to_string(),
            title: "Full Output".to_string(),
            line_start: 1,
            line_end: total_lines,
            token_estimate: 0, // updated in pass 2
            file_path: Some("full.md".to_string()),
        });
    } else {
        for marker in &markers {
            match marker {
                Marker::Start { id, line } => {
                    if let Some((prev_id, start_line)) = open_start.take() {
                        let content_start = start_line + 1;
                        let content_end = line.saturating_sub(1);
                        sections.push(build_section_no_content(
                            &prev_id,
                            content_start,
                            content_end,
                        ));
                    }
                    open_start = Some((id.clone(), *line));
                }
                Marker::End { id, line } => {
                    if let Some((ref open_id, start_line)) = open_start
                        && open_id == id
                    {
                        let content_start = start_line + 1;
                        let content_end = line.saturating_sub(1);
                        sections.push(build_section_no_content(id, content_start, content_end));
                        open_start = None;
                    }
                }
            }
        }
        if let Some((id, start_line)) = open_start {
            let content_start = start_line + 1;
            let content_end = total_lines.saturating_sub(1);
            sections.push(build_section_no_content(&id, content_start, content_end));
        }
        sections.sort_by_key(|s| s.line_start);
        sections.truncate(MAX_SECTIONS);
        if sections.is_empty() {
            sections.push(OutputSection {
                id: "full".to_string(),
                title: "Full Output".to_string(),
                line_start: 1,
                line_end: total_lines,
                token_estimate: 0,
                file_path: Some("full.md".to_string()),
            });
        }
        deduplicate_file_paths(&mut sections);
    }

    // Pass 2: re-read file and write section content files.
    let output_dir = session_dir.join("output");
    fs::create_dir_all(&output_dir)?;

    let file2 = fs::File::open(output_log_path)?;
    let reader2 = BufReader::new(file2);

    // Prepare writers for each section.
    let mut section_writers: Vec<Option<std::io::BufWriter<fs::File>>> =
        Vec::with_capacity(sections.len());
    let mut section_token_counts: Vec<usize> = vec![0; sections.len()];
    for section in &sections {
        let writer = if let Some(ref fp) = section.file_path {
            let path = output_dir.join(fp);
            let file = fs::File::create(&path)
                .with_context(|| format!("Failed to create section file: {}", path.display()))?;
            Some(std::io::BufWriter::new(file))
        } else {
            None
        };
        section_writers.push(writer);
    }

    for (line_idx, line_result) in reader2.lines().enumerate() {
        let line = line_result.context("Failed to read line (pass 2)")?;
        // 1-based line number
        let line_num = line_idx + 1;

        for (si, section) in sections.iter().enumerate() {
            let start = section.line_start;
            let end = section.line_end;
            if start > end {
                continue; // empty section
            }
            if line_num >= start && line_num <= end {
                if let Some(ref mut writer) = section_writers[si] {
                    use std::io::Write;
                    if line_num > start {
                        writeln!(writer)?;
                    }
                    write!(writer, "{line}")?;
                }
                section_token_counts[si] += estimate_tokens(&line);
            }
        }
    }

    // Flush all writers.
    for w in section_writers.iter_mut().flatten() {
        use std::io::Write;
        w.flush()?;
    }

    // Update token estimates.
    for (i, section) in sections.iter_mut().enumerate() {
        section.token_estimate = section_token_counts[i];
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

fn flatten_transcript_for_sections(output_log_path: &Path) -> Result<Option<String>> {
    let file = fs::File::open(output_log_path)
        .with_context(|| format!("Failed to open {}", output_log_path.display()))?;
    let reader = BufReader::new(file);

    let mut first_non_empty_line = None;
    let mut saw_json_line = false;
    let mut flattened = String::new();

    for line_result in reader.lines() {
        let line = line_result
            .with_context(|| format!("Failed to read line from {}", output_log_path.display()))?;
        if line.trim().is_empty() {
            continue;
        }

        if first_non_empty_line.is_none() {
            first_non_empty_line = Some(line.clone());
            if serde_json::from_str::<TranscriptEvent>(&line).is_err() {
                return Ok(None);
            }
        }

        let Ok(event) = serde_json::from_str::<TranscriptEvent>(&line) else {
            return Ok(None);
        };
        saw_json_line = true;

        let Some(item) = event.item else {
            continue;
        };

        if event.event_type == "item.completed"
            && item.item_type == "agent_message"
            && let Some(text) = item.text
        {
            if !flattened.is_empty() {
                flattened.push('\n');
            }
            flattened.push_str(&text);
        }
    }

    if saw_json_line {
        Ok(Some(flattened))
    } else {
        Ok(None)
    }
}

/// Build an OutputSection without content (for streaming two-pass approach).
fn build_section_no_content(id: &str, content_start: usize, content_end: usize) -> OutputSection {
    let safe_id = sanitize_section_id(id);
    let line_start = content_start + 1;
    let line_end = if content_end < content_start {
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
        token_estimate: 0,
        file_path: Some(format!("{safe_id}.md")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_output_log(contents: &str) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("output.log"), contents).unwrap();
        tmp
    }

    #[test]
    fn persist_structured_output_from_file_extracts_summary_from_json_transcript() {
        let tmp = write_output_log(
            r#"{"type":"item.completed","item":{"id":"item_1","type":"agent_message","text":"<!-- CSA:SECTION:summary -->\nPASS\n<!-- CSA:SECTION:summary:END -->"}}"#,
        );

        let index = persist_structured_output_from_file(tmp.path(), &tmp.path().join("output.log"))
            .unwrap();

        assert_eq!(index.sections.len(), 1);
        assert_eq!(index.sections[0].id, "summary");
        assert_eq!(
            fs::read_to_string(tmp.path().join("output/summary.md")).unwrap(),
            "PASS"
        );
    }

    #[test]
    fn persist_structured_output_from_file_extracts_multiple_sections_from_json_transcript() {
        let tmp = write_output_log(concat!(
            r#"{"type":"item.completed","item":{"id":"item_1","type":"agent_message","text":"<!-- CSA:SECTION:summary -->\nPASS\n<!-- CSA:SECTION:summary:END -->"}}"#,
            "\n",
            r#"{"type":"item.completed","item":{"id":"item_2","type":"agent_message","text":"<!-- CSA:SECTION:details -->\nFound issue details\n<!-- CSA:SECTION:details:END -->"}}"#,
        ));

        let index = persist_structured_output_from_file(tmp.path(), &tmp.path().join("output.log"))
            .unwrap();

        assert_eq!(index.sections.len(), 2);
        assert!(tmp.path().join("output/summary.md").exists());
        assert!(tmp.path().join("output/details.md").exists());
        assert_eq!(
            fs::read_to_string(tmp.path().join("output/summary.md")).unwrap(),
            "PASS"
        );
        assert_eq!(
            fs::read_to_string(tmp.path().join("output/details.md")).unwrap(),
            "Found issue details"
        );
    }

    #[test]
    fn persist_structured_output_from_file_keeps_plain_text_path() {
        let tmp = write_output_log(
            "<!-- CSA:SECTION:summary -->\nPASS\n<!-- CSA:SECTION:summary:END -->\n",
        );

        let index = persist_structured_output_from_file(tmp.path(), &tmp.path().join("output.log"))
            .unwrap();

        assert_eq!(index.sections.len(), 1);
        assert_eq!(index.sections[0].id, "summary");
        assert_eq!(
            fs::read_to_string(tmp.path().join("output/summary.md")).unwrap(),
            "PASS"
        );
    }

    #[test]
    fn persist_structured_output_from_file_falls_back_on_malformed_json_like_input() {
        let tmp = write_output_log(
            "{not valid json\n<!-- CSA:SECTION:summary -->\nPASS\n<!-- CSA:SECTION:summary:END -->\n",
        );

        let index = persist_structured_output_from_file(tmp.path(), &tmp.path().join("output.log"))
            .unwrap();

        assert_eq!(index.sections.len(), 1);
        assert_eq!(index.sections[0].id, "summary");
        assert_eq!(
            fs::read_to_string(tmp.path().join("output/summary.md")).unwrap(),
            "PASS"
        );
    }
}
