use std::{fs, path::Path};

use anyhow::Result;
use csa_config::ProjectConfig;
use csa_core::types::{OutputFormat, ToolName};
use tracing::warn;

use super::{DebateMode, render_debate_cli_output};
use crate::debate_cmd_output::{
    DebateOutputHeader, DebateSummary, DebateVerdict, append_debate_artifacts_to_result,
    extract_debate_summary_with_metadata, persist_debate_output_artifacts, render_debate_output,
};
use crate::tier_model_fallback::{
    TierAttemptFailure, format_all_models_failed_reason, persist_fallback_chain,
    persist_fallback_result_fields,
};

pub(crate) struct FinalizedDebateOutcome {
    pub(crate) exit_code: i32,
    pub(crate) rendered_output: String,
}

pub(crate) struct DebateFinalizeContext<'a> {
    pub(crate) all_tier_models_failed: bool,
    pub(crate) project_config: Option<&'a ProjectConfig>,
    pub(crate) resolved_tier_name: Option<&'a str>,
    pub(crate) failures: &'a [TierAttemptFailure],
    pub(crate) debate_mode: DebateMode,
    pub(crate) output_header: Option<DebateOutputHeader>,
    pub(crate) original_tool: Option<ToolName>,
    pub(crate) fallback_tool: Option<ToolName>,
    pub(crate) fallback_reason: Option<&'a str>,
    /// Winning debater model spec, if the debate succeeded. Bounds the persisted
    /// failover chain to before-winner skips (#1714).
    pub(crate) selected_model_spec: Option<&'a str>,
    pub(crate) tier_preference_order: &'a [String],
    pub(crate) fail_on_revise: bool,
    pub(crate) fail_on_reject: bool,
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
    // A completed debate whose rendered output carries a recognized verdict
    // ran successfully. Verdict polarity is debate content; only explicit
    // fail-on flags turn REVISE/REJECT back into a non-zero pipeline signal.
    let (
        completed_debate_with_verdict,
        meta_session_id,
        persisted_session_id,
        output,
        debate_summary,
    ) = match (context.all_tier_models_failed, execution) {
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
                false,
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
                false,
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
            let debate_extraction = extract_debate_summary_with_metadata(
                &output,
                execution.execution.summary.as_str(),
                context.debate_mode,
            );
            let completed_debate_with_verdict = debate_extraction.had_explicit_verdict;
            (
                completed_debate_with_verdict,
                execution.meta_session_id,
                persisted_session_id,
                output,
                debate_extraction.summary,
            )
        }
        (false, None) => unreachable!("debate tier candidate list is never empty"),
    };

    let final_exit_code = if let Some(session_id) = persisted_session_id.as_deref() {
        let session_dir = csa_session::get_session_dir(project_root, session_id)?;
        let artifacts = persist_debate_output_artifacts(&session_dir, &debate_summary, &output)?;
        let artifact_exit_code = persisted_debate_verdict_exit_code(&session_dir);
        let resolved_exit_code = resolve_debate_session_exit_code(
            artifact_exit_code,
            completed_debate_with_verdict,
            &debate_summary,
            context.fail_on_revise,
            context.fail_on_reject,
        );
        append_debate_artifacts_to_result(project_root, session_id, &artifacts, &debate_summary)?;
        persist_debate_exit_code(
            project_root,
            session_id,
            resolved_exit_code,
            &debate_summary.summary,
        )?;
        if let (Some(original_tool), Some(fallback_tool)) =
            (context.original_tool, context.fallback_tool)
        {
            persist_fallback_result_fields(
                project_root,
                session_id,
                original_tool,
                fallback_tool,
                context.fallback_reason,
            );
            persist_fallback_chain(
                project_root,
                session_id,
                original_tool,
                fallback_tool,
                crate::tier_model_fallback::build_fallback_chain_for_result(
                    context.project_config,
                    context.resolved_tier_name,
                    context.failures,
                    context.selected_model_spec,
                    context.tier_preference_order,
                ),
            );
        }
        resolved_exit_code
    } else if completed_debate_with_verdict {
        resolve_debate_session_exit_code(
            crate::verdict_exit_code::exit_code_from_debate_verdict(
                debate_summary.verdict.as_str(),
                debate_summary.decision.as_deref(),
            ),
            true,
            &debate_summary,
            context.fail_on_revise,
            context.fail_on_reject,
        )
    } else {
        crate::verdict_exit_code::INFRASTRUCTURE_FAILURE_EXIT_CODE
    };

    let rendered_output = render_debate_cli_output(
        output_format,
        &debate_summary,
        &output,
        &meta_session_id,
        context.output_header,
    )?;
    Ok(FinalizedDebateOutcome {
        exit_code: final_exit_code,
        rendered_output,
    })
}

fn persisted_debate_verdict_exit_code(session_dir: &Path) -> i32 {
    let verdict_path = session_dir.join("output").join("debate-verdict.json");
    let raw = match fs::read_to_string(&verdict_path) {
        Ok(raw) => raw,
        Err(error) => {
            warn!(
                path = %verdict_path.display(),
                error = %error,
                "Missing or unreadable debate verdict artifact; treating as infrastructure failure"
            );
            return crate::verdict_exit_code::INFRASTRUCTURE_FAILURE_EXIT_CODE;
        }
    };
    let artifact = match serde_json::from_str::<DebateVerdict>(&raw) {
        Ok(artifact) => artifact,
        Err(error) => {
            warn!(
                path = %verdict_path.display(),
                error = %error,
                "Invalid debate verdict artifact; treating as infrastructure failure"
            );
            return crate::verdict_exit_code::INFRASTRUCTURE_FAILURE_EXIT_CODE;
        }
    };

    crate::verdict_exit_code::exit_code_from_debate_verdict(
        artifact.verdict.as_str(),
        artifact.decision.as_deref(),
    )
}

fn resolve_debate_session_exit_code(
    artifact_exit_code: i32,
    completed_debate_with_verdict: bool,
    summary: &DebateSummary,
    fail_on_revise: bool,
    fail_on_reject: bool,
) -> i32 {
    if artifact_exit_code == crate::verdict_exit_code::INFRASTRUCTURE_FAILURE_EXIT_CODE {
        return artifact_exit_code;
    }
    if !completed_debate_with_verdict {
        return artifact_exit_code;
    }
    if fail_on_revise && debate_token_matches(summary, "REVISE") {
        return 1;
    }
    if fail_on_reject && debate_token_matches(summary, "REJECT") {
        return 1;
    }
    0
}

fn debate_token_matches(summary: &DebateSummary, expected: &str) -> bool {
    summary.verdict.trim().eq_ignore_ascii_case(expected)
        || summary
            .decision
            .as_deref()
            .is_some_and(|decision| decision.trim().eq_ignore_ascii_case(expected))
}

fn persist_debate_exit_code(
    project_root: &Path,
    session_id: &str,
    exit_code: i32,
    summary: &str,
) -> Result<()> {
    let mut result = csa_session::load_result(project_root, session_id)?
        .ok_or_else(|| anyhow::anyhow!("Missing result.toml for debate session {session_id}"))?;
    if result.exit_code == exit_code
        && result.status == csa_session::SessionResult::status_from_exit_code(exit_code)
        && result.summary == summary
    {
        return Ok(());
    }

    result.exit_code = exit_code;
    result.status = csa_session::SessionResult::status_from_exit_code(exit_code);
    result.summary = summary.to_string();
    csa_session::save_result(project_root, session_id, &result)?;
    Ok(())
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
