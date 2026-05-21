//! Async handler for `csa skill run` — delegates to the standard CSA run pipeline.

use anyhow::Result;
use csa_core::types::OutputFormat;

use crate::goal_loop;

/// Run a named skill via the standard CSA run pipeline.
///
/// Equivalent to `csa run --skill <name> [prompt]`.
pub(crate) async fn handle_skill_run(
    name: String,
    prompt: Vec<String>,
    current_depth: u32,
    output_format: OutputFormat,
) -> Result<i32> {
    let prompt_str = if prompt.is_empty() {
        None
    } else {
        Some(prompt.join(" "))
    };

    goal_loop::handle_run_or_goal(goal_loop::GoalRunRequest {
        goal_criteria: None,
        tool: None,
        auto_route: None,
        hint_difficulty: None,
        skill: Some(name),
        prompt: prompt_str,
        prompt_flag: None,
        prompt_file: None,
        inline_context_from_review_session: None,
        session: None,
        last: false,
        fork_from: None,
        fork_last: false,
        fork_from_caller: false,
        description: None,
        fork_call: false,
        return_to: None,
        parent: None,
        ephemeral: false,
        allow_base_branch_working: false,
        cd: None,
        model_spec: None,
        model: None,
        thinking: None,
        force: false,
        force_override_user_config: false,
        allow_fallback: false,
        no_failover: false,
        fast_but_more_cost: false,
        wait: false,
        idle_timeout: None,
        initial_response_timeout: None,
        timeout: None,
        no_idle_timeout: false,
        no_memory: false,
        memory_query: None,
        current_depth,
        output_format,
        stream_mode: csa_process::StreamMode::BufferOnly,
        tier: None,
        force_ignore_tier_setting: false,
        no_fs_sandbox: false,
        extra_writable: vec![],
        extra_readable: vec![],
    })
    .await
}
