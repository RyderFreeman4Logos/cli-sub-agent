//! Caller-confirmed single-finding fix path for failed `csa review` sessions.

use std::path::Path;

use anyhow::{Context, Result};
use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::{ReviewDecision, ToolName};
use csa_session::{TaskContext, ToolState};
use serde::Deserialize;

use crate::cli::ReviewArgs;
use crate::startup_env::StartupSubtreeEnv;

use super::output::sanitize_review_output;
use super::post_review::CONFIRM_THEN_FIX_FINDING_ACTION;

#[path = "review_cmd_fix_finding_prompt.rs"]
mod prompt;
use prompt::{
    build_fix_finding_prompt, ensure_fix_finding_prompt_available,
    resolve_fix_finding_prompt_before_daemon,
};

const FIX_FINDING_TASK_TYPE: &str = "review_fix_finding";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FixFindingRoute {
    pub(super) session_id: String,
    pub(super) tool: ToolName,
    pub(super) model_spec: Option<String>,
    pub(super) model: Option<String>,
    pub(super) thinking: Option<String>,
    pub(super) provider_session_id: String,
}

#[derive(Debug, Deserialize)]
struct SuggestionEnvelope {
    suggestion: SuggestionFile,
}

#[derive(Debug, Deserialize)]
struct SuggestionFile {
    action: String,
    session_id: Option<String>,
    tool: Option<String>,
    model_spec: Option<String>,
    model: Option<String>,
    thinking: Option<String>,
    provider_session_id: Option<String>,
}

pub(crate) fn validate_fix_finding_before_daemon(args: &ReviewArgs) -> Result<()> {
    if !args.fix_finding {
        return Ok(());
    }

    let project_root = crate::pipeline::determine_project_root(args.cd.as_deref())?;
    let effective_config = csa_config::EffectiveConfig::load(&project_root)?;
    let project_config = effective_config.project;
    let global_config = effective_config.global;
    let session_ref = args
        .session
        .as_deref()
        .context("--fix-finding requires --session <failed-review-session-id>")?;
    let route = load_fix_finding_route(&project_root, session_ref)?;
    validate_fix_finding_route(
        &project_root,
        &route,
        project_config.as_ref(),
        &global_config,
    )?;
    // Fail closed with documented prompt requirements before daemon spawn / SA
    // guard. Prefer deriving an unambiguous source finding without consuming
    // stdin (daemon children resolve the final prompt body themselves).
    ensure_fix_finding_prompt_available(args, &project_root, &route.session_id)?;
    Ok(())
}

pub(crate) async fn handle_fix_finding(
    args: ReviewArgs,
    project_root: &Path,
    current_depth: u32,
    startup_env: &StartupSubtreeEnv,
) -> Result<i32> {
    let Some((config, global_config, model_catalog, _project_completion_policy)) =
        crate::pipeline::load_and_validate(project_root, current_depth)?
    else {
        return Ok(1);
    };
    let session_ref = args
        .session
        .as_deref()
        .context("--fix-finding requires --session <failed-review-session-id>")?;
    let route = load_fix_finding_route(project_root, session_ref)?;
    validate_fix_finding_route(project_root, &route, config.as_ref(), &global_config)?;

    let caller_prompt =
        resolve_fix_finding_prompt_before_daemon(&args, project_root, &route.session_id)?;
    let prompt = build_fix_finding_prompt(&caller_prompt);
    let fix_session_id = create_fix_finding_session(project_root, &route)?;
    let stream_mode =
        super::resolve::resolve_review_stream_mode(args.stream_stdout, args.no_stream_stdout);
    let idle_timeout_seconds = crate::pipeline::resolve_effective_idle_timeout_seconds(
        config.as_ref(),
        args.idle_timeout,
        args.timeout,
    );
    let initial_response_timeout_seconds =
        crate::pipeline::resolve_effective_initial_response_timeout_for_tool(
            config.as_ref(),
            args.initial_response_timeout,
            args.idle_timeout,
            args.timeout,
            route.tool.as_str(),
        );
    let review_routing =
        crate::review_routing::detect_review_routing_metadata(project_root, config.as_ref());

    let fix_future = super::execute::execute_review_with_tier_filter(
        route.tool,
        prompt,
        Some(fix_session_id),
        route.model.clone(),
        route.model_spec.clone(),
        None,
        false,
        Vec::new(),
        route.thinking.clone(),
        format!("fix finding from review {}", route.session_id),
        project_root,
        config.as_ref(),
        &global_config,
        &model_catalog,
        None,
        review_routing,
        stream_mode,
        idle_timeout_seconds,
        initial_response_timeout_seconds,
        args.force_override_user_config,
        true,
        true,
        None,
        args.build_jobs,
        args.fast_but_more_cost,
        false,
        args.no_fs_sandbox,
        args.allow_user_daemon_ipc,
        false,
        &args.extra_writable,
        &args.extra_readable,
        args.error_marker_scan_override(),
        args.resource_overrides(),
        current_depth,
        crate::pipeline::SessionCreationMode::DaemonManaged,
        startup_env,
    );

    let result = if let Some(timeout_secs) = args.timeout {
        match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), fix_future).await {
            Ok(inner) => inner?,
            Err(_) => {
                anyhow::bail!("Fix-finding pass aborted: --timeout {timeout_secs}s exceeded.");
            }
        }
    } else {
        fix_future.await?
    };

    print!(
        "{}",
        sanitize_review_output(&result.execution.execution.output)
    );
    println!(
        "<!-- CSA:CALLER_HINT action=\"review_next_round\" next_review=\"fresh_session\" \
         reason=\"fix-finding resumed reviewer session; the next review round must be independent\" -->"
    );
    Ok(result.execution.execution.exit_code)
}

