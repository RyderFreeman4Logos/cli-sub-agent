use std::path::Path;

use csa_config::ProjectConfig;
use csa_core::types::{ToolName, ToolSelectionStrategy};

use crate::pipeline;

pub(crate) fn allow_cross_tool_failover(
    strategy: ToolSelectionStrategy,
    resolved_tier_name: Option<&str>,
    force_ignore_tier_setting: bool,
    no_failover: bool,
) -> bool {
    if no_failover {
        return false;
    }

    !matches!(strategy, ToolSelectionStrategy::Explicit(_))
        || (!force_ignore_tier_setting && resolved_tier_name.is_some())
}

pub(crate) fn resolve_attempt_initial_response_timeout_seconds(
    config: Option<&ProjectConfig>,
    cli_initial_response_timeout: Option<u64>,
    cli_idle_timeout: Option<u64>,
    no_idle_timeout: bool,
    tool_name: &str,
) -> Option<u64> {
    if no_idle_timeout {
        None
    } else {
        pipeline::resolve_initial_response_timeout_for_tool(
            config,
            cli_initial_response_timeout,
            cli_idle_timeout,
            tool_name,
        )
    }
}

/// Build a prompt addendum for rate-limit failover that tells the new tool
/// how to retrieve the original session's conversation context via xurl.
///
/// Returns `None` when there is no session to reference.
pub(crate) fn build_failover_context_addendum(
    failed_tool: &str,
    session_id: Option<&str>,
) -> Option<String> {
    let sid = session_id?;
    let provider = match failed_tool {
        "gemini-cli" => "gemini",
        "claude-code" => "claude",
        "codex" => "codex",
        "opencode" => "opencode",
        other => other,
    };
    Some(format!(
        "[Rate-limit failover context]\n\
         This task was originally being handled by {failed_tool} (session {sid}) \
         which hit a rate limit / quota exhaustion.\n\
         If you need the full conversation context from the previous session, run:\n\
         ```\n\
         csa xurl threads --keyword {provider}\n\
         ```\n\
         Then use the session/thread ID to read the relevant conversation."
    ))
}

pub(crate) fn persist_fork_timeout_result_if_missing(
    project_root: &Path,
    is_fork: bool,
    tool: ToolName,
    session_id: Option<&str>,
    execution_start_time: chrono::DateTime<chrono::Utc>,
    timeout_seconds: u64,
) {
    if !is_fork {
        return;
    }
    let Some(session_id) = session_id else {
        return;
    };

    let err = anyhow::anyhow!(
        "wall-clock timeout interrupted forked execution before normal finalization after {timeout_seconds}s"
    );
    crate::pipeline_post_exec::ensure_terminal_result_for_session_on_post_exec_error(
        project_root,
        session_id,
        tool.as_str(),
        execution_start_time,
        &err,
    );
}
