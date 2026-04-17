use crate::cli::ReviewArgs;
use anyhow::Result;

pub(super) fn load_prior_rounds_section_or_persist_error(
    args: &ReviewArgs,
    project_root: &std::path::Path,
    review_description: &str,
) -> Result<Option<String>> {
    match args
        .prior_rounds_summary
        .as_deref()
        .map(crate::review_prior_rounds::load_prior_rounds_section)
        .transpose()
    {
        Ok(section) => Ok(section),
        Err(err) => Err(crate::session_guard::persist_pre_exec_error_result(
            crate::session_guard::PreExecErrorCtx {
                project_root,
                session_id: review_pre_exec_session_id(args),
                description: Some(review_description),
                parent: None,
                tool_name: explicit_review_tool(args).map(|tool| tool.as_str()),
                task_type: Some("review"),
                tier_name: args.tier.as_deref(),
                error: err,
            },
        )),
    }
}

pub(super) fn review_pre_exec_session_id(args: &ReviewArgs) -> Option<&str> {
    args.session_id.as_deref().or(args.session.as_deref())
}

pub(super) fn explicit_review_tool(args: &ReviewArgs) -> Option<csa_core::types::ToolName> {
    args.tool.or_else(|| {
        args.model_spec
            .as_deref()
            .and_then(|spec| spec.split('/').next())
            .and_then(|tool_name| crate::run_helpers::parse_tool_name(tool_name).ok())
    })
}
