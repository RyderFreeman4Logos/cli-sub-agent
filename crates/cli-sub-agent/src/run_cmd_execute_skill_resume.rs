use std::path::Path;

use csa_core::types::ToolName;

pub(super) fn maybe_auto_resume_interrupted_skill_session(
    project_root: &Path,
    skill: Option<&str>,
    resolved_tool: &ToolName,
    session_arg: Option<String>,
    is_fork: bool,
    fork_call: bool,
    ephemeral: bool,
) -> Option<String> {
    if session_arg.is_some() || is_fork || fork_call || ephemeral {
        return session_arg;
    }

    let Some(skill_name) = skill else {
        return session_arg;
    };
    let Some(interrupted_session_id) = super::super::resume::find_recent_interrupted_skill_session(
        project_root,
        skill_name,
        resolved_tool,
    ) else {
        return session_arg;
    };

    eprintln!(
        "Auto-resuming interrupted skill session {interrupted_session_id} for '{skill_name}'."
    );
    Some(interrupted_session_id)
}
