//! Fix loop for `csa review --fix`: resumes the review session to apply fixes,
//! then re-gates via quality pipeline after each round.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use tracing::{error, info, warn};

use crate::bug_class::{CONSOLIDATED_REVIEW_ARTIFACT_FILE, SINGLE_REVIEW_ARTIFACT_FILE};
use crate::review_routing::ReviewRoutingMetadata;
use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::ToolName;
use csa_session::{FindingsFile, state::ReviewSessionMeta, write_findings_toml};

use super::CLEAN;
use super::output::{
    is_review_output_empty, persist_review_meta, persist_review_verdict, sanitize_review_output,
};
use super::resolve::ANTI_RECURSION_PREAMBLE;

const CLEAN_CONVERGENCE_STALE_OUTPUT_FILES: &[&str] = &[
    "suggestion.toml",
    super::findings_toml::FINDINGS_TOML_SYNTHETIC_MARKER,
];
const REVIEW_PROSE_SECTION_IDS: &[&str] = &["summary", "details"];

/// All context needed to run the fix loop after a review finds issues.
pub(crate) struct FixLoopContext<'a> {
    pub effective_tool: ToolName,
    pub config: Option<&'a ProjectConfig>,
    pub global_config: &'a GlobalConfig,
    pub review_model: Option<String>,
    pub effective_tier_model_spec: Option<String>,
    pub review_thinking: Option<String>,
    pub review_routing: ReviewRoutingMetadata,
    pub stream_mode: csa_process::StreamMode,
    pub idle_timeout_seconds: u64,
    pub initial_response_timeout_seconds: Option<u64>,
    pub force_override_user_config: bool,
    pub force_ignore_tier_setting: bool,
    pub no_failover: bool,
    pub fast_but_more_cost: bool,
    pub no_fs_sandbox: bool,
    /// CLI `--no-error-marker-scan`: disable the #1652 scan for fix rounds (#1745).
    pub no_error_marker_scan: bool,
    pub extra_writable: &'a [PathBuf],
    pub extra_readable: &'a [PathBuf],
    pub timeout: Option<u64>,
    pub project_root: &'a Path,
    pub scope: String,
    pub decision: String,
    pub verdict: String,
    pub max_rounds: u8,
    pub initial_session_id: String,
    pub review_iterations: u32,
}

