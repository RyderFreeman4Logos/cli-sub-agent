//! Prompt, environment, and subtree-pin assembly for a run attempt.

use std::collections::HashMap;

use csa_config::{ExecutionEnvOptions, GlobalConfig, ProjectConfig};
use csa_executor::structured_output_instructions_for_fork_call;
use tracing::info;

use crate::run_cmd_fork::ForkResolution;
use crate::startup_env::StartupSubtreeEnv;

pub(super) struct AttemptPromptRequest<'a> {
    pub(super) global_config: &'a GlobalConfig,
    pub(super) tool_name: &'a str,
    pub(super) no_failover: bool,
    pub(super) build_jobs: Option<u32>,
    pub(super) skill: Option<&'a str>,
    pub(super) run_resolved_pin_spec: Option<&'a str>,
    pub(super) current_attempt_model_spec: Option<&'a str>,
    pub(super) subtree_model_pin_force_ignore_tier_setting: bool,
    pub(super) fork_resolution: Option<&'a ForkResolution>,
    pub(super) prompt_text: &'a str,
    pub(super) failover_context_addendum: Option<&'a str>,
    pub(super) fork_call: bool,
    pub(super) config: Option<&'a ProjectConfig>,
    pub(super) startup_env: &'a StartupSubtreeEnv,
}

pub(super) struct AttemptPrompt {
    pub(super) extra_env: Option<HashMap<String, String>>,
    pub(super) subtree_pin: Option<csa_core::env::SubtreeModelPin>,
    pub(super) effective_prompt: String,
}

pub(super) fn build_attempt_prompt(request: AttemptPromptRequest<'_>) -> AttemptPrompt {
    let mut extra_env = request.global_config.build_execution_env(
        request.tool_name,
        ExecutionEnvOptions::from_no_failover(request.no_failover),
    );
    crate::build_jobs_env::apply_build_jobs_env(&mut extra_env, request.build_jobs);
    crate::executor_csa_guard::mark_skill_executor_env(&mut extra_env, request.skill.is_some());

    let subtree_model_pin_spec = resolve_attempt_subtree_model_pin_spec(
        request.run_resolved_pin_spec,
        request.current_attempt_model_spec,
    );
    let subtree_pin = crate::run_cmd_model_pin::resolve_subtree_model_pin(
        subtree_model_pin_spec,
        request.subtree_model_pin_force_ignore_tier_setting,
        request.no_failover,
    );

    let mut effective_prompt = if let Some(fork_res) = request.fork_resolution {
        if let Some(ref context_prefix) = fork_res.context_prefix {
            info!(
                context_len = context_prefix.len(),
                "Prepending soft fork context to prompt"
            );
            format!("{context_prefix}\n\n---\n\n{}", request.prompt_text)
        } else {
            request.prompt_text.to_string()
        }
    } else {
        request.prompt_text.to_string()
    };

    if let Some(addendum) = request.failover_context_addendum {
        effective_prompt = format!("{addendum}\n\n---\n\n{effective_prompt}");
    }
    if let Some(guard) = crate::run_cmd_model_pin::subtree_model_pin_prompt_guard(
        subtree_model_pin_spec,
        request.subtree_model_pin_force_ignore_tier_setting,
        request.no_failover,
    ) {
        effective_prompt = format!("{guard}\n\n{effective_prompt}");
    }

    if request.fork_call
        && let Some(instructions) = structured_output_instructions_for_fork_call(true)
    {
        effective_prompt.push_str(instructions);
    }
    if let Some(guard) = crate::pipeline::prompt_guard::anti_recursion_guard(
        request.config,
        request.startup_env.current_depth(),
    ) {
        effective_prompt = format!("{guard}\n\n{effective_prompt}");
    }

    AttemptPrompt {
        extra_env,
        subtree_pin,
        effective_prompt,
    }
}

pub(super) fn resolve_attempt_subtree_model_pin_spec<'a>(
    run_resolved_pin_spec: Option<&'a str>,
    current_attempt_model_spec: Option<&'a str>,
) -> Option<&'a str> {
    current_attempt_model_spec.or(run_resolved_pin_spec)
}
