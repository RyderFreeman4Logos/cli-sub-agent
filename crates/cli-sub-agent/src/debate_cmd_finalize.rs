use std::path::Path;

use anyhow::Result;
use csa_core::types::OutputFormat;
use tracing::warn;

use super::{DebateMode, render_debate_cli_output};
use crate::debate_cmd_output::{
    DebateOutputHeader, DebateSummary, append_debate_artifacts_to_result, extract_debate_summary,
    persist_debate_output_artifacts, render_debate_output,
};
use crate::tier_model_fallback::{TierAttemptFailure, format_all_models_failed_reason};

pub(crate) struct FinalizedDebateOutcome {
    pub(crate) exit_code: i32,
    pub(crate) rendered_output: String,
}

pub(crate) struct DebateFinalizeContext<'a> {
    pub(crate) all_tier_models_failed: bool,
    pub(crate) resolved_tier_name: Option<&'a str>,
    pub(crate) failures: &'a [TierAttemptFailure],
    pub(crate) debate_mode: DebateMode,
    pub(crate) output_header: Option<DebateOutputHeader>,
}

fn build_unavailable_debate_summary(
    resolved_tier_name: Option<&str>,
    failures: &[TierAttemptFailure],
    debate_mode: DebateMode,
) -> DebateSummary {
    let failure_reason = format_all_models_failed_reason(resolved_tier_name, failures)
        .unwrap_or_else(|| "all configured debate tier models failed".to_string());
    DebateSummary {
        verdict: "UNAVAILABLE".to_string(),
        decision: Some("unavailable".to_string()),
        confidence: "low".to_string(),
        summary: format!("Debate unavailable: {failure_reason}"),
        key_points: failures
            .iter()
            .map(|failure| format!("{}={}", failure.model_spec, failure.reason))
            .collect(),
        failure_reason: Some(failure_reason),
        mode: debate_mode,
    }
}

pub(crate) fn finalize_debate_outcome(
    project_root: &Path,
    output_format: OutputFormat,
    execution: Option<crate::pipeline::SessionExecutionResult>,
    context: DebateFinalizeContext<'_>,
) -> Result<FinalizedDebateOutcome> {
    let (exit_code, meta_session_id, persisted_session_id, output, debate_summary) =
        match (context.all_tier_models_failed, execution) {
            (true, Some(execution)) => {
                let persisted_session_id = resolve_persisted_debate_session_id(
                    project_root,
                    &execution.meta_session_id,
                    true,
                )?;
                let output = render_debate_output(
                    &execution.execution.output,
                    persisted_session_id
                        .as_deref()
                        .unwrap_or(execution.meta_session_id.as_str()),
                    execution.provider_session_id.as_deref(),
                );
                (
                    execution.execution.exit_code,
                    execution.meta_session_id,
                    persisted_session_id,
                    output,
                    build_unavailable_debate_summary(
                        context.resolved_tier_name,
                        context.failures,
                        context.debate_mode,
                    ),
                )
            }
            (true, None) => {
                let meta_session_id = "unknown".to_string();
                let output = render_debate_output("", &meta_session_id, None);
                (
                    1,
                    meta_session_id,
                    None,
                    output,
                    build_unavailable_debate_summary(
                        context.resolved_tier_name,
                        context.failures,
                        context.debate_mode,
                    ),
                )
            }
            (false, Some(execution)) => {
                let persisted_session_id = resolve_persisted_debate_session_id(
                    project_root,
                    &execution.meta_session_id,
                    false,
                )?;
                let output = render_debate_output(
                    &execution.execution.output,
                    persisted_session_id
                        .as_deref()
                        .unwrap_or(execution.meta_session_id.as_str()),
                    execution.provider_session_id.as_deref(),
                );
                let debate_summary = extract_debate_summary(
                    &output,
                    execution.execution.summary.as_str(),
                    context.debate_mode,
                );
                (
                    execution.execution.exit_code,
                    execution.meta_session_id,
                    persisted_session_id,
                    output,
                    debate_summary,
                )
            }
            (false, None) => unreachable!("debate tier candidate list is never empty"),
        };

    if let Some(session_id) = persisted_session_id.as_deref() {
        let session_dir = csa_session::get_session_dir(project_root, session_id)?;
        let artifacts = persist_debate_output_artifacts(&session_dir, &debate_summary, &output)?;
        append_debate_artifacts_to_result(project_root, session_id, &artifacts, &debate_summary)?;
    }

    let rendered_output = render_debate_cli_output(
        output_format,
        &debate_summary,
        &output,
        &meta_session_id,
        context.output_header,
    )?;
    Ok(FinalizedDebateOutcome {
        exit_code,
        rendered_output,
    })
}

pub(crate) fn resolve_persisted_debate_session_id(
    project_root: &Path,
    meta_session_id: &str,
    allow_missing_for_all_tier_failure: bool,
) -> Result<Option<String>> {
    match csa_session::load_session(project_root, meta_session_id) {
        Ok(_) => Ok(Some(meta_session_id.to_string())),
        Err(err) if allow_missing_for_all_tier_failure => {
            warn!(
                session_id = meta_session_id,
                error = %err,
                "Skipping debate artifact persistence because no owned session directory exists"
            );
            Ok(None)
        }
        Err(err) => Err(err),
    }
}
