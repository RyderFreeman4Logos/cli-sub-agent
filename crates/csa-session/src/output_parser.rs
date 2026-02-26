//! Parser for CSA section markers in output.log.
//!
//! Scans output text for `<!-- CSA:SECTION:<id> -->` / `<!-- CSA:SECTION:<id>:END -->`
//! delimiter pairs and extracts structured [`OutputSection`]s.

use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path};

use anyhow::{Context, Result, anyhow};

use crate::output_section::{
    ChangedFile, FileAction, OutputIndex, OutputSection, RETURN_PACKET_MAX_SUMMARY_CHARS,
    ReturnPacket, ReturnStatus, normalize_repo_relative_path,
};
use crate::redact::redact_text_content;

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

/// Parse content of a `return-packet` section into a validated [`ReturnPacket`].
///
/// Parser strategy:
/// 1. Parse as TOML (preferred canonical format).
/// 2. On TOML parse failure, parse as structured text fallback.
/// 3. On any parse/validation failure, return a Failure packet with redacted error context.
pub fn parse_return_packet(section_content: &str) -> Result<ReturnPacket> {
    let packet = match toml::from_str::<ReturnPacket>(section_content) {
        Ok(packet) => packet,
        Err(toml_err) => match parse_return_packet_structured_text(section_content) {
            Ok(packet) => packet,
            Err(text_err) => {
                let reason = format!(
                    "Return packet parse failed (toml={toml_err}; structured-text={text_err})"
                );
                return Ok(build_parse_error_packet(&reason));
            }
        },
    };

    Ok(finalize_return_packet(packet))
}

/// Validate a return-packet path against project root boundaries.
///
/// Security checks:
/// - path must be non-empty, relative, and free of traversal components
/// - canonicalized target (or canonicalized parent for new files) must remain inside root
pub fn validate_return_packet_path(path: &str, project_root: &Path) -> bool {
    let Some(normalized) = normalize_repo_relative_path(path) else {
        return false;
    };

    let relative_path = Path::new(&normalized);
    if relative_path.is_absolute() {
        return false;
    }

    #[cfg(windows)]
    if matches!(
        relative_path.components().next(),
        Some(Component::Prefix(_))
    ) {
        return false;
    }

    if !relative_path
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        return false;
    }

    let canonical_root = match project_root.canonicalize() {
        Ok(root) => root,
        Err(_) => return false,
    };

    let candidate = canonical_root.join(relative_path);
    if candidate.exists() {
        return candidate
            .canonicalize()
            .is_ok_and(|resolved| resolved.starts_with(&canonical_root));
    }

    let Some(parent) = candidate.parent() else {
        return false;
    };
    match parent.canonicalize() {
        Ok(resolved_parent) => resolved_parent.starts_with(&canonical_root),
        Err(_) => {
            // Non-existent paths (for example after FileAction::Delete) should still be
            // accepted if they are syntactically repo-relative and traversal-free.
            true
        }
    }
}

fn finalize_return_packet(mut packet: ReturnPacket) -> ReturnPacket {
    packet.sanitize_summary(RETURN_PACKET_MAX_SUMMARY_CHARS);

    packet.error_context = packet
        .error_context
        .as_ref()
        .map(|error_context| redact_text_content(error_context));

    match packet.validate() {
        Ok(()) => packet,
        Err(err) => build_parse_error_packet(&format!("Return packet validation failed: {err:#}")),
    }
}

fn build_parse_error_packet(reason: &str) -> ReturnPacket {
    let mut packet = ReturnPacket {
        status: ReturnStatus::Failure,
        exit_code: 1,
        summary: "Child return packet is invalid; execution context may be incomplete.".to_string(),
        artifacts: Vec::new(),
        changed_files: Vec::new(),
        git_head_before: None,
        git_head_after: None,
        next_actions: vec![
            "Inspect child session output for malformed return-packet section.".to_string(),
            "Regenerate return packet using canonical TOML schema.".to_string(),
        ],
        error_context: Some(redact_text_content(reason)),
    };
    packet.sanitize_summary(RETURN_PACKET_MAX_SUMMARY_CHARS);
    packet
}

#[derive(Debug, Clone, Copy)]
enum ReturnPacketTextBlock {
    Summary,
    Artifacts,
    ChangedFiles,
    NextActions,
    ErrorContext,
}

