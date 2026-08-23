use std::fmt;

use csa_core::types::ToolName;

pub(crate) const READONLY_REPO_TOOLS_UNAVAILABLE_REASON: &str =
    "openai_compat_readonly_repo_tools_unavailable";

const REQUIRED_READONLY_REPO_TOOLS: &[&str] = &["Bash", "Read", "Grep", "Glob"];

#[derive(Debug)]
pub(crate) struct ReadonlyRepoToolsUnavailable;

impl fmt::Display for ReadonlyRepoToolsUnavailable {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "CSA: {READONLY_REPO_TOOLS_UNAVAILABLE_REASON}: OpenAI-compat review cannot run because the required read-only repository tools are unavailable: {}. No provider request was made.",
            REQUIRED_READONLY_REPO_TOOLS.join(", "),
        )
    }
}

impl std::error::Error for ReadonlyRepoToolsUnavailable {}

pub(crate) fn ensure_review_tool_surface(
    tool: &ToolName,
    task_type: Option<&str>,
) -> Result<(), ReadonlyRepoToolsUnavailable> {
    if *tool == ToolName::OpenaiCompat && task_type == Some("reviewer_sub_session") {
        return Err(ReadonlyRepoToolsUnavailable);
    }
    Ok(())
}
