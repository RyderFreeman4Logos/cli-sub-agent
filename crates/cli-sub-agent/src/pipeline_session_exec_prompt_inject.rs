//! Post-assembly prompt-block injection for `csa run` session execution.
//!
//! The base prompt guards are applied by the caller; this module appends the
//! remaining order-sensitive blocks — the review-aware writer guard (#1842) and
//! structured-output section markers — and returns the effective prompt.
//! Extracted from `execute_with_session_and_meta_*` so the #1842 guard can be
//! threaded in while keeping that module under the 8000-token monolith budget.

use std::path::Path;

use csa_config::ProjectConfig;
use tracing::info;

use crate::pipeline::prompt_cache::PromptAssembly;

const DAEMON_SESSION_ID_ENV: &str = "CSA_DAEMON_SESSION_ID";
const CLAUDE_DAEMON_CHILD_BACKGROUND_GUARD: &str = r#"<claude-code-daemon-child-background-guard issue="1712">
This CSA daemon-child is running under Claude Code. Do not start Bash tools with run_in_background: true in this inner session; Claude Code task-notification events are not reliably relayed through the daemon-child transport. Run shell work in the foreground and let CSA's outer session wait/attach mechanism handle backgrounding.
</claude-code-daemon-child-background-guard>"#;

/// Append the review-aware writer guard and structured-output markers to an
/// already-guarded `prompt_assembly`, then finalize and return the prompt.
///
/// Order is significant and mirrors the original inline sequence: the
/// review-aware writer guard (#1842) first, then structured-output markers.
/// The review guard is appended to the DYNAMIC prompt layer (never the cached
/// static block) so a per-project checklist cannot poison the
/// cross-project/mode prompt cache (#1842 constraint B); `sa_mode` is read from
/// the same caller-injection env signal the post-exec path uses.
pub(super) fn finalize_effective_prompt(
    mut prompt_assembly: PromptAssembly,
    tool_name: &str,
    task_type: Option<&str>,
    is_first_turn: bool,
    project_root: &Path,
    config: Option<&ProjectConfig>,
) -> String {
    if should_prepend_claude_daemon_child_background_guard(tool_name) {
        info!("Injecting claude-code daemon-child background-task guard (#1712)");
        prompt_assembly.prepend_dynamic(&format!("{CLAUDE_DAEMON_CHILD_BACKGROUND_GUARD}\n\n"));
    }

    let caller_sa_mode =
        std::env::var(crate::pipeline::prompt_guard::PROMPT_GUARD_CALLER_INJECTION_ENV)
            .ok()
            .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "true" | "1"))
            .unwrap_or(false);
    if let Some(review_guard) = super::session_exec_review_guard::build_review_writer_guard(
        caller_sa_mode,
        task_type,
        is_first_turn,
        project_root,
    ) {
        info!(
            bytes = review_guard.len(),
            "Injecting review-aware writer guard (#1842)"
        );
        prompt_assembly.append_dynamic_block(&review_guard);
    }

    // Inject structured output section markers when enabled in config.
    let structured_output_enabled = config.is_none_or(|cfg| cfg.session.structured_output);
    if let Some(instructions) =
        csa_executor::structured_output_instructions(structured_output_enabled)
    {
        info!("Injecting structured output instructions into prompt");
        prompt_assembly.add_static_or_append_dynamic(instructions);
    }

    prompt_assembly.finish()
}

fn should_prepend_claude_daemon_child_background_guard(tool_name: &str) -> bool {
    tool_name == "claude-code" && std::env::var_os(DAEMON_SESSION_ID_ENV).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finish_prompt(tool_name: &str) -> String {
        finalize_effective_prompt(
            PromptAssembly::new("user task".to_string(), false),
            tool_name,
            Some("run"),
            true,
            Path::new("."),
            None,
        )
    }

    #[test]
    fn claude_code_daemon_child_prompt_disables_inner_background_bash() {
        let _daemon_guard =
            crate::test_env_lock::ScopedTestEnvVar::set(DAEMON_SESSION_ID_ENV, "01KDAEMON");

        let prompt = finish_prompt("claude-code");

        assert!(prompt.starts_with("<claude-code-daemon-child-background-guard issue=\"1712\">"));
        assert!(prompt.contains("Do not start Bash tools with run_in_background: true"));
        assert!(prompt.contains("user task"));
    }

    #[test]
    fn foreground_claude_code_prompt_does_not_get_background_guard() {
        let _env_lock = crate::test_env_lock::TEST_ENV_LOCK.blocking_lock();
        let _daemon_guard = crate::test_env_lock::ScopedEnvVarRestore::unset(DAEMON_SESSION_ID_ENV);

        let prompt = finish_prompt("claude-code");

        assert!(!prompt.contains("claude-code-daemon-child-background-guard"));
    }

    #[test]
    fn daemon_child_background_guard_is_limited_to_claude_code() {
        let _daemon_guard =
            crate::test_env_lock::ScopedTestEnvVar::set(DAEMON_SESSION_ID_ENV, "01KDAEMON");

        let prompt = finish_prompt("codex");

        assert!(!prompt.contains("claude-code-daemon-child-background-guard"));
    }
}
