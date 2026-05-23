use std::path::Path;

use csa_config::ProjectConfig;
use csa_core::types::{ToolName, ToolSelectionStrategy};

use crate::pipeline;

pub(crate) fn allow_cross_tool_failover(
    strategy: ToolSelectionStrategy,
    _resolved_tier_name: Option<&str>,
    _force_ignore_tier_setting: bool,
    no_failover: bool,
) -> bool {
    if no_failover {
        return false;
    }

    // Explicit `--tool` (from CLI or skill agent_config) is the user's hard
    // selection: never silently fall over to a different tool, even when a
    // tier is also specified (#1440). Tier still drives model selection for
    // the chosen tool via `resolve_requested_tool_from_tier`.
    !matches!(strategy, ToolSelectionStrategy::Explicit(_))
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
        "antigravity-cli" => "antigravity",
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

pub(crate) fn merge_retry_changed_paths(
    accumulated_changed_paths: &mut Vec<String>,
    all_attempt_change_snapshots_available: &mut bool,
    attempt_changed_paths: Option<Vec<String>>,
) {
    match attempt_changed_paths {
        Some(paths) => accumulated_changed_paths.extend(paths),
        None => *all_attempt_change_snapshots_available = false,
    }
}

pub(crate) fn merge_run_loop_changed_paths(
    mut accumulated_changed_paths: Vec<String>,
    mut all_attempt_change_snapshots_available: bool,
    final_changed_paths: Option<Vec<String>>,
) -> Option<Vec<String>> {
    match final_changed_paths {
        Some(paths) => accumulated_changed_paths.extend(paths),
        None => all_attempt_change_snapshots_available = false,
    }

    if !all_attempt_change_snapshots_available {
        return None;
    }

    accumulated_changed_paths.sort();
    accumulated_changed_paths.dedup();
    Some(accumulated_changed_paths)
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

#[cfg(test)]
mod tests {
    use super::merge_run_loop_changed_paths;

    #[test]
    fn merge_run_loop_changed_paths_preserves_known_empty_delta() {
        let merged = merge_run_loop_changed_paths(Vec::new(), true, Some(Vec::new()));

        assert_eq!(merged, Some(Vec::new()));
    }

    #[test]
    fn merge_run_loop_changed_paths_returns_unknown_when_any_snapshot_is_missing() {
        let merged =
            merge_run_loop_changed_paths(vec!["src/lib.rs".to_string()], false, Some(vec![]));

        assert_eq!(merged, None);
    }

    #[test]
    fn merge_run_loop_changed_paths_deduplicates_known_paths() {
        let merged = merge_run_loop_changed_paths(
            vec!["src/lib.rs".to_string()],
            true,
            Some(vec!["src/lib.rs".to_string(), "src/main.rs".to_string()]),
        );

        assert_eq!(
            merged,
            Some(vec!["src/lib.rs".to_string(), "src/main.rs".to_string()])
        );
    }
}
