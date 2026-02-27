use anyhow::Result;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::Path;

use csa_session::SessionResult;

use crate::session_cmds::{format_file_size, resolve_session_prefix_with_fallback};

#[derive(Debug, Clone)]
struct TranscriptSummary {
    event_count: u64,
    size_bytes: u64,
    first_timestamp: Option<String>,
    last_timestamp: Option<String>,
}

fn load_transcript_summary(session_dir: &Path) -> Result<Option<TranscriptSummary>> {
    let transcript_path = session_dir.join("output").join("acp-events.jsonl");
    if !transcript_path.is_file() {
        return Ok(None);
    }

    let size_bytes = fs::metadata(&transcript_path)?.len();
    let file = File::open(&transcript_path)?;
    let reader = BufReader::new(file);

    let mut event_count = 0u64;
    let mut first_timestamp: Option<String> = None;
    let mut last_timestamp: Option<String> = None;

    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        event_count = event_count.saturating_add(1);
        if let Some(ts) = extract_transcript_timestamp(&line) {
            if first_timestamp.is_none() {
                first_timestamp = Some(ts.clone());
            }
            last_timestamp = Some(ts);
        }
    }

    Ok(Some(TranscriptSummary {
        event_count,
        size_bytes,
        first_timestamp,
        last_timestamp,
    }))
}

fn extract_transcript_timestamp(line: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(line)
        .ok()?
        .get("ts")?
        .as_str()
        .map(ToString::to_string)
}

/// Options for structured output display in `csa session result`.
#[derive(Debug, Default)]
pub(crate) struct StructuredOutputOpts {
    pub summary: bool,
    pub section: Option<String>,
    pub full: bool,
}

impl StructuredOutputOpts {
    fn is_active(&self) -> bool {
        self.summary || self.section.is_some() || self.full
    }
}

pub(crate) fn handle_session_result(
    session: String,
    json: bool,
    cd: Option<String>,
    structured: StructuredOutputOpts,
) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let resolved = resolve_session_prefix_with_fallback(&project_root, &session)?;
    let resolved_id = resolved.session_id;
    let session_dir = csa_session::get_session_dir(&project_root, &resolved_id)?;

    // If structured output flags are active, handle them and return early
    if structured.is_active() {
        return display_structured_output(&session_dir, &resolved_id, &structured, json);
    }

    let transcript_summary = match load_transcript_summary(&session_dir) {
        Ok(summary) => summary,
        Err(err) => {
            tracing::warn!(
                session_id = %resolved_id,
                path = %session_dir.display(),
                error = %err,
                "Failed to load transcript summary; continuing without transcript metadata"
            );
            None
        }
    };
    match csa_session::load_result(&project_root, &resolved_id)? {
        Some(result) => {
            if json {
                display_result_json(&result, transcript_summary.as_ref())?;
            } else {
                display_result_text(&resolved_id, &result, transcript_summary.as_ref());
            }
        }
        None => {
            eprintln!("No result found for session '{}'", resolved_id);
        }
    }
    Ok(())
}

fn display_result_json(
    result: &SessionResult,
    transcript_summary: Option<&TranscriptSummary>,
) -> Result<()> {
    let mut payload = serde_json::to_value(result)?;
    if let Some(summary) = transcript_summary {
        payload["transcript_summary"] = serde_json::json!({
            "event_count": summary.event_count,
            "size_bytes": summary.size_bytes,
            "first_timestamp": summary.first_timestamp,
            "last_timestamp": summary.last_timestamp,
        });
    }
    println!("{}", serde_json::to_string_pretty(&payload)?);
    Ok(())
}

fn display_result_text(
    session_id: &str,
    result: &SessionResult,
    transcript_summary: Option<&TranscriptSummary>,
) {
    println!("Session: {}", session_id);
    println!("Status:  {}", result.status);
    println!("Exit:    {}", result.exit_code);
    println!("Tool:    {}", result.tool);
    println!("Started: {}", result.started_at);
    println!("Ended:   {}", result.completed_at);
    println!("Summary: {}", result.summary);
    if !result.artifacts.is_empty() {
        println!("Artifacts:");
        for a in &result.artifacts {
            println!("  - {}", a);
        }
    }
    if let Some(summary) = transcript_summary {
        println!("Transcript:");
        println!("  Events: {}", summary.event_count);
        println!("  Size:   {} bytes", summary.size_bytes);
        println!(
            "  First:  {}",
            summary.first_timestamp.as_deref().unwrap_or("-")
        );
        println!(
            "  Last:   {}",
            summary.last_timestamp.as_deref().unwrap_or("-")
        );
    }
}

const FALLBACK_LINES: usize = 20;

/// Display structured output sections based on the requested mode.
fn display_structured_output(
    session_dir: &Path,
    session_id: &str,
    opts: &StructuredOutputOpts,
    json: bool,
) -> Result<()> {
    if opts.summary {
        return display_summary_section(session_dir, session_id, json);
    }

    if let Some(ref section_id) = opts.section {
        return display_single_section(session_dir, session_id, section_id, json);
    }

    if opts.full {
        return display_all_sections(session_dir, session_id, json);
    }

    Ok(())
}

