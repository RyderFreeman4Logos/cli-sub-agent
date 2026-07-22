use std::path::Path;

use csa_config::ProjectConfig;
use csa_core::types::ToolArg;

/// Reuse a tier only when an explicit resume names the same tool recorded in
/// the existing session. Other tool selectors remain fresh routing requests.
pub(super) fn infer_resume_tier_for_matching_tool(
    project_root: &Path,
    config: Option<&ProjectConfig>,
    session_ref: Option<&str>,
    tool_arg: Option<&ToolArg>,
) -> Option<String> {
    let config = config?;
    let session_ref = session_ref?;
    let tool_name = match tool_arg? {
        ToolArg::Specific(tool) => tool.as_str(),
        ToolArg::Auto | ToolArg::AnyAvailable | ToolArg::Alias(_) => return None,
    };
    let resolution =
        csa_session::resolve_resume_session(project_root, session_ref, tool_name).ok()?;
    let session = csa_session::load_session(project_root, &resolution.meta_session_id).ok()?;
    let tier_name = session.task_context.tier_name?;

    session
        .tools
        .contains_key(tool_name)
        .then(|| config.resolve_tier_selector(&tier_name))
        .flatten()
}
