use anyhow::Result;
use csa_session::state::ReviewSessionMeta;
use csa_session::{SessionResultView, TokenUsage};
use std::fs;
use std::path::Path;

use super::{StructuredOutputOpts, TranscriptSummary};

pub(super) fn display_result_json(
    result: &SessionResultView,
    transcript_summary: Option<&TranscriptSummary>,
    review_meta: Option<&ReviewSessionMeta>,
    token_usage: Option<&TokenUsage>,
) -> Result<()> {
    let payload = build_result_json_payload(result, transcript_summary, review_meta, token_usage)?;
    println!("{}", serde_json::to_string_pretty(&payload)?);
    Ok(())
}

pub(super) fn display_result_text(
    session_id: &str,
    session_dir: &Path,
    result: &SessionResultView,
    transcript_summary: Option<&TranscriptSummary>,
    review_meta: Option<&ReviewSessionMeta>,
    token_usage: Option<&TokenUsage>,
) {
    let envelope = &result.envelope;
    println!("Session: {session_id}");
    println!("Status:  {}", envelope.status);
    println!("Exit:    {}", envelope.exit_code);
    println!("Tool:    {}", envelope.tool);
    println!("Started: {}", envelope.started_at);
    println!("Ended:   {}", envelope.completed_at);
    if let Some(summary) =
        crate::session_summary_text::human_session_summary(session_dir, &envelope.summary)
    {
        let summary = crate::session_summary_text::enrich_review_summary(session_dir, &summary);
        println!("Summary: {summary}");
    }
    if !envelope.artifacts.is_empty() {
        println!("Artifacts:");
        for a in &envelope.artifacts {
            println!("  - {a}");
        }
    }
    display_sidecar("Manager Sidecar", result.manager_sidecar.as_ref());
    display_sidecar("Legacy Sidecar", result.legacy_sidecar.as_ref());
    if let Some(meta) = review_meta {
        println!("Review Iterations: {}", meta.review_iterations);
    }
    if let Some(usage) = token_usage {
        print_token_usage(usage);
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

/// Load total_token_usage from a session's state.toml on disk.
///
/// Returns None on any parse/read failure or when the field is absent.
/// Reading directly avoids the project-root coupling of `load_session`,
/// which lets cross-project sessions render their token totals too.
pub(super) fn load_total_token_usage(session_dir: &Path) -> Option<TokenUsage> {
    let state_path = session_dir.join("state.toml");
    let content = fs::read_to_string(&state_path).ok()?;
    let value: toml::Value = toml::from_str(&content).ok()?;
    let usage_table = value.get("total_token_usage")?;
    usage_table.clone().try_into::<TokenUsage>().ok()
}

fn print_token_usage(usage: &TokenUsage) {
    for line in render_token_usage_lines(usage) {
        println!("{line}");
    }
}

pub(super) fn render_token_usage_lines(usage: &TokenUsage) -> Vec<String> {
    let any_field = usage.input_tokens.is_some()
        || usage.output_tokens.is_some()
        || usage.total_tokens.is_some()
        || usage.estimated_cost_usd.is_some()
        || usage.cache_read_input_tokens.is_some();
    if !any_field {
        return Vec::new();
    }

    let mut lines = vec!["Tokens:".to_string()];
    if let Some(v) = usage.input_tokens {
        lines.push(format!("  Input:  {} tokens", format_thousands(v)));
    }
    if let Some(v) = usage.output_tokens {
        lines.push(format!("  Output: {} tokens", format_thousands(v)));
    }
    if let Some(v) = display_total_tokens(usage) {
        lines.push(format!("  Total:  {} tokens", format_thousands(v)));
    }
    if let Some(v) = usage.cache_read_input_tokens {
        if let Some(ratio) = usage.cache_hit_ratio() {
            lines.push(format!(
                "  Cache read: {} tokens ({:.0}% hit rate)",
                format_thousands(v),
                ratio * 100.0
            ));
        } else {
            lines.push(format!("  Cache read: {} tokens", format_thousands(v)));
        }
    }
    if let Some(cost) = usage.estimated_cost_usd {
        lines.push(format!("  Cost:   ${cost:.4}"));
    }
    lines
}

fn display_total_tokens(usage: &TokenUsage) -> Option<u64> {
    match (usage.input_tokens, usage.output_tokens) {
        (Some(input), Some(output)) => input.checked_add(output),
        _ => usage.total_tokens,
    }
}

fn format_thousands(n: u64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let len = bytes.len();
    let mut out = String::with_capacity(len + len / 3);
    for (idx, byte) in bytes.iter().enumerate() {
        if idx > 0 && (len - idx).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*byte as char);
    }
    out
}

pub(super) fn build_result_json_payload(
    result: &SessionResultView,
    transcript_summary: Option<&TranscriptSummary>,
    review_meta: Option<&ReviewSessionMeta>,
    token_usage: Option<&TokenUsage>,
) -> Result<serde_json::Value> {
    let mut payload = serde_json::to_value(&result.envelope)?;
    if let Some(sidecar) = result
        .manager_sidecar
        .as_ref()
        .and_then(redact_result_sidecar_for_json)
    {
        payload["manager_sidecar"] = serde_json::to_value(sidecar)?;
    }
    if let Some(sidecar) = result
        .legacy_sidecar
        .as_ref()
        .and_then(redact_result_sidecar_for_json)
    {
        payload["legacy_sidecar"] = serde_json::to_value(sidecar)?;
    }
    if let Some(summary) = transcript_summary {
        payload["transcript_summary"] = serde_json::json!({
            "event_count": summary.event_count,
            "size_bytes": summary.size_bytes,
            "first_timestamp": summary.first_timestamp,
            "last_timestamp": summary.last_timestamp,
        });
    }
    if let Some(meta) = review_meta {
        payload["review_meta"] = serde_json::to_value(meta)?;
    }
    if let Some(usage) = token_usage {
        let usage = normalized_token_usage_for_output(usage);
        let mut value = serde_json::to_value(&usage)?;
        if let Some(ratio) = usage.cache_hit_ratio() {
            value["cache_hit_ratio"] = serde_json::json!(ratio);
        }
        payload["total_token_usage"] = value;
    }
    Ok(payload)
}

fn normalized_token_usage_for_output(usage: &TokenUsage) -> TokenUsage {
    let mut normalized = usage.clone();
    normalized.total_tokens = display_total_tokens(usage);
    normalized
}

fn display_sidecar(label: &str, sidecar: Option<&toml::Value>) {
    if let Some(rendered) = sidecar.and_then(render_result_sidecar_for_text) {
        println!("{label}:");
        print_rendered_sidecar(&rendered, 2);
    }
}

pub(super) fn render_result_sidecar_for_text(sidecar: &toml::Value) -> Option<String> {
    match csa_session::render_redacted_result_sidecar(sidecar) {
        Ok(rendered) if rendered.trim().is_empty() => None,
        Ok(rendered) => Some(rendered),
        Err(err) => Some(format!("<failed to render TOML sidecar: {err}>")),
    }
}

fn redact_result_sidecar_for_json(sidecar: &toml::Value) -> Option<toml::Value> {
    match csa_session::redact_result_sidecar_value(sidecar) {
        Ok(toml::Value::Table(table)) if table.is_empty() => None,
        Ok(value) => Some(value),
        Err(_) => Some(toml::Value::String(
            "<failed to render TOML sidecar>".to_string(),
        )),
    }
}

fn print_rendered_sidecar(rendered: &str, indent: usize) {
    let padding = " ".repeat(indent);
    for line in rendered.lines() {
        println!("{padding}{line}");
    }
}

const FALLBACK_LINES: usize = 20;

/// Display structured output sections based on the requested mode.
pub(super) fn display_structured_output(
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
pub(super) fn display_summary_section(
    session_dir: &Path,
    session_id: &str,
    json: bool,
) -> Result<()> {
    let (section_id, content) = match csa_session::read_section(session_dir, "summary")? {
        Some(content) => ("summary", content),
        None => match csa_session::read_section(session_dir, "full")? {
            Some(content) => ("full", content),
            None => return display_summary_fallback(session_dir, session_id, json),
        },
    };

    if json {
        let payload = serde_json::json!({
            "section": section_id,
            "content": content,
            "tokens": csa_session::estimate_tokens(&content),
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else if section_id == "full" {
        print_truncated_content(&content, FALLBACK_LINES);
    } else {
        println!("{content}");
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
    eprintln!("No output found for session '{session_id}'");
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
pub(super) fn display_single_section(
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
                println!("{content}");
            }
        }
        None => match csa_session::load_output_index(session_dir)? {
            Some(index) => {
                let available: Vec<&str> = index.sections.iter().map(|s| s.id.as_str()).collect();
                anyhow::bail!(
                    "Section '{}' not found in session '{}'. Available sections: {}",
                    section_id,
                    session_id,
                    available.join(", ")
                );
            }
            None => {
                anyhow::bail!(
                    "No structured output for session '{session_id}'. Run without --section to see raw result."
                );
            }
        },
    }
    Ok(())
}

/// Show all sections in index order.
pub(super) fn display_all_sections(session_dir: &Path, session_id: &str, json: bool) -> Result<()> {
    let sections = csa_session::read_all_sections(session_dir)?;
    if sections.is_empty() {
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
                    print!("{content}");
                }
                return Ok(());
            }
        }
        eprintln!("No output found for session '{session_id}'");
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
            println!("{content}");
        }
    }
    Ok(())
}