/// Show only the summary section, with fallback to first N lines of output.log.
pub(crate) fn display_summary_section(
    session_dir: &Path,
    session_id: &str,
    json: bool,
) -> Result<()> {
    // Try reading "summary" section first
    let (section_id, content) = match csa_session::read_section(session_dir, "summary")? {
        Some(content) => ("summary", content),
        None => {
            // If there's a "full" section, use that as fallback
            match csa_session::read_section(session_dir, "full")? {
                Some(content) => ("full", content),
                None => {
                    // Final fallback: first N lines of output.log
                    return display_summary_fallback(session_dir, session_id, json);
                }
            }
        }
    };

    if json {
        let payload = serde_json::json!({
            "section": section_id,
            "content": content,
            "tokens": csa_session::estimate_tokens(&content),
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        let is_full_fallback = section_id == "full";
        if is_full_fallback {
            print_truncated_content(&content, FALLBACK_LINES);
        } else {
            println!("{}", content);
        }
    }
    Ok(())
}

fn display_summary_fallback(session_dir: &Path, session_id: &str, json: bool) -> Result<()> {
    let output_log = session_dir.join("output.log");
    if output_log.is_file() {
        let content = fs::read_to_string(&output_log)?;
        if !content.is_empty() {
            if json {
                let payload = serde_json::json!({
                    "section": "summary",
                    "source": "output.log",
                    "content": content.lines().take(FALLBACK_LINES).collect::<Vec<_>>().join("\n"),
                    "truncated": content.lines().count() > FALLBACK_LINES,
                });
                println!("{}", serde_json::to_string_pretty(&payload)?);
            } else {
                print_truncated_content(&content, FALLBACK_LINES);
            }
            return Ok(());
        }
    }
    eprintln!("No output found for session '{}'", session_id);
    Ok(())
}

fn print_truncated_content(content: &str, max_lines: usize) {
    let lines: Vec<&str> = content.lines().take(max_lines).collect();
    println!("{}", lines.join("\n"));
    if content.lines().count() > max_lines {
        eprintln!(
            "... ({} more lines, use --full to see all)",
            content.lines().count() - max_lines
        );
    }
}

/// Show a single section by ID.
pub(crate) fn display_single_section(
    session_dir: &Path,
    session_id: &str,
    section_id: &str,
    json: bool,
) -> Result<()> {
    match csa_session::read_section(session_dir, section_id)? {
        Some(content) => {
            if json {
                let payload = serde_json::json!({
                    "section": section_id,
                    "content": content,
                    "tokens": csa_session::estimate_tokens(&content),
                });
                println!("{}", serde_json::to_string_pretty(&payload)?);
            } else {
                println!("{}", content);
            }
        }
        None => {
            // Check if index exists to give a better error
            match csa_session::load_output_index(session_dir)? {
                Some(index) => {
                    let available: Vec<&str> =
                        index.sections.iter().map(|s| s.id.as_str()).collect();
                    anyhow::bail!(
                        "Section '{}' not found in session '{}'. Available sections: {}",
                        section_id,
                        session_id,
                        available.join(", ")
                    );
                }
                None => {
                    anyhow::bail!(
                        "No structured output for session '{}'. Run without --section to see raw result.",
                        session_id
                    );
                }
            }
        }
    }
    Ok(())
}

/// Show all sections in index order.
pub(crate) fn display_all_sections(
    session_dir: &Path,
    session_id: &str,
    json: bool,
) -> Result<()> {
    let sections = csa_session::read_all_sections(session_dir)?;
    if sections.is_empty() {
        // Fallback: show full output.log
        let output_log = session_dir.join("output.log");
        if output_log.is_file() {
            let content = fs::read_to_string(&output_log)?;
            if !content.is_empty() {
                if json {
                    let payload = serde_json::json!({
                        "sections": [{
                            "section": "full",
                            "content": content,
                            "tokens": csa_session::estimate_tokens(&content),
                        }]
                    });
                    println!("{}", serde_json::to_string_pretty(&payload)?);
                } else {
                    print!("{}", content);
                }
                return Ok(());
            }
        }
        eprintln!("No output found for session '{}'", session_id);
        return Ok(());
    }

    if json {
        let json_sections: Vec<serde_json::Value> = sections
            .iter()
            .map(|(section, content)| {
                serde_json::json!({
                    "section": section.id,
                    "title": section.title,
                    "content": content,
                    "tokens": section.token_estimate,
                })
            })
            .collect();
        let payload = serde_json::json!({ "sections": json_sections });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        for (i, (section, content)) in sections.iter().enumerate() {
            if i > 0 {
                println!();
            }
            println!("=== {} ({}) ===", section.title, section.id);
            println!("{}", content);
        }
    }
    Ok(())
}

pub(crate) fn handle_session_artifacts(session: String, cd: Option<String>) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let resolved = resolve_session_prefix_with_fallback(&project_root, &session)?;
    let resolved_id = resolved.session_id;
    let session_dir = csa_session::get_session_dir(&project_root, &resolved_id)?;
    let output_dir = session_dir.join("output");

    // Show structured output index if available
    if let Some(index) = csa_session::load_output_index(&session_dir)? {
        println!(
            "Structured output ({} sections, ~{} tokens):",
            index.sections.len(),
            index.total_tokens
        );
        for section in &index.sections {
            let size_str = if let Some(ref fp) = section.file_path {
                let path = output_dir.join(fp);
                match fs::metadata(&path) {
                    Ok(meta) => format_file_size(meta.len()),
                    Err(_) => "missing".to_string(),
                }
            } else {
                "-".to_string()
            };
            println!(
                "  {:<20}  {:<30}  ~{}tok  {}",
                section.id, section.title, section.token_estimate, size_str
            );
        }
        println!();
    }

    // List all files in output/ with sizes
    if output_dir.is_dir() {
        let mut entries: Vec<_> = fs::read_dir(&output_dir)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_file())
            .collect();
        entries.sort_by_key(|e| e.file_name());

        if entries.is_empty() {
            eprintln!("No artifacts for session '{}'", resolved_id);
        } else {
            println!("Files:");
            for entry in &entries {
                let path = entry.path();
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                println!("  {:<40}  {}", name, format_file_size(size));
            }
        }
    } else {
        eprintln!("No artifacts for session '{}'", resolved_id);
    }

    Ok(())
}