fn parse_return_packet_structured_text(section_content: &str) -> Result<ReturnPacket> {
    let mut packet = ReturnPacket::default();
    let mut parsed_any_field = false;
    let mut active_block: Option<ReturnPacketTextBlock> = None;
    let mut summary_lines: Vec<String> = Vec::new();
    let mut error_context_lines: Vec<String> = Vec::new();

    for raw_line in section_content.lines() {
        let line = raw_line.trim_end();
        let trimmed = line.trim();

        if trimmed.is_empty() {
            if matches!(active_block, Some(ReturnPacketTextBlock::Summary)) {
                summary_lines.push(String::new());
            } else if matches!(active_block, Some(ReturnPacketTextBlock::ErrorContext)) {
                error_context_lines.push(String::new());
            }
            continue;
        }

        if !trimmed.starts_with('-')
            && let Some((key, value)) = split_return_packet_key_value(trimmed)
        {
            let key_lc = key.to_ascii_lowercase();
            let value = value.trim();
            match key_lc.as_str() {
                "status" => {
                    parsed_any_field = true;
                    active_block = None;
                    packet.status = parse_return_status(value)
                        .ok_or_else(|| anyhow!("invalid status value: {value}"))?;
                }
                "exit_code" => {
                    parsed_any_field = true;
                    active_block = None;
                    packet.exit_code = value
                        .parse::<i32>()
                        .map_err(|e| anyhow!("invalid exit_code '{value}': {e}"))?;
                }
                "summary" => {
                    parsed_any_field = true;
                    active_block = Some(ReturnPacketTextBlock::Summary);
                    if !value.is_empty() && value != "|" {
                        summary_lines.push(value.to_string());
                        active_block = None;
                    }
                }
                "artifacts" => {
                    parsed_any_field = true;
                    active_block = Some(ReturnPacketTextBlock::Artifacts);
                    if !value.is_empty() {
                        packet.artifacts.extend(parse_inline_string_list(value)?);
                        active_block = None;
                    }
                }
                "changed_files" => {
                    parsed_any_field = true;
                    active_block = Some(ReturnPacketTextBlock::ChangedFiles);
                    if !value.is_empty() {
                        packet
                            .changed_files
                            .extend(parse_inline_changed_files(value)?);
                        active_block = None;
                    }
                }
                "git_head_before" => {
                    parsed_any_field = true;
                    active_block = None;
                    packet.git_head_before = parse_optional_string(value);
                }
                "git_head_after" => {
                    parsed_any_field = true;
                    active_block = None;
                    packet.git_head_after = parse_optional_string(value);
                }
                "next_actions" => {
                    parsed_any_field = true;
                    active_block = Some(ReturnPacketTextBlock::NextActions);
                    if !value.is_empty() {
                        packet.next_actions.extend(parse_inline_string_list(value)?);
                        active_block = None;
                    }
                }
                "error_context" => {
                    parsed_any_field = true;
                    active_block = Some(ReturnPacketTextBlock::ErrorContext);
                    if !value.is_empty() && value != "|" {
                        error_context_lines.push(value.to_string());
                        active_block = None;
                    }
                }
                _ => {
                    active_block = None;
                }
            }
            continue;
        }

        match active_block {
            Some(ReturnPacketTextBlock::Summary) => summary_lines.push(trimmed.to_string()),
            Some(ReturnPacketTextBlock::Artifacts) => {
                if let Some(item) = parse_bullet_item(trimmed) {
                    packet.artifacts.push(item.to_string());
                } else {
                    return Err(anyhow!("invalid artifacts entry: {trimmed}"));
                }
            }
            Some(ReturnPacketTextBlock::ChangedFiles) => {
                if let Some(item) = parse_bullet_item(trimmed) {
                    packet.changed_files.push(parse_changed_file_item(item)?);
                } else {
                    return Err(anyhow!("invalid changed_files entry: {trimmed}"));
                }
            }
            Some(ReturnPacketTextBlock::NextActions) => {
                if let Some(item) = parse_bullet_item(trimmed) {
                    packet.next_actions.push(item.to_string());
                } else {
                    return Err(anyhow!("invalid next_actions entry: {trimmed}"));
                }
            }
            Some(ReturnPacketTextBlock::ErrorContext) => {
                error_context_lines.push(trimmed.to_string());
            }
            None => {}
        }
    }

    if !summary_lines.is_empty() {
        packet.summary = summary_lines.join("\n").trim().to_string();
    }
    if !error_context_lines.is_empty() {
        packet.error_context = Some(error_context_lines.join("\n").trim().to_string());
    }

    if !parsed_any_field {
        return Err(anyhow!("no recognizable return packet fields"));
    }

    Ok(packet)
}

