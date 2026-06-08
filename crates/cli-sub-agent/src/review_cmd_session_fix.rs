use std::path::Path;

use anyhow::{Context, Result};
use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::ToolName;

use super::resolve::selection::ResolvedReviewSelection;
use crate::cli::ReviewArgs;
use crate::review_consensus::review_iteration_resolver;

#[cfg(test)]
#[path = "review_cmd_tests_session_fix.rs"]
mod tests;

pub(super) struct SessionFixTool {
    pub(super) session_id: String,
    pub(super) tool: Option<ToolName>,
}

pub(super) struct SelectionResolutionCtx<'a> {
    pub(super) args: &'a ReviewArgs,
    pub(super) project_config: Option<&'a ProjectConfig>,
    pub(super) global_config: &'a GlobalConfig,
    pub(super) parent_tool: Option<&'a str>,
    pub(super) project_root: &'a Path,
    pub(super) effective_tier: Option<&'a str>,
    pub(super) selection_tool: Option<ToolName>,
    pub(super) direct_tool_requested: bool,
    pub(super) session_fix: Option<&'a SessionFixTool>,
    pub(super) review_description: &'a str,
}

pub(super) struct SelectionToolResolution {
    pub(super) selection_tool: Option<ToolName>,
    pub(super) direct_tool_requested: bool,
    pub(super) session_fix: Option<SessionFixTool>,
}

pub(super) fn resolve_selection_tool(
    args: &ReviewArgs,
    project_root: &Path,
    args_tool: Option<ToolName>,
) -> Result<SelectionToolResolution> {
    let session_fix = resolve_session_fix_tool(args, project_root)?;
    validate_session_fix_explicit_tool(args, session_fix.as_ref())?;
    let selection_tool = args_tool.or_else(|| session_fix.as_ref().and_then(|fix| fix.tool));
    Ok(SelectionToolResolution {
        selection_tool,
        direct_tool_requested: args_tool.is_some(),
        session_fix,
    })
}

pub(super) fn resolve_selection_or_persist_error(
    ctx: SelectionResolutionCtx<'_>,
) -> Result<ResolvedReviewSelection> {
    let resolved = match super::resolve::resolve_review_selection(
        ctx.selection_tool,
        ctx.args.model_spec.as_deref(),
        ctx.project_config,
        ctx.global_config,
        ctx.parent_tool,
        ctx.project_root,
        ctx.args.force_override_user_config,
        ctx.effective_tier,
        ctx.args.force_ignore_tier_setting,
        ctx.direct_tool_requested,
    ) {
        Ok(resolved) => resolved,
        Err(err) => return Err(persist_selection_error(&ctx, err)),
    };
    validate_session_fix_resolved_tool(ctx.session_fix, resolved.tool)
        .map_err(|err| persist_selection_error(&ctx, err))?;
    Ok(resolved)
}

pub(super) fn effective_no_failover_for_session_fix(
    no_failover: bool,
    session_fix: Option<&SessionFixTool>,
) -> bool {
    no_failover || session_fix.and_then(|fix| fix.tool).is_some()
}

fn persist_selection_error(ctx: &SelectionResolutionCtx<'_>, err: anyhow::Error) -> anyhow::Error {
    crate::session_guard::persist_pre_exec_error_result(crate::session_guard::PreExecErrorCtx {
        project_root: ctx.project_root,
        session_id: super::prior_rounds::review_pre_exec_session_id(ctx.args),
        description: Some(ctx.review_description),
        parent: None,
        tool_name: super::prior_rounds::explicit_review_tool(ctx.args).map(|tool| tool.as_str()),
        task_type: Some("review"),
        tier_name: ctx.effective_tier,
        error: err,
    })
}

fn validate_session_fix_resolved_tool(
    session_fix: Option<&SessionFixTool>,
    resolved_tool: ToolName,
) -> Result<()> {
    let Some(session_fix) = session_fix else {
        return Ok(());
    };
    let Some(session_tool) = session_fix.tool else {
        return Ok(());
    };
    if resolved_tool != session_tool {
        anyhow::bail!(
            "`csa review --session {} --fix` must use the original review tool '{}'; \
             tier/model routing resolved '{}'.",
            session_fix.session_id,
            session_tool,
            resolved_tool
        );
    }
    Ok(())
}

pub(super) fn resolve_session_fix_tool(
    args: &ReviewArgs,
    project_root: &Path,
) -> Result<Option<SessionFixTool>> {
    if !args.fix {
        return Ok(None);
    }
    let Some(session_ref) = args.session.as_deref() else {
        return Ok(None);
    };

    let resolution = csa_session::resolve_fork_source(project_root, session_ref)
        .with_context(|| format!("failed to resolve --session {session_ref} for review --fix"))?;
    let session_id = resolution.meta_session_id;
    let tool = infer_session_tool(project_root, &session_id)?;

    if tool.is_none() && args.tool.is_none() && args.model_spec.is_none() {
        anyhow::bail!(
            "Cannot infer review tool for `csa review --session {session_ref} --fix`: \
             session {session_id} has no metadata.toml tool, review_meta.json tool, or result.toml tool. \
             Pass --tool <tool> or choose a review session with tool metadata."
        );
    }

    Ok(Some(SessionFixTool { session_id, tool }))
}