/// Run fix rounds, returning the final exit code.
///
/// Each round resumes the review session with a fix prompt, then re-runs
/// the quality gate.  Returns `Ok(0)` on first passing round, or
/// `Ok(1)` when all rounds are exhausted.
pub(crate) async fn run_fix_loop(ctx: FixLoopContext<'_>) -> Result<i32> {
    let mut session_id = ctx.initial_session_id;

    for round in 1..=ctx.max_rounds {
        info!(round, max_rounds = ctx.max_rounds, session_id = %session_id, "Fix round starting");

        let fix_prompt = format!(
            "{ANTI_RECURSION_PREAMBLE}\
             Fix round {round}/{}.\n\
             Fix all issues found in the review. Run formatting and linting commands as needed.\n\
             After applying fixes, verify the changes compile and pass basic checks.\n\
             If no issues remain, emit verdict: CLEAN.",
            ctx.max_rounds,
        );

        let fix_future = super::execute_review_with_tier_filter(
            ctx.effective_tool,
            fix_prompt,
            Some(session_id.clone()),
            ctx.review_model.clone(),
            ctx.effective_tier_model_spec.clone(),
            None,
            false,
            Vec::new(),
            ctx.review_thinking.clone(),
            format!("fix round {round}/{}", ctx.max_rounds),
            ctx.project_root,
            ctx.config,
            ctx.global_config,
            None,
            ctx.review_routing.clone(),
            ctx.stream_mode,
            ctx.idle_timeout_seconds,
            ctx.initial_response_timeout_seconds,
            ctx.force_override_user_config,
            ctx.force_ignore_tier_setting,
            ctx.no_failover,
            ctx.fast_but_more_cost,
            false,
            ctx.no_fs_sandbox,
            false, // fix pass must write — override readonly_project_root
            ctx.extra_writable,
            ctx.extra_readable,
            ctx.no_error_marker_scan,
        );

        let fix_result = if let Some(timeout_secs) = ctx.timeout {
            match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), fix_future)
                .await
            {
                Ok(inner) => inner?,
                Err(_) => {
                    error!(
                        timeout_secs = timeout_secs,
                        round, "Fix round aborted: wall-clock timeout exceeded"
                    );
                    anyhow::bail!(
                        "Fix round {round}/{} aborted: --timeout {timeout_secs}s exceeded.",
                        ctx.max_rounds,
                    );
                }
            }
        } else {
            fix_future.await?
        };

        print!(
            "{}",
            sanitize_review_output(&fix_result.execution.execution.output)
        );
        let fix_empty = is_review_output_empty(&fix_result.execution.execution.output);
        if fix_empty {
            warn!(
                round,
                "Fix round produced no substantive output — treating as failed"
            );
        }
        session_id = fix_result.execution.meta_session_id.clone();

        // Run the quality gate after fix.
        let fix_gate_steps = ctx.global_config.review.effective_gate_steps();
        let fix_gate_timeout = ctx
            .config
            .and_then(|c| c.review.as_ref())
            .map(|r| r.gate_timeout_secs)
            .unwrap_or_else(csa_config::ReviewConfig::default_gate_timeout);
        let fix_gate_mode = &ctx.global_config.review.gate_mode;

        let gate_passed = if fix_gate_steps.is_empty() {
            let gate_command = ctx
                .config
                .and_then(|c| c.review.as_ref())
                .and_then(|r| r.gate_command.as_deref());
            let gate_result = crate::pipeline::gate::evaluate_quality_gate(
                ctx.project_root,
                gate_command,
                fix_gate_timeout,
                fix_gate_mode,
            )
            .await?;

            if !gate_result.passed() {
                warn!(
                    round,
                    max_rounds = ctx.max_rounds,
                    command = %gate_result.command,
                    exit_code = ?gate_result.exit_code,
                    "Quality gate still failing after fix round"
                );
            }
            gate_result.passed()
        } else {
            let pipeline_result = crate::pipeline::gate::evaluate_quality_gates(
                ctx.project_root,
                &fix_gate_steps,
                fix_gate_timeout,
                fix_gate_mode,
            )
            .await?;

            if !pipeline_result.passed {
                warn!(
                    round,
                    max_rounds = ctx.max_rounds,
                    failed_step = ?pipeline_result.failed_step,
                    "Quality gate pipeline still failing after fix round"
                );
            }
            pipeline_result.passed
        };

        if gate_passed && !fix_empty {
            info!(round, "Fix round succeeded — quality gate passed");
            let review_meta = ReviewSessionMeta {
                session_id: session_id.clone(),
                head_sha: csa_session::detect_git_head(ctx.project_root).unwrap_or_default(),
                decision: "pass".to_string(),
                verdict: CLEAN.to_string(),
                status_reason: None,
                routed_to: None,
                primary_failure: None,
                failure_reason: None,
                tool: ctx.effective_tool.to_string(),
                scope: ctx.scope.clone(),
                exit_code: 0,
                fix_attempted: true,
                fix_rounds: u32::from(round),
                review_iterations: ctx.review_iterations,
                timestamp: chrono::Utc::now(),
                diff_fingerprint: None,
            };
            persist_fix_final_artifacts_with_current_output(
                ctx.project_root,
                &review_meta,
                true,
                Some(&fix_result.execution.execution.output),
            );
            return Ok(0);
        }
    }

    // All fix rounds exhausted; gate still fails.
    let review_meta = ReviewSessionMeta {
        session_id,
        head_sha: csa_session::detect_git_head(ctx.project_root).unwrap_or_default(),
        decision: ctx.decision,
        verdict: ctx.verdict,
        status_reason: None,
        routed_to: None,
        primary_failure: None,
        failure_reason: None,
        tool: ctx.effective_tool.to_string(),
        scope: ctx.scope,
        exit_code: 1,
        fix_attempted: true,
        fix_rounds: u32::from(ctx.max_rounds),
        review_iterations: ctx.review_iterations,
        timestamp: chrono::Utc::now(),
        diff_fingerprint: None,
    };
    persist_fix_final_artifacts(ctx.project_root, &review_meta, false);
    error!(
        max_rounds = ctx.max_rounds,
        "All fix rounds exhausted — quality gate still failing"
    );
    Ok(1)
}