fn create_fix_finding_session(project_root: &Path, route: &FixFindingRoute) -> Result<String> {
    let description = format!("fix finding from review {}", route.session_id);
    let mut session = csa_session::create_session_fresh(
        project_root,
        Some(&description),
        Some(&route.session_id),
        Some(route.tool.as_str()),
    )
    .with_context(|| {
        format!(
            "failed to create fix-finding session for review {}",
            route.session_id
        )
    })?;
    session.task_context = TaskContext {
        task_type: Some(FIX_FINDING_TASK_TYPE.to_string()),
        tier_name: None,
    };
    session.tools.insert(
        route.tool.as_str().to_string(),
        ToolState {
            provider_session_id: Some(route.provider_session_id.clone()),
            last_action_summary: description,
            last_exit_code: 0,
            updated_at: chrono::Utc::now(),
            tool_version: None,
            token_usage: None,
        },
    );
    csa_session::save_session(&session).with_context(|| {
        format!(
            "failed to persist fix-finding session {} for review {}",
            session.meta_session_id, route.session_id
        )
    })?;
    Ok(session.meta_session_id)
}

pub(super) fn load_fix_finding_route(
    project_root: &Path,
    session_ref: &str,
) -> Result<FixFindingRoute> {
    let resolution = csa_session::resolve_fork_source(project_root, session_ref)
        .with_context(|| format!("failed to resolve --session {session_ref} for --fix-finding"))?;
    let session_id = resolution.meta_session_id;
    ensure_failed_review(project_root, &session_id)?;

    let session = csa_session::load_session(project_root, &session_id)
        .with_context(|| format!("failed to load review session {session_id}"))?;
    let session_dir = csa_session::get_session_dir(project_root, &session_id)
        .with_context(|| format!("failed to resolve review session dir for {session_id}"))?;
    let suggestion_path = session_dir.join("output").join("suggestion.toml");
    let suggestion = load_fix_finding_suggestion(&suggestion_path, &session_id)?;
    let tool = parse_suggestion_tool(&suggestion, &session_id)?;
    let provider_session_id = session
        .tools
        .get(tool.as_str())
        .and_then(|state| state.provider_session_id.as_deref())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Cannot run `csa review --fix-finding --session {session_id}`: \
                 state.toml has no provider_session_id for tool '{tool}'. \
                 Refusing to cold-start while claiming reviewer KV-cache reuse."
            )
        })?
        .to_string();

    if let Some(recorded_provider) = suggestion.provider_session_id.as_deref()
        && recorded_provider != provider_session_id
    {
        anyhow::bail!(
            "Cannot preserve exact review session for --fix-finding: \
             suggestion.toml recorded provider_session_id '{recorded_provider}', \
             but state.toml has '{provider_session_id}'."
        );
    }

    let route = FixFindingRoute {
        session_id,
        tool,
        model_spec: suggestion.model_spec,
        model: suggestion.model,
        thinking: suggestion.thinking,
        provider_session_id,
    };
    validate_route_has_exact_model(&route)?;
    Ok(route)
}

fn load_fix_finding_suggestion(path: &Path, session_id: &str) -> Result<SuggestionFile> {
    let contents = std::fs::read_to_string(path).with_context(|| {
        format!(
            "Cannot run --fix-finding for session {session_id}: missing exact-route sidecar {}. \
             Rerun `csa review` to emit a confirm_then_fix_finding suggestion, \
             or use legacy `csa review --session {session_id} --fix`.",
            path.display()
        )
    })?;
    let envelope: SuggestionEnvelope =
        toml::from_str(&contents).with_context(|| format!("failed to parse {}", path.display()))?;
    if envelope.suggestion.action != CONFIRM_THEN_FIX_FINDING_ACTION {
        anyhow::bail!(
            "Cannot run --fix-finding for session {session_id}: suggestion action is '{}', \
             expected '{}'. Rerun `csa review` to capture exact route metadata.",
            envelope.suggestion.action,
            CONFIRM_THEN_FIX_FINDING_ACTION
        );
    }
    if let Some(recorded_session_id) = envelope.suggestion.session_id.as_deref()
        && recorded_session_id != session_id
    {
        anyhow::bail!(
            "Cannot run --fix-finding: suggestion.toml session_id '{recorded_session_id}' \
             does not match resolved review session '{session_id}'."
        );
    }
    Ok(envelope.suggestion)
}

