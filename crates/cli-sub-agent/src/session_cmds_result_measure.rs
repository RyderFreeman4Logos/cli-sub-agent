//! Token-savings measurement for `csa session measure`.

use anyhow::Result;
use std::fs;
use std::path::Path;

use crate::session_cmds::resolve_session_prefix_with_fallback;

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
        println!("Session: {short_id}");
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
    let index = csa_session::load_output_index(session_dir)?;

    if let Some(index) = index {
        let total_tokens = index.total_tokens;
        let section_names: Vec<String> = index.sections.iter().map(|s| s.id.clone()).collect();
        let section_count = index.sections.len();

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