/// Token savings measurement for structured output.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct TokenMeasurement {
    pub session_id: String,
    pub total_tokens: usize,
    pub summary_tokens: usize,
    pub savings_tokens: usize,
    pub savings_percent: f64,
    pub section_count: usize,
    pub section_names: Vec<String>,
    pub is_structured: bool,
}

pub(crate) fn handle_session_measure(
    session: String,
    json: bool,
    cd: Option<String>,
) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let resolved = resolve_session_prefix_with_fallback(&project_root, &session)?;
    let resolved_id = resolved.session_id;
    let session_dir = csa_session::get_session_dir(&project_root, &resolved_id)?;

    let measurement = compute_token_measurement(&session_dir, &resolved_id)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&measurement)?);
    } else {
        let short_id = &resolved_id[..11.min(resolved_id.len())];
        println!("Session: {}", short_id);
        println!(
            "Total output: {} tokens",
            format_number(measurement.total_tokens)
        );
        println!(
            "Summary only: {} tokens",
            format_number(measurement.summary_tokens)
        );
        if measurement.is_structured && measurement.total_tokens > 0 {
            println!(
                "Savings: {:.1}% ({} tokens saved)",
                measurement.savings_percent,
                format_number(measurement.savings_tokens)
            );
            println!(
                "Sections: {} ({})",
                measurement.section_count,
                measurement.section_names.join(", ")
            );
        } else {
            println!("Savings: N/A (unstructured output)");
        }
    }

    Ok(())
}

pub(crate) fn compute_token_measurement(
    session_dir: &Path,
    session_id: &str,
) -> Result<TokenMeasurement> {
    // Try loading the structured output index
    let index = csa_session::load_output_index(session_dir)?;

    if let Some(index) = index {
        let total_tokens = index.total_tokens;
        let section_names: Vec<String> = index.sections.iter().map(|s| s.id.clone()).collect();
        let section_count = index.sections.len();

        // Find summary section tokens (first section named "summary", or first section)
        let summary_tokens = index
            .sections
            .iter()
            .find(|s| s.id == "summary")
            .map(|s| s.token_estimate)
            .unwrap_or_else(|| {
                index
                    .sections
                    .first()
                    .map(|s| s.token_estimate)
                    .unwrap_or(0)
            });

        // "full" section means unstructured (parser wraps entire output as "full")
        let is_structured = section_count > 1 || (section_count == 1 && section_names[0] != "full");

        let savings_tokens = total_tokens.saturating_sub(summary_tokens);
        let savings_percent = if total_tokens > 0 {
            (1.0 - summary_tokens as f64 / total_tokens as f64) * 100.0
        } else {
            0.0
        };

        Ok(TokenMeasurement {
            session_id: session_id.to_string(),
            total_tokens,
            summary_tokens,
            savings_tokens,
            savings_percent,
            section_count,
            section_names,
            is_structured,
        })
    } else {
        // No index — try computing from output.log directly
        let output_log = session_dir.join("output.log");
        let total_tokens = if output_log.is_file() {
            let content = fs::read_to_string(&output_log)?;
            csa_session::estimate_tokens(&content)
        } else {
            0
        };

        Ok(TokenMeasurement {
            session_id: session_id.to_string(),
            total_tokens,
            summary_tokens: total_tokens,
            savings_tokens: 0,
            savings_percent: 0.0,
            section_count: 0,
            section_names: vec![],
            is_structured: false,
        })
    }
}

/// Format a number with commas for readability.
pub(crate) fn format_number(n: usize) -> String {
    let s = n.to_string();
    let chars: Vec<char> = s.chars().rev().collect();
    let chunks: Vec<String> = chars
        .chunks(3)
        .map(|chunk| chunk.iter().collect::<String>())
        .collect();
    chunks.join(",").chars().rev().collect()
}

#[cfg(test)]
#[path = "session_cmds_result_tests.rs"]
mod tests;
