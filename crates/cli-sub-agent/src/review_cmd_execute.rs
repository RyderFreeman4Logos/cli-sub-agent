//! Review execution helpers extracted from `review_cmd.rs`.

#[path = "review_cmd_execute_artifact_guard.rs"]
mod artifact_guard;
#[path = "review_cmd_execute_failures.rs"]
mod failures;

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use csa_config::{ExecutionEnvOptions, GlobalConfig, ProjectConfig};
use csa_core::{
    gemini::{
        API_KEY_ENV, API_KEY_FALLBACK_ENV_KEY, AUTH_MODE_API_KEY, AUTH_MODE_ENV_KEY,
        AUTH_MODE_OAUTH,
    },
    types::{OutputFormat, ReviewDecision, ToolName},
};
use csa_executor::{Executor, PeakMemoryContext};
use csa_session::{
    SessionResult, get_session_dir, load_result, load_session, save_result, save_session,
};
use tracing::{info, warn};

use crate::review_routing::{
    ReviewRoutingMetadata, persist_review_routing_artifact_with_fallback_chain,
};
use crate::startup_env::StartupSubtreeEnv;
use crate::tier_model_fallback::{
    TierAttemptFailure, chain_failure_reasons, earliest_backend_reset_window,
    fallback_reason_for_result, format_all_models_failed_reason_with_reset,
    ordered_tier_candidates, parse_backend_reset_duration, persist_fallback_chain,
    persist_fallback_result_fields,
};

use super::output::{
    ToolReviewFailureKind, derive_review_result_summary, detect_tool_review_failure,
    ensure_review_summary_artifact, has_structured_review_content, is_edit_restriction_summary,
    is_review_output_empty,
};
use artifact_guard::detect_repo_root_review_artifact_violations;
#[cfg(test)]
use failures::read_review_failure_excerpt;
use failures::{
    build_gemini_api_key_retry_env, classify_review_failover_error,
    classify_review_failover_reason, classify_review_failure_result,
    enforce_review_artifact_contract, extract_meta_session_id_from_error,
    maybe_synthesize_missing_review_result, repair_completed_review_restriction_result,
    retire_tier_failover_session,
};

const REVIEWER_SUB_SESSION_TASK_TYPE: &str = "reviewer_sub_session";
const CSA_READONLY_SESSION_ENV: &str = "CSA_READONLY_SESSION";

fn with_readonly_session_env(
    base: Option<&HashMap<String, String>>,
    readonly: bool,
) -> Option<HashMap<String, String>> {
    let mut env = base.cloned().unwrap_or_default();
    if readonly {
        env.insert(CSA_READONLY_SESSION_ENV.to_string(), "1".to_string());
    }
    (!env.is_empty()).then_some(env)
}

fn review_prompt_is_readonly(prompt: &str) -> bool {
    prompt.contains("Use the csa-review skill.")
}

pub(crate) struct ReviewExecutionOutcome {
    pub execution: crate::pipeline::SessionExecutionResult,
    pub persistable_session_id: Option<String>,
    pub executed_tool: ToolName,
    pub status_reason: Option<String>,
    pub forced_decision: Option<ReviewDecision>,
    pub routed_to: Option<String>,
    pub primary_failure: Option<String>,
    pub failure_reason: Option<String>,
}
fn review_execution_env_options(no_failover: bool) -> ExecutionEnvOptions {
    let options = ExecutionEnvOptions::with_no_flash_fallback();
    if no_failover {
        options.with_no_failover()
    } else {
        options
    }
}

fn warn_if_fast_mode_has_no_codex_review_candidate(
    effective_fast_mode: bool,
    warn_no_codex_fast_mode: bool,
    candidates: &[(ToolName, Option<String>)],
) {
    if effective_fast_mode
        && warn_no_codex_fast_mode
        && !candidates.iter().any(|(tool, _)| *tool == ToolName::Codex)
    {
        eprintln!(
            "warning: --fast-but-more-cost only affects codex; no codex review attempt is in the resolved candidate set."
        );
    }
}