fn parse_suggestion_tool(suggestion: &SuggestionFile, session_id: &str) -> Result<ToolName> {
    let tool = suggestion.tool.as_deref().ok_or_else(|| {
        anyhow::anyhow!(
            "Cannot run --fix-finding for session {session_id}: \
             suggestion.toml does not record the exact review tool."
        )
    })?;
    crate::run_helpers::parse_tool_name(tool).with_context(|| {
        format!("suggestion.toml for review session {session_id} records invalid tool '{tool}'")
    })
}

fn ensure_failed_review(project_root: &Path, session_id: &str) -> Result<()> {
    let Some(meta) = crate::review_consensus::review_iteration_resolver::load_review_meta(
        project_root,
        session_id,
    )?
    else {
        anyhow::bail!(
            "Cannot run --fix-finding for session {session_id}: review_meta.json is missing."
        );
    };
    let decision = meta
        .decision
        .parse::<ReviewDecision>()
        .unwrap_or(ReviewDecision::Uncertain);
    if decision != ReviewDecision::Fail {
        anyhow::bail!(
            "Cannot run --fix-finding for session {session_id}: review decision is '{}', not 'fail'.",
            meta.decision
        );
    }
    Ok(())
}

pub(super) fn validate_route_against_config(
    route: &FixFindingRoute,
    project_config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
) -> Result<()> {
    validate_route_has_exact_model(route)?;
    let recorded_thinking = route_recorded_thinking(route)?;
    let tool_name = route.tool.as_str();
    if let Some(lock) = project_config
        .and_then(|config| config.thinking_lock(tool_name))
        .or_else(|| global_config.thinking_lock(tool_name))
        && Some(lock) != recorded_thinking.as_deref()
    {
        anyhow::bail!(
            "Cannot preserve exact review route for --fix-finding: \
             current thinking_lock for tool '{tool_name}' is '{lock}', \
             but the failed review route recorded thinking '{}'. \
             Remove or align the lock, then retry.",
            recorded_thinking.as_deref().unwrap_or("<missing>")
        );
    }
    Ok(())
}

fn validate_fix_finding_route(
    project_root: &Path,
    route: &FixFindingRoute,
    project_config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
) -> Result<()> {
    validate_route_against_config(route, project_config, global_config)?;
    if super::fix::should_skip_for_readonly_tool(
        project_config,
        route.tool,
        project_root,
        std::slice::from_ref(&route.session_id),
    ) {
        anyhow::bail!(
            "Cannot run --fix-finding for session {}: failed review route uses tool '{}', \
             but current project config sets allow_edit_existing_files=false for that tool. \
             Refusing to resume a write-intended fix pass.",
            route.session_id,
            route.tool
        );
    }
    Ok(())
}

fn validate_route_has_exact_model(route: &FixFindingRoute) -> Result<()> {
    if let Some(model_spec) = route.model_spec.as_deref() {
        validate_model_spec_matches_tool(model_spec, route.tool)?;
        return Ok(());
    }
    if route
        .model
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
        && route
            .thinking
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
    {
        return Ok(());
    }
    anyhow::bail!(
        "Cannot run --fix-finding for session {}: exact model/thinking route is missing. \
         Refusing to resume because a config default or fallback could change the reviewer route.",
        route.session_id
    )
}

fn validate_model_spec_matches_tool(model_spec: &str, tool: ToolName) -> Result<()> {
    let spec_tool = model_spec.split('/').next().unwrap_or_default();
    if spec_tool != tool.as_str() {
        anyhow::bail!(
            "Cannot preserve exact review route for --fix-finding: \
             suggestion tool is '{tool}', but model_spec selects '{spec_tool}'."
        );
    }
    if model_spec.split('/').count() != 4 {
        anyhow::bail!(
            "Cannot preserve exact review route for --fix-finding: \
             model_spec '{model_spec}' is not in tool/provider/model/thinking format."
        );
    }
    Ok(())
}

fn route_recorded_thinking(route: &FixFindingRoute) -> Result<Option<String>> {
    if let Some(model_spec) = route.model_spec.as_deref() {
        let thinking = model_spec
            .rsplit('/')
            .next()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("model_spec '{model_spec}' has no thinking segment"))?;
        return Ok(Some(thinking.to_string()));
    }
    Ok(route.thinking.clone())
}

#[cfg(test)]
#[path = "review_cmd_fix_finding_tests.rs"]
mod tests;