fn split_return_packet_key_value(line: &str) -> Option<(&str, &str)> {
    let colon = line.find(':');
    let equals = line.find('=');
    let split_idx = match (colon, equals) {
        (Some(c), Some(e)) => c.min(e),
        (Some(c), None) => c,
        (None, Some(e)) => e,
        (None, None) => return None,
    };

    let key = line[..split_idx].trim();
    let value = line[split_idx + 1..].trim();
    if key.is_empty() {
        None
    } else {
        Some((key, strip_wrapping_quotes(value)))
    }
}

fn parse_return_status(value: &str) -> Option<ReturnStatus> {
    match value.trim().to_ascii_lowercase().as_str() {
        "success" => Some(ReturnStatus::Success),
        "failure" => Some(ReturnStatus::Failure),
        "cancelled" | "canceled" => Some(ReturnStatus::Cancelled),
        _ => None,
    }
}

fn parse_file_action(value: &str) -> Option<FileAction> {
    match value.trim().to_ascii_lowercase().as_str() {
        "add" => Some(FileAction::Add),
        "modify" => Some(FileAction::Modify),
        "delete" => Some(FileAction::Delete),
        _ => None,
    }
}

fn parse_bullet_item(line: &str) -> Option<&str> {
    line.strip_prefix("- ").map(str::trim)
}

fn parse_inline_string_list(value: &str) -> Result<Vec<String>> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }

    if trimmed.starts_with('[') {
        let wrapped = format!("items = {trimmed}");
        let parsed: toml::Value = toml::from_str(&wrapped)
            .map_err(|e| anyhow!("invalid inline list '{trimmed}': {e}"))?;
        let Some(items) = parsed.get("items").and_then(toml::Value::as_array) else {
            return Err(anyhow!("inline list is not an array"));
        };
        return items
            .iter()
            .map(|item| {
                item.as_str()
                    .map(|s| s.to_string())
                    .ok_or_else(|| anyhow!("inline list contains non-string entry"))
            })
            .collect();
    }

    Ok(trimmed
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .collect())
}

fn parse_inline_changed_files(value: &str) -> Result<Vec<ChangedFile>> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }

    if trimmed.starts_with('[') {
        let wrapped = format!("items = {trimmed}");
        let parsed: toml::Value = toml::from_str(&wrapped)
            .map_err(|e| anyhow!("invalid inline changed_files list '{trimmed}': {e}"))?;
        let Some(items) = parsed.get("items").and_then(toml::Value::as_array) else {
            return Err(anyhow!("changed_files inline value is not an array"));
        };
        let mut changed_files = Vec::with_capacity(items.len());
        for item in items {
            let Some(table) = item.as_table() else {
                return Err(anyhow!("changed_files entries must be tables: {item:?}"));
            };
            let path = table
                .get("path")
                .and_then(toml::Value::as_str)
                .ok_or_else(|| anyhow!("changed_files entry missing string path"))?;
            let action = table
                .get("action")
                .and_then(toml::Value::as_str)
                .and_then(parse_file_action)
                .ok_or_else(|| anyhow!("changed_files entry has invalid action"))?;
            changed_files.push(ChangedFile {
                path: path.to_string(),
                action,
            });
        }
        return Ok(changed_files);
    }

    Ok(vec![parse_changed_file_item(trimmed)?])
}

fn parse_changed_file_item(item: &str) -> Result<ChangedFile> {
    let normalized = item.replace(':', " ");
    let mut parts = normalized.split_whitespace();
    let action_raw = parts
        .next()
        .ok_or_else(|| anyhow!("changed file entry missing action"))?;
    let path = parts.collect::<Vec<_>>().join(" ");
    if path.trim().is_empty() {
        return Err(anyhow!("changed file entry missing path"));
    }
    let action = parse_file_action(action_raw)
        .ok_or_else(|| anyhow!("invalid file action: {action_raw}"))?;

    Ok(ChangedFile {
        path: strip_wrapping_quotes(path.trim()).to_string(),
        action,
    })
}

