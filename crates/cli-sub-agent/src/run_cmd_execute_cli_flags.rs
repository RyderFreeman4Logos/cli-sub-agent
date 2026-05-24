use anyhow::Result;
use tracing::warn;

use csa_core::types::ToolName;

use crate::cli::ReturnTarget;

pub(super) fn warn_deprecated_session_flags(last: bool, has_session_arg: bool) {
    if last {
        warn!("--last is deprecated: use --fork-last instead (fork-first architecture)");
        eprintln!(
            "warning: --last is deprecated and will be removed in a future release. Use --fork-last instead."
        );
    }
    if has_session_arg {
        warn!("--session is deprecated: use --fork-from instead (fork-first architecture)");
        eprintln!(
            "warning: --session is deprecated and will be removed in a future release. Use --fork-from instead."
        );
    }
}

pub(super) fn resolve_return_target(
    fork_call: bool,
    return_to: Option<&str>,
) -> Result<Option<ReturnTarget>> {
    if !fork_call {
        return Ok(None);
    }
    Ok(Some(match return_to {
        Some(value) => crate::cli::parse_return_to(value)?,
        None => ReturnTarget::Auto,
    }))
}

pub(super) fn warn_if_fast_mode_has_no_codex_run_candidate(
    fast_but_more_cost: bool,
    initial_tool: ToolName,
    runtime_fallback_candidates: &[ToolName],
) {
    if fast_but_more_cost
        && initial_tool != ToolName::Codex
        && !runtime_fallback_candidates.contains(&ToolName::Codex)
    {
        eprintln!(
            "warning: --fast-but-more-cost only affects codex; no codex run attempt is in the resolved candidate set."
        );
    }
}
