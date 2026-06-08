//! Tests for `csa run` git-push authorization in attempt prompts.

use csa_config::GlobalConfig;

fn build_attempt(allow_git_push: bool) -> super::prompt::AttemptPrompt {
    let global_config = GlobalConfig::default();
    super::prompt::build_attempt_prompt(super::prompt::AttemptPromptRequest {
        global_config: &global_config,
        tool_name: "codex",
        no_failover: false,
        build_jobs: None,
        skill: None,
        run_resolved_pin_spec: None,
        current_attempt_model_spec: None,
        subtree_model_pin_force_ignore_tier_setting: false,
        fork_resolution: None,
        prompt_text: "Fix the review finding, run checks, and commit atomically.",
        failover_context_addendum: None,
        fork_call: false,
        allow_git_push,
        config: None,
        startup_env: &crate::startup_env::EMPTY_STARTUP_SUBTREE_ENV,
    })
}

fn has_push_env(attempt: &super::prompt::AttemptPrompt) -> bool {
    attempt.extra_env.as_ref().is_some_and(|env| {
        env.get(crate::pipeline_env::CSA_GIT_PUSH_ALLOWED_ENV)
            .is_some_and(|value| value == "true")
    })
}

#[test]
fn run_attempt_prompt_blocks_implicit_git_push_by_default() {
    let attempt = build_attempt(false);

    assert!(attempt.effective_prompt.contains("<git-push-guard>"));
    assert!(attempt.effective_prompt.contains("--allow-git-push"));
    assert!(!has_push_env(&attempt));
}

#[test]
fn run_attempt_prompt_allows_git_push_with_explicit_authorization() {
    let attempt = build_attempt(true);

    assert!(!attempt.effective_prompt.contains("<git-push-guard>"));
    assert!(has_push_env(&attempt));
}
