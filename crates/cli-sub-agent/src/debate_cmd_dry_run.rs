use std::path::Path;

use anyhow::{Context, Result};
use chrono::Utc;
use csa_core::types::OutputFormat;
use csa_session::{SessionArtifact, SessionResult};
use serde::Serialize;

use super::DebateMode;

#[derive(Debug, Serialize)]
pub(crate) struct DebateDryRunSummary {
    pub(crate) session_id: String,
    pub(crate) tool: String,
    pub(crate) model: String,
    pub(crate) prompt_bytes: usize,
    pub(crate) rounds: u32,
    pub(crate) mode: DebateMode,
}

pub(crate) fn create_debate_dry_run_session(
    project_root: &Path,
    description: &str,
    tool: &str,
    tier_name: Option<&str>,
    startup_session_id: Option<&str>,
) -> Result<String> {
    let mut session = csa_session::create_session(
        project_root,
        Some(description),
        startup_session_id,
        Some(tool),
    )?;
    session.task_context = csa_session::TaskContext {
        task_type: Some("debate".to_string()),
        tier_name: tier_name.map(str::to_string),
    };
    csa_session::save_session(&session)?;

    let now = Utc::now();
    let result = SessionResult {
        post_exec_gate: None,
        status: SessionResult::status_from_exit_code(0),
        exit_code: 0,
        summary: "debate dry-run complete: AI invocation skipped".to_string(),
        tool: tool.to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now,
        events_count: 0,
        artifacts: vec![SessionArtifact::new("dry-run")],
        ..Default::default()
    };
    csa_session::save_result(project_root, &session.meta_session_id, &result)?;
    Ok(session.meta_session_id)
}

pub(crate) fn render_debate_dry_run_summary(
    output_format: OutputFormat,
    summary: &DebateDryRunSummary,
) -> Result<String> {
    match output_format {
        OutputFormat::Text => Ok(format!(
            "Debate dry-run: OK\n\
             session: {}\n\
             tool: {}\n\
             model: {}\n\
             prompt_bytes: {}\n\
             rounds: {}\n\
             mode: {}\n\
             ai_invocation: skipped",
            summary.session_id,
            summary.tool,
            summary.model,
            summary.prompt_bytes,
            summary.rounds,
            format_debate_mode(summary.mode),
        )),
        OutputFormat::Json => {
            serde_json::to_string_pretty(summary).context("Failed to serialize dry-run JSON")
        }
    }
}

fn format_debate_mode(mode: DebateMode) -> &'static str {
    match mode {
        DebateMode::Heterogeneous => "heterogeneous",
        DebateMode::SameModelAdversarial => "same-model-adversarial",
    }
}