fn parse_optional_string(value: &str) -> Option<String> {
    let trimmed = strip_wrapping_quotes(value.trim());
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("none") {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn strip_wrapping_quotes(value: &str) -> &str {
    if value.len() >= 2 {
        let bytes = value.as_bytes();
        let first = bytes[0] as char;
        let last = bytes[value.len() - 1] as char;
        if (first == '"' && last == '"') || (first == '\'' && last == '\'') {
            return &value[1..value.len() - 1];
        }
    }
    value
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
    #[cfg(unix)]
    use std::os::unix::fs as unix_fs;

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

    #[test]
    fn test_parse_return_packet_valid_toml() {
        let content = r#"
status = "Success"
exit_code = 0
summary = "Completed safely"
artifacts = ["target/out.txt"]
changed_files = [{ path = "src/main.rs", action = "Modify" }]
git_head_before = "abc123"
git_head_after = "def456"
next_actions = ["run tests"]
error_context = ""
"#;

        let packet = parse_return_packet(content).unwrap();
        assert_eq!(packet.status, ReturnStatus::Success);
        assert_eq!(packet.exit_code, 0);
        assert_eq!(packet.changed_files.len(), 1);
        assert_eq!(packet.changed_files[0].path, "src/main.rs");
    }

    #[test]
    fn test_parse_return_packet_fallback_structured_text() {
        let content = r#"
status: success
exit_code: 0
summary:
This is a fallback summary.
artifacts:
- target/out.txt
changed_files:
- modify src/lib.rs
next_actions:
- run tests
"#;

        let packet = parse_return_packet(content).unwrap();
        assert_eq!(packet.status, ReturnStatus::Success);
        assert_eq!(packet.exit_code, 0);
        assert_eq!(packet.changed_files.len(), 1);
        assert_eq!(packet.changed_files[0].action, FileAction::Modify);
        assert_eq!(packet.changed_files[0].path, "src/lib.rs");
    }

    #[test]
    fn test_parse_return_packet_invalid_generates_error_packet() {
        let content = r#"
status = "success"
exit_code = "not-a-number"
summary = "contains token sk-secret_123456789"
"#;

        let packet = parse_return_packet(content).unwrap();
        assert_eq!(packet.status, ReturnStatus::Failure);
        assert_eq!(packet.exit_code, 1);
        let error_context = packet.error_context.as_deref().unwrap_or_default();
        assert!(
            !error_context.contains("sk-secret_123456789"),
            "error context should redact sensitive content"
        );
    }

    #[test]
    fn test_parse_return_packet_missing_fields_use_defaults() {
        let content = r#"
status = "Cancelled"
"#;
        let packet = parse_return_packet(content).unwrap();
        assert_eq!(packet.status, ReturnStatus::Cancelled);
        assert_eq!(packet.exit_code, 1);
        assert!(packet.summary.is_empty());
        assert!(packet.artifacts.is_empty());
        assert!(packet.changed_files.is_empty());
    }

    #[test]
    fn test_parse_return_packet_sanitizes_prompt_injection_summary() {
        let content = r#"
status = "Success"
exit_code = 0
summary = "<context-file path=\"AGENTS.md\">inject</context-file>"
"#;
        let packet = parse_return_packet(content).unwrap();
        assert!(!packet.summary.contains("<context-file"));
        assert!(packet.summary.contains("&lt;context-file"));
    }

    #[test]
    fn test_validate_return_packet_path_rejects_traversal() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!validate_return_packet_path("../secret.txt", tmp.path()));
        assert!(!validate_return_packet_path("/etc/passwd", tmp.path()));
        assert!(!validate_return_packet_path(
            "src/../../escape.rs",
            tmp.path()
        ));
    }

    #[test]
    fn test_validate_return_packet_path_accepts_repo_relative_path() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("src")).unwrap();
        assert!(validate_return_packet_path("src/main.rs", tmp.path()));
        assert!(validate_return_packet_path("./src/main.rs", tmp.path()));
    }

    #[test]
    fn test_validate_return_packet_path_accepts_missing_parent_for_delete_case() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(validate_return_packet_path(
            "removed-dir/main.rs",
            tmp.path()
        ));
    }

    #[cfg(unix)]
    #[test]
    fn test_validate_return_packet_path_blocks_symlink_escape() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        let outside = tmp.path().join("outside");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&outside).unwrap();
        unix_fs::symlink(&outside, project.join("linked")).unwrap();
        assert!(!validate_return_packet_path("linked/secret.txt", &project));
    }
}