pub(super) fn validate_session_fix_explicit_tool(
    args: &ReviewArgs,
    session_fix: Option<&SessionFixTool>,
) -> Result<()> {
    let Some(session_fix) = session_fix else {
        return Ok(());
    };
    let Some(session_tool) = session_fix.tool else {
        return Ok(());
    };

    if let Some(cli_tool) = args.tool
        && cli_tool != session_tool
    {
        anyhow::bail!(
            "`csa review --session {} --fix` must use the original review tool '{}'; \
             explicit --tool '{}' would violate the session tool lock.",
            session_fix.session_id,
            session_tool,
            cli_tool
        );
    }

    if let Some(spec_tool) = args
        .model_spec
        .as_deref()
        .and_then(|spec| spec.split('/').next())
        .map(crate::run_helpers::parse_tool_name)
        .transpose()?
        && spec_tool != session_tool
    {
        anyhow::bail!(
            "`csa review --session {} --fix` must use the original review tool '{}'; \
             --model-spec selects '{}'.",
            session_fix.session_id,
            session_tool,
            spec_tool
        );
    }

    Ok(())
}

pub(crate) fn validate_session_fix_before_daemon(args: &ReviewArgs) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(args.cd.as_deref())?;
    let project_config = ProjectConfig::load(&project_root)?;
    let global_config = GlobalConfig::load()?;
    let parent_tool =
        crate::run_helpers::resolve_tool(crate::run_helpers::detect_parent_tool(), &global_config);

    resolve_session_fix_selection(
        args,
        &project_root,
        project_config.as_ref(),
        &global_config,
        parent_tool.as_deref(),
    )?;

    Ok(())
}

pub(super) fn resolve_session_fix_selection(
    args: &ReviewArgs,
    project_root: &Path,
    project_config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
    parent_tool: Option<&str>,
) -> Result<Option<ToolName>> {
    let Some(session_fix) = resolve_session_fix_tool(args, project_root)? else {
        return Ok(None);
    };
    validate_session_fix_explicit_tool(args, Some(&session_fix))?;

    let (effective_tier, args_tool) =
        super::resolve::resolve_review_effective_tier(args, project_config)?;
    let selection_tool = args_tool.or(session_fix.tool);
    let resolved = super::resolve::resolve_review_selection(
        selection_tool,
        args.model_spec.as_deref(),
        project_config,
        global_config,
        parent_tool,
        project_root,
        args.force_override_user_config,
        effective_tier.as_deref(),
        args.force_ignore_tier_setting,
        args_tool.is_some(),
    )?;
    validate_session_fix_resolved_tool(Some(&session_fix), resolved.tool)?;

    csa_session::resolve_resume_session(
        project_root,
        &session_fix.session_id,
        resolved.tool.as_str(),
    )
    .with_context(|| {
        format!(
            "resolved review --session --fix tool '{}' cannot access session {}",
            resolved.tool, session_fix.session_id
        )
    })?;

    Ok(Some(resolved.tool))
}

fn infer_session_tool(project_root: &Path, session_id: &str) -> Result<Option<ToolName>> {
    if let Some(tool) = csa_session::load_metadata(project_root, session_id)
        .with_context(|| format!("failed to load metadata for review session {session_id}"))?
        .map(|metadata| metadata.tool)
        && let Some(tool) = parse_recorded_tool("metadata.toml", session_id, &tool)?
    {
        return Ok(Some(tool));
    }

    if let Some(meta) = review_iteration_resolver::load_review_meta(project_root, session_id)?
        && let Some(tool) = parse_recorded_tool("review_meta.json", session_id, &meta.tool)?
    {
        return Ok(Some(tool));
    }

    if let Some(result) = csa_session::load_result(project_root, session_id)
        .with_context(|| format!("failed to load result.toml for review session {session_id}"))?
        && let Some(tool) = parse_recorded_tool("result.toml", session_id, &result.tool)?
    {
        return Ok(Some(tool));
    }

    Ok(None)
}

fn parse_recorded_tool(source: &str, session_id: &str, tool: &str) -> Result<Option<ToolName>> {
    if tool.trim().is_empty() || tool == "unknown" {
        return Ok(None);
    }
    crate::run_helpers::parse_tool_name(tool)
        .with_context(|| {
            format!("{source} for review session {session_id} records invalid tool '{tool}'")
        })
        .map(Some)
}
