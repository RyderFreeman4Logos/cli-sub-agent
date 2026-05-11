use csa_config::ExecutionEnvOptions;
use csa_core::types::ToolName;

pub(crate) fn debate_execution_env_options(no_failover: bool) -> ExecutionEnvOptions {
    let options = ExecutionEnvOptions::with_no_flash_fallback();
    if no_failover {
        options.with_no_failover()
    } else {
        options
    }
}

pub(crate) fn warn_if_fast_mode_has_no_codex_debate_candidate(
    fast_but_more_cost: bool,
    candidates: &[(ToolName, Option<String>)],
) {
    if fast_but_more_cost && !candidates.iter().any(|(tool, _)| *tool == ToolName::Codex) {
        eprintln!(
            "warning: --fast-but-more-cost only affects codex; no codex debate attempt is in the resolved candidate set."
        );
    }
}
