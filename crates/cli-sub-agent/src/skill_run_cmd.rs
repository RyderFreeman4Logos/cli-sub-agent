//! Async handler for `csa skill run` — delegates to the standard CSA run pipeline
//! or emits the skill prompt for direct execution (`--inject`).

use anyhow::Result;
use csa_core::types::OutputFormat;

use crate::goal_loop;
use crate::skill_resolver;

/// Emit the resolved skill prompt to stdout for the calling agent to execute
/// directly, bypassing CSA session creation.
pub(crate) async fn handle_skill_inject(name: String, prompt: Vec<String>) -> Result<i32> {
    let project_root = std::env::current_dir()?;
    let resolved = skill_resolver::resolve_skill(&name, &project_root)?;

    let prompt_str = if prompt.is_empty() {
        String::new()
    } else {
        prompt.join(" ")
    };

    println!("<skill-injection name=\"{name}\">");
    println!("You MUST execute the following skill instructions directly. Do NOT delegate to csa.");
    if !prompt_str.is_empty() {
        println!();
        println!("## Input");
        println!();
        println!("{prompt_str}");
    }
    println!();
    println!("{}", resolved.skill_md);
    println!("</skill-injection>");

    Ok(0)
}

/// Run a named skill via the standard CSA run pipeline.
///
/// Equivalent to `csa run --skill <name> [prompt]`.
pub(crate) async fn handle_skill_run(
    name: String,
    prompt: Vec<String>,
    current_depth: u32,
    output_format: OutputFormat,
    startup_env: crate::startup_env::StartupSubtreeEnv,
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
        build_jobs: None,
        resource_overrides: crate::run_resource_overrides::RunResourceOverrides::inherited(),
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
        allow_user_daemon_ipc: false,
        // Defer to the CSA_PATTERN_INTERNAL marker / config: a skill run spawned
        // by a pattern-internal `csa plan run` bash step inherits the marker and
        // disables the scan by default (#1847).
        error_marker_scan_override: None,
        no_hook_bypass_scan: false,
        no_preflight: false,
        no_post_exec_gate: false,
        require_commit: false,
        allow_git_push: false,
        extra_writable: vec![],
        extra_readable: vec![],
        startup_env,
    })
    .await
}