fn persist_fix_final_artifacts(
    project_root: &Path,
    review_meta: &ReviewSessionMeta,
    converged_clean: bool,
) {
    persist_fix_final_artifacts_with_current_output(
        project_root,
        review_meta,
        converged_clean,
        None,
    );
}

fn persist_fix_final_artifacts_with_current_output(
    project_root: &Path,
    review_meta: &ReviewSessionMeta,
    converged_clean: bool,
    current_fix_output: Option<&str>,
) {
    persist_review_meta(project_root, review_meta);
    if converged_clean {
        match csa_session::get_session_dir(project_root, &review_meta.session_id) {
            Ok(session_dir) => {
                if let Err(error) = write_findings_toml(&session_dir, &FindingsFile::default()) {
                    warn!(
                        session_id = %review_meta.session_id,
                        error = %error,
                        "Failed to write output/findings.toml after CLEAN convergence"
                    );
                }
                // Clear stale review JSON artifacts so the verdict loader does
                // not override the cleanly-converged empty findings.toml.
                for artifact_file in [
                    SINGLE_REVIEW_ARTIFACT_FILE,
                    CONSOLIDATED_REVIEW_ARTIFACT_FILE,
                ] {
                    let stale_artifact = session_dir.join(artifact_file);
                    if let Err(error) = std::fs::remove_file(&stale_artifact)
                        && error.kind() != std::io::ErrorKind::NotFound
                    {
                        warn!(
                            session_id = %review_meta.session_id,
                            artifact_file,
                            error = %error,
                            "Failed to remove stale review artifact after CLEAN convergence"
                        );
                    }
                }
                clear_clean_convergence_fail_signals(
                    &session_dir,
                    &review_meta.session_id,
                    current_fix_output,
                );
            }
            Err(error) => {
                warn!(
                    session_id = %review_meta.session_id,
                    error = %error,
                    "Cannot resolve session dir for final fix artifacts"
                );
            }
        }
    }
    persist_review_verdict(project_root, review_meta, &[], Vec::new());
    if converged_clean {
        crate::review_gate::maybe_write_review_gate_marker(
            project_root,
            &review_meta.head_sha,
            &review_meta.session_id,
            &review_meta.scope,
        );
    }
    super::post_review::persist_review_failure_suggestion(project_root, review_meta);
}

fn clear_clean_convergence_fail_signals(
    session_dir: &Path,
    session_id: &str,
    current_fix_output: Option<&str>,
) {
    let output_dir = session_dir.join("output");
    for stale_file in CLEAN_CONVERGENCE_STALE_OUTPUT_FILES {
        let stale_path = output_dir.join(stale_file);
        if let Err(error) = fs::remove_file(&stale_path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            warn!(
                session_id,
                stale_file,
                error = %error,
                "Failed to remove stale review output artifact after CLEAN convergence"
            );
        }
    }
    retain_current_review_prose_sections(session_dir, session_id, current_fix_output);
}

fn retain_current_review_prose_sections(
    session_dir: &Path,
    session_id: &str,
    current_fix_output: Option<&str>,
) {
    let output_dir = session_dir.join("output");
    let index_path = output_dir.join("index.toml");
    if !index_path.exists() {
        remove_legacy_review_prose_files(&output_dir, session_id);
        return;
    }

    let contents = match fs::read_to_string(&index_path) {
        Ok(contents) => contents,
        Err(error) => {
            warn!(
                session_id,
                error = %error,
                "Failed to read output/index.toml while clearing stale review prose"
            );
            return;
        }
    };
    let mut index: csa_session::OutputIndex = match toml::from_str(&contents) {
        Ok(index) => index,
        Err(error) => {
            warn!(
                session_id,
                error = %error,
                "Failed to parse output/index.toml while clearing stale review prose"
            );
            return;
        }
    };

    let current_review_sections = current_fix_output
        .map(current_output_review_prose_section_ids)
        .unwrap_or_default();
    let keep = review_prose_keep_mask(&index, &current_review_sections);

    if keep.iter().all(|keep_section| *keep_section) {
        return;
    }

    for (idx, section) in index.sections.iter().enumerate() {
        if keep[idx] {
            continue;
        }
        if let Some(file_path) = &section.file_path {
            let stale_path = output_dir.join(file_path);
            if let Err(error) = fs::remove_file(&stale_path)
                && error.kind() != std::io::ErrorKind::NotFound
            {
                warn!(
                    session_id,
                    file_path,
                    error = %error,
                    "Failed to remove stale review prose section after CLEAN convergence"
                );
            }
        }
    }

    index.sections = index
        .sections
        .into_iter()
        .enumerate()
        .filter_map(|(idx, section)| keep[idx].then_some(section))
        .collect();
    index.total_tokens = index
        .sections
        .iter()
        .map(|section| section.token_estimate)
        .sum();

    match toml::to_string_pretty(&index) {
        Ok(rendered) => {
            if let Err(error) = fs::write(&index_path, rendered) {
                warn!(
                    session_id,
                    error = %error,
                    "Failed to rewrite output/index.toml after clearing stale review prose"
                );
            }
        }
        Err(error) => {
            warn!(
                session_id,
                error = %error,
                "Failed to render output/index.toml after clearing stale review prose"
            );
        }
    }
}