struct ReviewTierCandidateRequest<'a> {
    initial_tool: ToolName,
    initial_model_spec: Option<&'a str>,
    tier_name: Option<&'a str>,
    project_config: Option<&'a ProjectConfig>,
    global_config: Option<&'a GlobalConfig>,
    tier_fallback_enabled: bool,
    no_failover: bool,
    tier_preference_order: &'a [String],
}

fn review_ordered_tier_candidates(
    request: ReviewTierCandidateRequest<'_>,
) -> Vec<(ToolName, Option<String>)> {
    ordered_tier_candidates(
        request.initial_tool,
        request.initial_model_spec,
        request.tier_name,
        request.project_config,
        request.global_config,
        request.tier_fallback_enabled && !request.no_failover,
        request.tier_preference_order,
    )
}

#[allow(clippy::too_many_arguments)]
#[cfg(test)]
pub(crate) async fn execute_review(
    tool: ToolName,
    prompt: String,
    session: Option<String>,
    model: Option<String>,
    tier_model_spec: Option<String>,
    tier_name: Option<String>,
    tier_fallback_enabled: bool,
    thinking: Option<String>,
    description: String,
    project_root: &Path,
    project_config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
    pre_session_hook: Option<csa_hooks::PreSessionHookInvocation>,
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
    extra_readable: &[PathBuf],
    error_marker_scan_override: Option<bool>,
) -> Result<ReviewExecutionOutcome> {
    let startup_env = StartupSubtreeEnv::default();
    execute_review_with_tier_filter(
        tool,
        prompt,
        session,
        model,
        tier_model_spec,
        tier_name,
        tier_fallback_enabled,
        Vec::new(),
        thinking,
        description,
        project_root,
        project_config,
        global_config,
        pre_session_hook,
        review_routing,
        stream_mode,
        idle_timeout_seconds,
        initial_response_timeout_seconds,
        force_override_user_config,
        force_ignore_tier_setting,
        no_failover,
        None,
        false,
        false,
        no_fs_sandbox,
        readonly_project_root,
        extra_writable,
        extra_readable,
        error_marker_scan_override,
        0,
        &startup_env,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn execute_review_with_tier_filter(
    tool: ToolName,
    prompt: String,
    session: Option<String>,
    model: Option<String>,
    tier_model_spec: Option<String>,
    tier_name: Option<String>,
    tier_fallback_enabled: bool,
    tier_preference_order: Vec<String>,
    thinking: Option<String>,
    description: String,
    project_root: &Path,
    project_config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
    pre_session_hook: Option<csa_hooks::PreSessionHookInvocation>,
    review_routing: ReviewRoutingMetadata,
    stream_mode: csa_process::StreamMode,
    idle_timeout_seconds: u64,
    initial_response_timeout_seconds: Option<u64>,
    force_override_user_config: bool,
    force_ignore_tier_setting: bool,
    no_failover: bool,
    build_jobs: Option<u32>,
    fast_but_more_cost: bool,
    warn_no_codex_fast_mode: bool,
    no_fs_sandbox: bool,
    readonly_project_root: bool,
    extra_writable: &[PathBuf],
    extra_readable: &[PathBuf],
    error_marker_scan_override: Option<bool>,
    current_depth: u32,
    startup_env: &StartupSubtreeEnv,
) -> Result<ReviewExecutionOutcome> {
    let execution_started_at = Utc::now();
    if session.is_none()
        && let Some(inherited_session_id) = startup_env.session_id()
    {
        warn!(
            inherited_session_id = %inherited_session_id,
            "Ignoring inherited CSA_SESSION_ID for `csa review`; pass --session to resume explicitly"
        );
    }
    let effective_fast_mode = fast_but_more_cost
        || project_config
            .and_then(|c| c.tools.get("codex"))
            .and_then(|t| t.fast_mode)
            .unwrap_or(false)
        || global_config
            .tools
            .get("codex")
            .and_then(|t| t.fast_mode)
            .unwrap_or(false);
    let candidates = review_ordered_tier_candidates(ReviewTierCandidateRequest {
        initial_tool: tool,
        initial_model_spec: tier_model_spec.as_deref(),
        tier_name: tier_name.as_deref(),
        project_config,
        global_config: Some(global_config),
        tier_fallback_enabled,
        no_failover,
        tier_preference_order: &tier_preference_order,
    });
    warn_if_fast_mode_has_no_codex_review_candidate(
        effective_fast_mode,
        warn_no_codex_fast_mode,
        &candidates,
    );
    let mut failures = Vec::new();
    let mut reset_windows = Vec::new();

    for (attempt_index, (attempt_tool, attempt_model_spec)) in candidates.iter().enumerate() {
        let attempt_started_at = std::time::Instant::now();
        let enforce_tier =
            tier_name.is_some() && attempt_model_spec.is_some() && !force_ignore_tier_setting;
        let mut executor = crate::pipeline::build_and_validate_executor(
            attempt_tool,
            attempt_model_spec.as_deref(),
            model.as_deref(),
            thinking.as_deref(),
            crate::pipeline::ConfigRefs {
                project: project_config,
                global: Some(global_config),
            },
            enforce_tier,
            force_override_user_config,
            false,
        )
        .await?;
        if effective_fast_mode {
            executor.enable_codex_fast_mode();
        }

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
            prompt.clone()
        };
        if let Some(guard) =
            crate::pipeline::prompt_guard::anti_recursion_guard(project_config, current_depth)
        {
            effective_prompt = format!("{guard}\n\n{effective_prompt}");
        }

        let base_env_owned = global_config.build_execution_env(
            executor.tool_name(),
            review_execution_env_options(no_failover),
        );
        let mut base_env_owned = base_env_owned;
        crate::build_jobs_env::apply_build_jobs_env(&mut base_env_owned, build_jobs);
        // #1741: keep a pinned subtree pinned through the reviewer child so a
        // nested Layer-N+1 call from the reviewer does not re-select the tier
        // default. Mirrors csa run (run_cmd_attempt.rs). `attempt_model_spec` is
        // the spec this reviewer is actually running as. The pin is carried
        // out-of-band as a typed value (no-op unless force_ignore_tier_setting +
        // a non-empty spec) and applied by the executor's trusted channel —
        // never via the env map, so no request/config env can spoof it.
        let subtree_pin = crate::run_cmd_model_pin::resolve_subtree_model_pin(
            attempt_model_spec.as_deref(),
            force_ignore_tier_setting,
            no_failover,
        );
        let extra_env_owned =
            with_readonly_session_env(base_env_owned.as_ref(), review_prompt_is_readonly(&prompt));
        let _slot_guard = crate::pipeline::acquire_slot(&executor, global_config)?;

        let mut execution = match execute_review_once_with_artifact_guard(
            &executor,
            attempt_tool,
            &effective_prompt,
            session.clone(),
            description.clone(),
            project_root,
            project_config,
            extra_env_owned.as_ref(),
            subtree_pin.as_ref(),
            tier_name.as_deref(),
            global_config,
            pre_session_hook.clone(),
            stream_mode,
            idle_timeout_seconds,
            initial_response_timeout_seconds,
            no_fs_sandbox,
            readonly_project_root,
            extra_writable,
            extra_readable,
            error_marker_scan_override,
            startup_env,
        )
        .await
        {
            Ok(execution) => execution,
            Err(err) => {
                let error_text = format!("{err:#}");
                if tier_fallback_enabled
                    && candidates.len() > 1
                    && let Some(detected) = classify_review_failover_error(
                        *attempt_tool,
                        attempt_model_spec.as_deref(),
                        &error_text,
                        Some(attempt_started_at.elapsed()),
                    )
                {
                    let model_label = attempt_model_spec
                        .clone()
                        .unwrap_or_else(|| attempt_tool.as_str().to_string());
                    if let Some(reset_after) = parse_backend_reset_duration(&error_text) {
                        reset_windows.push(reset_after);
                    }
                    failures.push(TierAttemptFailure {
                        model_spec: model_label.clone(),
                        reason: detected.reason.clone(),
                        quota_exhausted: detected.quota_exhausted,
                    });
                    warn!(
                        failed_tool = %attempt_tool,
                        failed_model = %model_label,
                        reason = %detected.reason,
                        attempt = attempt_index + 1,
                        total = candidates.len(),
                        "Review tier model failed before execution completed; advancing to next configured model"
                    );
                    if attempt_index + 1 < candidates.len() {
                        // Ensure a result.toml exists for the failed attempt so the
                        // superseded marker can be written, then retire the session to
                        // prevent "Failed" from appearing in `csa session list` during
                        // the failover window (#1475).
                        maybe_synthesize_missing_review_result(
                            project_root,
                            *attempt_tool,
                            execution_started_at,
                            &err,
                        );
                        if let Some(session_id) = extract_meta_session_id_from_error(&err) {
                            retire_tier_failover_session(project_root, &session_id);
                        }
                        continue;
                    }
                    maybe_synthesize_missing_review_result(
                        project_root,
                        *attempt_tool,
                        execution_started_at,
                        &err,
                    );
                    if let Some(session_id) = extract_meta_session_id_from_error(&err) {
                        let fallback_chain =
                            crate::tier_model_fallback::build_fallback_chain_for_result(
                                project_config,
                                tier_name.as_deref(),
                                &failures,
                                // All tier models failed: no winner, so persist
                                // the full chain.
                                None,
                                &tier_preference_order,
                            );
                        persist_fallback_result_fields(
                            project_root,
                            &session_id,
                            tool,
                            *attempt_tool,
                            fallback_reason_for_result(&failures),
                        );
                        persist_review_routing_artifact_with_fallback_chain(
                            project_root,
                            &session_id,
                            &review_routing,
                            &fallback_chain,
                        );
                        persist_fallback_chain(
                            project_root,
                            &session_id,
                            tool,
                            *attempt_tool,
                            fallback_chain,
                        );
                    }
                    let failure_reason = format_all_models_failed_reason_with_reset(
                        tier_name.as_deref(),
                        &failures,
                        earliest_backend_reset_window(&reset_windows),
                    );
                    return Ok(ReviewExecutionOutcome {
                        execution: crate::pipeline::SessionExecutionResult {
                            execution: csa_process::ExecutionResult {
                                exit_code: 1,
                                output: String::new(),
                                stderr_output: error_text,
                                summary: "Review unavailable".to_string(),
                                peak_memory_mb: None,
                                ..Default::default()
                            },
                            meta_session_id: extract_meta_session_id_from_error(&err)
                                .unwrap_or_else(|| "unknown".to_string()),
                            provider_session_id: None,
                            changed_paths: None,
                        },
                        persistable_session_id: extract_meta_session_id_from_error(&err),
                        executed_tool: *attempt_tool,
                        status_reason: Some("tier_models_unavailable".to_string()),
                        forced_decision: Some(ReviewDecision::Unavailable),
                        routed_to: None,
                        primary_failure: chain_failure_reasons(&failures),
                        failure_reason,
                    });
                }
                maybe_synthesize_missing_review_result(
                    project_root,
                    *attempt_tool,
                    execution_started_at,
                    &err,
                );
                return Err(err);
            }
        };

        repair_completed_review_restriction_result(project_root, *attempt_tool, &mut execution)?;

        let mut status_reason = None;
        if let Some(kind) = detect_tool_review_failure(
            *attempt_tool,
            &execution.execution.output,
            &execution.execution.stderr_output,
        ) {
            let retry_env = (!no_failover)
                .then(|| build_gemini_api_key_retry_env(extra_env_owned.as_ref()))
                .flatten();
            warn!(
                tool = %attempt_tool,
                reason = kind.status_reason(),
                retry_attempted = retry_env.is_some(),
                "Detected Gemini OAuth browser prompt during review execution"
            );

            if let Some(api_key_env) = retry_env {
                let mut retried = match execute_review_once_with_artifact_guard(
                    &executor,
                    attempt_tool,
                    &effective_prompt,
                    session.clone(),
                    description.clone(),
                    project_root,
                    project_config,
                    Some(&api_key_env),
                    subtree_pin.as_ref(),
                    tier_name.as_deref(),
                    global_config,
                    pre_session_hook.clone(),
                    stream_mode,
                    idle_timeout_seconds,
                    initial_response_timeout_seconds,
                    no_fs_sandbox,
                    readonly_project_root,
                    extra_writable,
                    extra_readable,
                    error_marker_scan_override,
                    startup_env,
                )
                .await
                {
                    Ok(execution) => execution,
                    Err(err) => {
                        maybe_synthesize_missing_review_result(
                            project_root,
                            *attempt_tool,
                            execution_started_at,
                            &err,
                        );
                        return Err(err);
                    }
                };
                repair_completed_review_restriction_result(
                    project_root,
                    *attempt_tool,
                    &mut retried,
                )?;

                if let Some(retry_kind) = detect_tool_review_failure(
                    *attempt_tool,
                    &retried.execution.output,
                    &retried.execution.stderr_output,
                ) {
                    classify_review_failure_result(
                        project_root,
                        *attempt_tool,
                        &mut retried,
                        retry_kind,
                    )?;
                    status_reason = Some(retry_kind.status_reason().to_string());
                    execution = retried;
                } else {
                    execution = retried;
                }
            } else {
                classify_review_failure_result(project_root, *attempt_tool, &mut execution, kind)?;
                status_reason = Some(kind.status_reason().to_string());
            }
        }

        let failure_reason = classify_review_failover_reason(
            *attempt_tool,
            attempt_model_spec.as_deref(),
            &execution,
            status_reason.as_deref(),
            Some(attempt_started_at.elapsed()),
        );

        if tier_fallback_enabled
            && candidates.len() > 1
            && let Some(failure) = failure_reason
        {
            let model_label = attempt_model_spec
                .clone()
                .unwrap_or_else(|| attempt_tool.as_str().to_string());
            let reason = failure.reason.clone();
            if let Some(reset_after) = parse_backend_reset_duration(&format!(
                "{}\n{}",
                execution.execution.stderr_output, execution.execution.output
            )) {
                reset_windows.push(reset_after);
            }
            failures.push(TierAttemptFailure {
                model_spec: model_label.clone(),
                reason: reason.clone(),
                quota_exhausted: failure.quota_exhausted,
            });
            warn!(
                failed_tool = %attempt_tool,
                failed_model = %model_label,
                reason = %reason,
                attempt = attempt_index + 1,
                total = candidates.len(),
                "Review tier model failed; advancing to next configured model"
            );
            if attempt_index + 1 == candidates.len() {
                let session_dir = get_session_dir(project_root, &execution.meta_session_id)?;
                ensure_review_summary_artifact(&session_dir, &execution.execution.output)?;
                persist_fallback_result_fields(
                    project_root,
                    &execution.meta_session_id,
                    tool,
                    *attempt_tool,
                    fallback_reason_for_result(&failures),
                );
                let fallback_chain = crate::tier_model_fallback::build_fallback_chain_for_result(
                    project_config,
                    tier_name.as_deref(),
                    &failures,
                    // Every tier model failed: no winner, persist full chain.
                    None,
                    &tier_preference_order,
                );
                persist_review_routing_artifact_with_fallback_chain(
                    project_root,
                    &execution.meta_session_id,
                    &review_routing,
                    &fallback_chain,
                );
                persist_fallback_chain(
                    project_root,
                    &execution.meta_session_id,
                    tool,
                    *attempt_tool,
                    fallback_chain,
                );
                let persistable_session_id = Some(execution.meta_session_id.clone());
                return Ok(ReviewExecutionOutcome {
                    execution,
                    persistable_session_id,
                    executed_tool: *attempt_tool,
                    status_reason: Some("tier_models_unavailable".to_string()),
                    forced_decision: Some(ReviewDecision::Unavailable),
                    routed_to: None,
                    primary_failure: chain_failure_reasons(&failures),
                    failure_reason: format_all_models_failed_reason_with_reset(
                        tier_name.as_deref(),
                        &failures,
                        earliest_backend_reset_window(&reset_windows),
                    ),
                });
            }
            // Retire the failed intermediate session so `csa session list` shows
            // "Retired" rather than "Failed" during the failover window (#1475).
            retire_tier_failover_session(project_root, &execution.meta_session_id);
            continue;
        }

        let session_dir = get_session_dir(project_root, &execution.meta_session_id)?;
        ensure_review_summary_artifact(&session_dir, &execution.execution.output)?;
        persist_fallback_result_fields(
            project_root,
            &execution.meta_session_id,
            tool,
            *attempt_tool,
            fallback_reason_for_result(&failures),
        );
        let fallback_chain = crate::tier_model_fallback::build_fallback_chain_for_result(
            project_config,
            tier_name.as_deref(),
            &failures,
            // The winning model: bounds the persisted chain to before-winner
            // skips so a first-choice success omits never-reached tier
            // models (#1714).
            attempt_model_spec.as_deref(),
            &tier_preference_order,
        );
        persist_review_routing_artifact_with_fallback_chain(
            project_root,
            &execution.meta_session_id,
            &review_routing,
            &fallback_chain,
        );
        persist_fallback_chain(
            project_root,
            &execution.meta_session_id,
            tool,
            *attempt_tool,
            fallback_chain,
        );
        let routed_to = (attempt_tool != &tool
            || attempt_model_spec.as_deref() != tier_model_spec.as_deref())
        .then(|| {
            attempt_model_spec.clone().or_else(|| {
                tier_name.as_deref().and_then(|resolved_tier_name| {
                    project_config.and_then(|cfg| {
                        cfg.tiers.get(resolved_tier_name).and_then(|tier| {
                            tier.models.iter().find_map(|model_spec| {
                                model_spec
                                    .split('/')
                                    .next()
                                    .filter(|tool_name| *tool_name == attempt_tool.as_str())
                                    .map(|_| model_spec.clone())
                            })
                        })
                    })
                })
            })
        })
        .flatten();
        let persistable_session_id = Some(execution.meta_session_id.clone());
        return Ok(ReviewExecutionOutcome {
            execution,
            persistable_session_id,
            executed_tool: *attempt_tool,
            status_reason,
            forced_decision: None,
            routed_to,
            // #1852: reaching here means THIS attempt's review SUCCEEDED. The
            // earlier `failures` are the tools we failed over FROM, not a
            // terminal failure of the review — recording them as
            // `primary_failure` mislabels a success-via-fallback verdict as
            // auth/quota-unavailable and (via `requires_fail_closed_verdict`)
            // drags the verdict toward fail-closed. The failover provenance is
            // already preserved in the persisted `fallback_chain`, `routed_to`,
            // and `fallback_reason`. Only the all-candidates-failed return
            // (above) carries `chain_failure_reasons` as the terminal class.
            primary_failure: None,
            failure_reason: None,
        });
    }

    unreachable!("tier candidate list is never empty")
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
    subtree_pin: Option<&csa_core::env::SubtreeModelPin>,
    tier_name: Option<&str>,
    global_config: &GlobalConfig,
    pre_session_hook: Option<csa_hooks::PreSessionHookInvocation>,
    stream_mode: csa_process::StreamMode,
    idle_timeout_seconds: u64,
    initial_response_timeout_seconds: Option<u64>,
    no_fs_sandbox: bool,
    readonly_project_root: bool,
    extra_writable: &[PathBuf],
    extra_readable: &[PathBuf],
    error_marker_scan_override: Option<bool>,
    startup_env: &StartupSubtreeEnv,
) -> Result<crate::pipeline::SessionExecutionResult> {
    crate::pipeline::execute_with_session_and_meta_with_parent_source(
        executor,
        tool,
        effective_prompt,
        OutputFormat::Json,
        session,
        false,
        Some(description),
        None,
        project_root,
        project_config,
        extra_env,
        subtree_pin,
        false,
        Some(REVIEWER_SUB_SESSION_TASK_TYPE),
        tier_name,
        None,
        stream_mode,
        idle_timeout_seconds,
        initial_response_timeout_seconds,
        None,
        None,
        Some(global_config),
        pre_session_hook,
        crate::pipeline::ParentSessionSource::ExplicitOnly,
        crate::pipeline::SessionCreationMode::DaemonManaged,
        no_fs_sandbox,
        readonly_project_root,
        extra_writable,
        extra_readable,
        error_marker_scan_override,
        false,
        startup_env,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn execute_review_once_with_artifact_guard(
    executor: &Executor,
    tool: &ToolName,
    effective_prompt: &str,
    session: Option<String>,
    description: String,
    project_root: &Path,
    project_config: Option<&ProjectConfig>,
    extra_env: Option<&HashMap<String, String>>,
    subtree_pin: Option<&csa_core::env::SubtreeModelPin>,
    tier_name: Option<&str>,
    global_config: &GlobalConfig,
    pre_session_hook: Option<csa_hooks::PreSessionHookInvocation>,
    stream_mode: csa_process::StreamMode,
    idle_timeout_seconds: u64,
    initial_response_timeout_seconds: Option<u64>,
    no_fs_sandbox: bool,
    readonly_project_root: bool,
    extra_writable: &[PathBuf],
    extra_readable: &[PathBuf],
    error_marker_scan_override: Option<bool>,
    startup_env: &StartupSubtreeEnv,
) -> Result<crate::pipeline::SessionExecutionResult> {
    let invocation_started_at = Utc::now();
    match execute_review_once(
        executor,
        tool,
        effective_prompt,
        session,
        description,
        project_root,
        project_config,
        extra_env,
        subtree_pin,
        tier_name,
        global_config,
        pre_session_hook,
        stream_mode,
        idle_timeout_seconds,
        initial_response_timeout_seconds,
        no_fs_sandbox,
        readonly_project_root,
        extra_writable,
        extra_readable,
        error_marker_scan_override,
        startup_env,
    )
    .await
    {
        Ok(mut execution) => {
            enforce_review_artifact_contract(
                project_root,
                tool,
                invocation_started_at,
                Some(&mut execution),
                None,
            )?;
            Ok(execution)
        }
        Err(err) => {
            enforce_review_artifact_contract(
                project_root,
                tool,
                invocation_started_at,
                None,
                Some(&err),
            )?;
            Err(err)
        }
    }
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

#[cfg(test)]
#[path = "review_cmd_execute_readonly_tests.rs"]
mod readonly_tests;

#[cfg(test)]
#[path = "review_cmd_execute_guard_tests.rs"]
mod guard_tests;

#[cfg(test)]
#[path = "review_cmd_execute_tier_tests.rs"]
mod tier_tests;

#[cfg(test)]
#[path = "review_cmd_execute_redirect_guard_tests.rs"]
mod redirect_guard_tests;
