use std::path::Path;

use csa_core::types::ToolName;

pub(super) fn emit_reusable_session_hint(
    project_root: &Path,
    resolved_tool: ToolName,
    effective_session_arg: Option<&str>,
    is_fork: bool,
) {
    if effective_session_arg.is_some() || is_fork {
        return;
    }

    let tool_names = vec![resolved_tool.as_str().to_string()];
    if let Ok(candidates) =
        csa_scheduler::session_reuse::find_reusable_sessions(project_root, "run", &tool_names)
        && let Some(best) = candidates.first()
    {
        eprintln!(
            "hint: reusable session available for {}: --fork-from {}",
            best.tool_name,
            best.session_id.get(..8).unwrap_or(&best.session_id),
        );
    }
}