fn review_prose_keep_mask(index: &csa_session::OutputIndex, current_ids: &[String]) -> Vec<bool> {
    let mut keep = vec![true; index.sections.len()];
    let mut expected_current = current_ids.iter().rev();
    let mut next_expected = expected_current.next();

    for (idx, section) in index.sections.iter().enumerate().rev() {
        if !is_review_prose_section_id(&section.id) {
            continue;
        }

        if next_expected.is_some_and(|expected| section.id == *expected) {
            next_expected = expected_current.next();
        } else {
            keep[idx] = false;
        }
    }

    keep
}

fn current_output_review_prose_section_ids(output: &str) -> Vec<String> {
    let mut ids = Vec::new();
    let mut open_section_id = None;

    for line in output.lines() {
        match parse_csa_section_marker(line) {
            Some(CsaSectionMarker::Start(id)) => {
                if let Some(previous_id) = open_section_id.take() {
                    push_review_prose_section_id(&mut ids, previous_id);
                }
                open_section_id = Some(id);
            }
            Some(CsaSectionMarker::End(id)) if open_section_id.as_deref() == Some(id.as_str()) => {
                push_review_prose_section_id(&mut ids, id);
                open_section_id = None;
            }
            None => {}
            Some(CsaSectionMarker::End(_)) => {}
        }
    }

    if let Some(id) = open_section_id {
        push_review_prose_section_id(&mut ids, id);
    }

    ids
}

fn push_review_prose_section_id(ids: &mut Vec<String>, section_id: String) {
    if is_review_prose_section_id(&section_id) {
        ids.push(section_id);
    }
}

enum CsaSectionMarker {
    Start(String),
    End(String),
}

fn parse_csa_section_marker(line: &str) -> Option<CsaSectionMarker> {
    let marker = line
        .trim()
        .strip_prefix("<!-- CSA:SECTION:")?
        .strip_suffix("-->")?
        .trim();
    let marker = marker
        .strip_suffix(":END")
        .map(|section_id| CsaSectionMarker::End(section_id.trim().to_string()))
        .unwrap_or_else(|| CsaSectionMarker::Start(marker.to_string()));
    Some(marker)
}

fn is_review_prose_section_id(section_id: &str) -> bool {
    REVIEW_PROSE_SECTION_IDS.contains(&section_id)
}

fn remove_legacy_review_prose_files(output_dir: &Path, session_id: &str) {
    for section_id in REVIEW_PROSE_SECTION_IDS {
        let stale_path = output_dir.join(format!("{section_id}.md"));
        if let Err(error) = fs::remove_file(&stale_path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            warn!(
                session_id,
                section_id,
                error = %error,
                "Failed to remove legacy review prose file after CLEAN convergence"
            );
        }
    }
}

#[cfg(test)]
pub(crate) fn persist_fix_final_artifacts_for_tests(
    project_root: &Path,
    review_meta: &ReviewSessionMeta,
    converged_clean: bool,
) {
    persist_fix_final_artifacts(project_root, review_meta, converged_clean);
}

#[cfg(test)]
pub(crate) fn persist_fix_final_artifacts_for_tests_with_output(
    project_root: &Path,
    review_meta: &ReviewSessionMeta,
    converged_clean: bool,
    current_fix_output: &str,
) {
    persist_fix_final_artifacts_with_current_output(
        project_root,
        review_meta,
        converged_clean,
        Some(current_fix_output),
    );
}

#[cfg(test)]
#[path = "review_cmd_fix_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "review_cmd_fix_convergence_tests.rs"]
mod convergence_tests;
