//! Review execution helpers extracted from `review_cmd.rs`.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use csa_config::{ExecutionEnvOptions, GlobalConfig, ProjectConfig};
use csa_core::{
    gemini::{
        API_KEY_ENV, API_KEY_FALLBACK_ENV_KEY, AUTH_MODE_API_KEY, AUTH_MODE_ENV_KEY,
        AUTH_MODE_OAUTH,
    },
    types::{OutputFormat, ToolName},
};
use csa_executor::Executor;
use csa_session::{
    SessionResult, get_session_dir, load_result, load_session, save_result, save_session,
};
use tracing::{info, warn};

use crate::review_routing::{ReviewRoutingMetadata, persist_review_routing_artifact};

use super::output::{
    ToolReviewFailureKind, derive_review_result_summary, detect_tool_review_failure,
    has_structured_review_content, is_edit_restriction_summary, is_review_output_empty,
};

pub(super) struct ReviewExecutionOutcome {
    pub execution: crate::pipeline::SessionExecutionResult,
    pub status_reason: Option<String>,
}

fn review_execution_env_options(no_failover: bool) -> ExecutionEnvOptions {
    let options = ExecutionEnvOptions::with_no_flash_fallback();
    if no_failover {
        options.with_no_failover()
    } else {
        options
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn execute_review(
    tool: ToolName,
    prompt: String,
    session: Option<String>,
    model: Option<String>,
    tier_model_spec: Option<String>,
    tier_name: Option<String>,
    thinking: Option<String>,
    description: String,
    project_root: &Path,
    project_config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
    review_routing: ReviewRoutingMetadata,
    stream_mode: csa_process::StreamMode,
    idle_timeout_seconds: u64,
    initial_response_timeout_seconds: Option<u64>,
    force_override_user_config: bool,
    force_ignore_tier_setting: bool,
    no_failover: bool,
    no_fs_sandbox: bool,
    readonly_project_root: bool,
    extra_writable: &[PathBuf],
) -> Result<ReviewExecutionOutcome> {
    let execution_started_at = Utc::now();
    let enforce_tier =
        tier_name.is_some() && tier_model_spec.is_some() && !force_ignore_tier_setting;
    let executor = crate::pipeline::build_and_validate_executor(
        &tool,
        tier_model_spec.as_deref(),
        model.as_deref(),
        thinking.as_deref(),
        crate::pipeline::ConfigRefs {
            project: project_config,
            global: Some(global_config),
        },
        enforce_tier,
        force_override_user_config,
        false, // review must not inherit `csa run` per-tool defaults
    )
    .await?;

    let can_edit =
        project_config.is_none_or(|cfg| cfg.can_tool_edit_existing(executor.tool_name()));
    let can_write_new =
        project_config.is_none_or(|cfg| cfg.can_tool_write_new(executor.tool_name()));
    let mut effective_prompt = if !can_edit || !can_write_new {
        info!(
            tool = %executor.tool_name(),
            can_edit,
            can_write_new,
            "Applying filesystem restrictions via prompt injection"
        );
        executor.apply_restrictions(&prompt, can_edit, can_write_new)
    } else {
        prompt
    };

    let extra_env_owned = global_config.build_execution_env(
        executor.tool_name(),
        review_execution_env_options(no_failover),
    );
    let _slot_guard = crate::pipeline::acquire_slot(&executor, global_config)?;

    if session.is_none()
        && let Ok(inherited_session_id) = std::env::var("CSA_SESSION_ID")
    {
        warn!(
            inherited_session_id = %inherited_session_id,
            "Ignoring inherited CSA_SESSION_ID for `csa review`; pass --session to resume explicitly"
        );
    }

    if let Some(guard) = crate::pipeline::prompt_guard::anti_recursion_guard(project_config) {
        effective_prompt = format!("{guard}\n\n{effective_prompt}");
    }

    let mut execution = match execute_review_once(
        &executor,
        &tool,
        &effective_prompt,
        session.clone(),
        description.clone(),
        project_root,
        project_config,
        extra_env_owned.as_ref(),
        tier_name.as_deref(),
        global_config,
        stream_mode,
        idle_timeout_seconds,
        initial_response_timeout_seconds,
        no_fs_sandbox,
        readonly_project_root,
        extra_writable,
    )
    .await
    {
        Ok(execution) => execution,
        Err(err) => {
            maybe_synthesize_missing_review_result(project_root, tool, execution_started_at, &err);
            return Err(err);
        }
    };

    persist_review_routing_artifact(project_root, &execution.meta_session_id, &review_routing);
    repair_completed_review_restriction_result(project_root, tool, &mut execution)?;

    let mut status_reason = None;
    if let Some(kind) = detect_tool_review_failure(tool, &execution.execution.output) {
        let retry_env = (!no_failover)
            .then(|| build_gemini_api_key_retry_env(extra_env_owned.as_ref()))
            .flatten();
        warn!(
            tool = %tool,
            reason = kind.status_reason(),
            retry_attempted = retry_env.is_some(),
            "Detected Gemini OAuth browser prompt during review execution"
        );

        if let Some(api_key_env) = retry_env {
            let mut retried = match execute_review_once(
                &executor,
                &tool,
                &effective_prompt,
                session,
                description,
                project_root,
                project_config,
                Some(&api_key_env),
                tier_name.as_deref(),
                global_config,
                stream_mode,
                idle_timeout_seconds,
                initial_response_timeout_seconds,
                no_fs_sandbox,
                readonly_project_root,
                extra_writable,
            )
            .await
            {
                Ok(execution) => execution,
                Err(err) => {
                    maybe_synthesize_missing_review_result(
                        project_root,
                        tool,
                        execution_started_at,
                        &err,
                    );
                    return Err(err);
                }
            };
            persist_review_routing_artifact(
                project_root,
                &retried.meta_session_id,
                &review_routing,
            );
            repair_completed_review_restriction_result(project_root, tool, &mut retried)?;

            if let Some(retry_kind) = detect_tool_review_failure(tool, &retried.execution.output) {
                classify_review_failure_result(project_root, tool, &mut retried, retry_kind)?;
                status_reason = Some(retry_kind.status_reason().to_string());
                execution = retried;
            } else {
                execution = retried;
            }
        } else {
            classify_review_failure_result(project_root, tool, &mut execution, kind)?;
            status_reason = Some(kind.status_reason().to_string());
        }
    }

    Ok(ReviewExecutionOutcome {
        execution,
        status_reason,
    })
}

#[allow(clippy::too_many_arguments)]
async fn execute_review_once(
    executor: &Executor,
    tool: &ToolName,
    effective_prompt: &str,
    session: Option<String>,
    description: String,
    project_root: &Path,
    project_config: Option<&ProjectConfig>,
    extra_env: Option<&HashMap<String, String>>,
    tier_name: Option<&str>,
    global_config: &GlobalConfig,
    stream_mode: csa_process::StreamMode,
    idle_timeout_seconds: u64,
    initial_response_timeout_seconds: Option<u64>,
    no_fs_sandbox: bool,
    readonly_project_root: bool,
    extra_writable: &[PathBuf],
) -> Result<crate::pipeline::SessionExecutionResult> {
    crate::pipeline::execute_with_session_and_meta_with_parent_source(
        executor,
        tool,
        effective_prompt,
        OutputFormat::Json,
        session,
        Some(description),
        None,
        project_root,
        project_config,
        extra_env,
        Some("review"),
        tier_name,
        None,
        stream_mode,
        idle_timeout_seconds,
        initial_response_timeout_seconds,
        None,
        None,
        Some(global_config),
        crate::pipeline::ParentSessionSource::ExplicitOnly,
        crate::pipeline::SessionCreationMode::DaemonManaged,
        no_fs_sandbox,
        readonly_project_root,
        extra_writable,
    )
    .await
}

fn build_gemini_api_key_retry_env(
    extra_env: Option<&HashMap<String, String>>,
) -> Option<HashMap<String, String>> {
    let env = extra_env?;
    if env.get(AUTH_MODE_ENV_KEY).map(String::as_str) != Some(AUTH_MODE_OAUTH) {
        return None;
    }

    let api_key = env.get(API_KEY_FALLBACK_ENV_KEY)?;
    let mut retry_env = env.clone();
    retry_env.insert(API_KEY_ENV.to_string(), api_key.clone());
    retry_env.insert(AUTH_MODE_ENV_KEY.to_string(), AUTH_MODE_API_KEY.to_string());
    retry_env.remove(API_KEY_FALLBACK_ENV_KEY);
    Some(retry_env)
}

fn classify_review_failure_result(
    project_root: &Path,
    tool: ToolName,
    execution: &mut crate::pipeline::SessionExecutionResult,
    failure: ToolReviewFailureKind,
) -> Result<()> {
    let summary = failure.summary_note().to_string();
    if execution.execution.stderr_output.is_empty() {
        execution.execution.stderr_output = summary.clone();
    } else if !execution.execution.stderr_output.contains(&summary) {
        if !execution.execution.stderr_output.ends_with('\n') {
            execution.execution.stderr_output.push('\n');
        }
        execution.execution.stderr_output.push_str(&summary);
        execution.execution.stderr_output.push('\n');
    }
    execution.execution.exit_code = 1;
    execution.execution.summary = summary.clone();

    let Some(mut persisted_result) = load_result(project_root, &execution.meta_session_id)
        .with_context(|| {
            format!(
                "failed to load result.toml for classified review session {}",
                execution.meta_session_id
            )
        })?
    else {
        return Ok(());
    };
    persisted_result.status = SessionResult::status_from_exit_code(1);
    persisted_result.exit_code = 1;
    persisted_result.summary = summary.clone();
    save_result(project_root, &execution.meta_session_id, &persisted_result).with_context(
        || {
            format!(
                "failed to rewrite classified result.toml for review session {}",
                execution.meta_session_id
            )
        },
    )?;

    let mut session =
        load_session(project_root, &execution.meta_session_id).with_context(|| {
            format!(
                "failed to load session state for classified review session {}",
                execution.meta_session_id
            )
        })?;
    if let Some(tool_state) = session.tools.get_mut(tool.as_str()) {
        tool_state.last_action_summary = summary;
        tool_state.last_exit_code = 1;
        tool_state.updated_at = chrono::Utc::now();
        save_session(&session).with_context(|| {
            format!(
                "failed to rewrite session state for classified review session {}",
                execution.meta_session_id
            )
        })?;
    }

    Ok(())
}

fn maybe_synthesize_missing_review_result(
    project_root: &Path,
    tool: ToolName,
    started_at: DateTime<Utc>,
    error: &anyhow::Error,
) {
    let Some(session_id) = extract_meta_session_id_from_error(error) else {
        return;
    };

    match load_result(project_root, &session_id) {
        Ok(Some(_)) => return,
        Ok(None) => {}
        Err(load_err) => {
            warn!(
                session_id = %session_id,
                error = %load_err,
                "Failed to check for existing review result.toml before fallback synthesis"
            );
        }
    }

    let session_dir = match get_session_dir(project_root, &session_id) {
        Ok(path) => path,
        Err(session_dir_err) => {
            warn!(
                session_id = %session_id,
                error = %session_dir_err,
                "Failed to resolve review session dir for fallback result synthesis"
            );
            return;
        }
    };

    let stderr_excerpt = read_review_failure_excerpt(&session_dir)
        .unwrap_or_else(|| truncate_for_summary(&format!("{error:#}"), 500));
    let (status, exit_code, error_kind) = classify_review_failure(error, &stderr_excerpt);
    let summary = truncate_for_summary(
        &format!("review {error_kind}: {}", stderr_excerpt.trim()),
        200,
    );
    let completed_at = Utc::now();
    let fallback_result = SessionResult {
        status: status.to_string(),
        exit_code,
        summary,
        tool: tool.to_string(),
        started_at,
        completed_at,
        events_count: 0,
        artifacts: Vec::new(),
        peak_memory_mb: None,
    };

    if let Err(save_err) = save_result(project_root, &session_id, &fallback_result) {
        warn!(
            session_id = %session_id,
            error = %save_err,
            "Failed to synthesize missing review result.toml"
        );
        return;
    }

    csa_session::write_cooldown_marker_for_project(project_root, &session_id, completed_at);
    warn!(
        session_id = %session_id,
        error_kind,
        "Synthesized missing review result.toml after execution error"
    );
}

fn extract_meta_session_id_from_error(error: &anyhow::Error) -> Option<String> {
    const MARKER: &str = "meta_session_id=";
    for cause in error.chain() {
        let message = cause.to_string();
        let Some(idx) = message.find(MARKER) else {
            continue;
        };
        let suffix = &message[idx + MARKER.len()..];
        let session_id: String = suffix
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric())
            .collect();
        if !session_id.is_empty() {
            return Some(session_id);
        }
    }
    None
}

fn read_review_failure_excerpt(session_dir: &Path) -> Option<String> {
    let stderr_path = session_dir.join("stderr.log");
    let contents = fs::read_to_string(stderr_path).ok()?;
    let trimmed = contents.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(truncate_for_summary(trimmed, 500))
}

fn classify_review_failure(
    error: &anyhow::Error,
    excerpt: &str,
) -> (&'static str, i32, &'static str) {
    let mut combined = excerpt.to_ascii_lowercase();
    for cause in error.chain() {
        combined.push('\n');
        combined.push_str(&cause.to_string().to_ascii_lowercase());
    }

    if combined.contains("initial_response_timeout")
        || combined.contains("timed out")
        || combined.contains("timeout")
    {
        ("timeout", 124, "timeout")
    } else if combined.contains("sigkill")
        || combined.contains("sigterm")
        || combined.contains("killed")
        || combined.contains("terminated by signal")
    {
        ("signal", 137, "signal")
    } else if combined.contains("fork")
        || combined.contains("spawn")
        || combined.contains("provider_session_id")
    {
        ("failure", 1, "spawn_fail")
    } else {
        ("failure", 1, "tool_crash")
    }
}

fn truncate_for_summary(text: &str, max_chars: usize) -> String {
    let truncated: String = text.chars().take(max_chars).collect();
    truncated.trim().replace('\n', " ")
}

fn repair_completed_review_restriction_result(
    project_root: &Path,
    tool: ToolName,
    execution: &mut crate::pipeline::SessionExecutionResult,
) -> Result<()> {
    if !should_repair_completed_review_restriction(&execution.execution) {
        return Ok(());
    }

    let repaired_summary = derive_review_result_summary(&execution.execution.output)
        .unwrap_or_else(|| execution.execution.summary.clone());

    info!(
        session_id = %execution.meta_session_id,
        tool = %tool,
        "Reclassifying completed review with edit restriction as success"
    );

    execution.execution.exit_code = 0;
    execution.execution.summary = repaired_summary.clone();

    let Some(mut persisted_result) = load_result(project_root, &execution.meta_session_id)
        .with_context(|| {
            format!(
                "failed to load result.toml for review session {}",
                execution.meta_session_id
            )
        })?
    else {
        return Ok(());
    };
    persisted_result.status = SessionResult::status_from_exit_code(0);
    persisted_result.exit_code = 0;
    persisted_result.summary = repaired_summary.clone();
    save_result(project_root, &execution.meta_session_id, &persisted_result).with_context(
        || {
            format!(
                "failed to rewrite repaired result.toml for review session {}",
                execution.meta_session_id
            )
        },
    )?;

    let mut session =
        load_session(project_root, &execution.meta_session_id).with_context(|| {
            format!(
                "failed to load session state for repaired review session {}",
                execution.meta_session_id
            )
        })?;
    if let Some(tool_state) = session.tools.get_mut(tool.as_str()) {
        tool_state.last_action_summary = repaired_summary;
        tool_state.last_exit_code = 0;
        tool_state.updated_at = chrono::Utc::now();
        save_session(&session).with_context(|| {
            format!(
                "failed to rewrite session state for repaired review session {}",
                execution.meta_session_id
            )
        })?;
    }

    Ok(())
}

fn should_repair_completed_review_restriction(execution: &csa_process::ExecutionResult) -> bool {
    execution.exit_code != 0
        && is_edit_restriction_summary(&execution.summary)
        && !is_review_output_empty(&execution.output)
        && has_structured_review_content(&execution.output)
}

/// Compute a SHA-256 content hash of the diff being reviewed.
///
/// The fingerprint enables diff-level deduplication: if two review
/// invocations produce the same diff content (e.g., revert-then-revert),
/// the second can reuse the first review's result.
pub(super) fn compute_diff_fingerprint(project_root: &Path, scope: &str) -> Option<String> {
    use sha2::{Digest, Sha256};

    let diff_args: Vec<&str> = if scope == "uncommitted" {
        vec!["diff", "HEAD"]
    } else if let Some(range) = scope.strip_prefix("range:") {
        vec!["diff", range]
    } else if let Some(base) = scope.strip_prefix("base:") {
        vec!["diff", base]
    } else {
        return None;
    };

    let output = std::process::Command::new("git")
        .args(&diff_args)
        .current_dir(project_root)
        .output()
        .ok()?;

    if !output.status.success() || output.stdout.is_empty() {
        return None;
    }

    let digest = Sha256::digest(&output.stdout);
    Some(format!("sha256:{digest:x}"))
}

#[cfg(test)]
#[path = "review_cmd_execute_tests.rs"]
mod tests;
