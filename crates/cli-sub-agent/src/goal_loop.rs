use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::Result;
use csa_core::types::{OutputFormat, ToolArg};

pub(crate) struct GoalLoop {
    goal_criteria: String,
    max_loops: u32,
    max_tokens: u64,
    current_loop: u32,
    tokens_used: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GoalDecision {
    Continue,
    BudgetExhausted(&'static str),
}

impl GoalLoop {
    pub(crate) fn new(goal_criteria: String, max_loops: u32, max_tokens: u64) -> Self {
        Self {
            goal_criteria,
            max_loops,
            max_tokens,
            current_loop: 0,
            tokens_used: 0,
        }
    }

    pub(crate) fn should_continue(&self) -> GoalDecision {
        if self.current_loop >= self.max_loops {
            return GoalDecision::BudgetExhausted("max loops");
        }
        if self.tokens_used >= self.max_tokens {
            return GoalDecision::BudgetExhausted("max tokens");
        }
        GoalDecision::Continue
    }

    fn record_iteration(&mut self, tokens_used: u64) {
        self.current_loop = self.current_loop.saturating_add(1);
        self.tokens_used = self.tokens_used.saturating_add(tokens_used);
    }

    fn next_iteration(&self) -> u32 {
        self.current_loop.saturating_add(1)
    }

    fn goal_criteria(&self) -> &str {
        &self.goal_criteria
    }

    fn loops_used(&self) -> u32 {
        self.current_loop
    }

    fn tokens_used(&self) -> u64 {
        self.tokens_used
    }
}

pub(crate) struct GoalRunRequest {
    pub(crate) goal_criteria: Option<String>,
    pub(crate) tool: Option<ToolArg>,
    pub(crate) auto_route: Option<String>,
    pub(crate) hint_difficulty: Option<String>,
    pub(crate) skill: Option<String>,
    pub(crate) prompt: Option<String>,
    pub(crate) prompt_flag: Option<String>,
    pub(crate) prompt_file: Option<PathBuf>,
    pub(crate) inline_context_from_review_session: Option<String>,
    pub(crate) session: Option<String>,
    pub(crate) last: bool,
    pub(crate) fork_from: Option<String>,
    pub(crate) fork_last: bool,
    pub(crate) description: Option<String>,
    pub(crate) fork_call: bool,
    pub(crate) return_to: Option<String>,
    pub(crate) parent: Option<String>,
    pub(crate) ephemeral: bool,
    pub(crate) allow_base_branch_working: bool,
    pub(crate) cd: Option<String>,
    pub(crate) model_spec: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) thinking: Option<String>,
    pub(crate) force: bool,
    pub(crate) force_override_user_config: bool,
    pub(crate) no_failover: bool,
    pub(crate) wait: bool,
    pub(crate) idle_timeout: Option<u64>,
    pub(crate) initial_response_timeout: Option<u64>,
    pub(crate) timeout: Option<u64>,
    pub(crate) no_idle_timeout: bool,
    pub(crate) no_memory: bool,
    pub(crate) memory_query: Option<String>,
    pub(crate) current_depth: u32,
    pub(crate) output_format: OutputFormat,
    pub(crate) stream_mode: csa_process::StreamMode,
    pub(crate) tier: Option<String>,
    pub(crate) force_ignore_tier_setting: bool,
    pub(crate) no_fs_sandbox: bool,
    pub(crate) extra_writable: Vec<PathBuf>,
    pub(crate) extra_readable: Vec<PathBuf>,
}

struct FailureContext {
    iteration: u32,
    exit_code: i32,
    session_id: Option<String>,
    summary: String,
}

struct IterationResult {
    exit_code: i32,
    session_id: Option<String>,
    summary: String,
    tokens_used: u64,
}

pub(crate) async fn handle_run_or_goal(request: GoalRunRequest) -> Result<i32> {
    if request.goal_criteria.is_some() {
        return handle_goal_run(request).await;
    }

    crate::run_cmd::handle_run(
        request.tool,
        request.auto_route,
        request.hint_difficulty,
        request.skill,
        request.prompt,
        request.prompt_flag,
        request.prompt_file,
        request.inline_context_from_review_session,
        request.session,
        request.last,
        request.fork_from,
        request.fork_last,
        request.description,
        request.fork_call,
        request.return_to,
        request.parent,
        request.ephemeral,
        request.allow_base_branch_working,
        request.cd,
        request.model_spec,
        request.model,
        request.thinking,
        request.force,
        request.force_override_user_config,
        request.no_failover,
        request.wait,
        request.idle_timeout,
        request.initial_response_timeout,
        request.timeout,
        request.no_idle_timeout,
        request.no_memory,
        request.memory_query,
        request.current_depth,
        request.output_format,
        request.stream_mode,
        request.tier,
        request.force_ignore_tier_setting,
        request.no_fs_sandbox,
        request.extra_writable,
        request.extra_readable,
    )
    .await
}

async fn handle_goal_run(request: GoalRunRequest) -> Result<i32> {
    let project_root = crate::pipeline::determine_project_root(request.cd.as_deref())?;
    let global_config = csa_config::GlobalConfig::load()?;
    let goal_criteria = request
        .goal_criteria
        .clone()
        .expect("goal criteria should be present in goal mode");
    let mut goal_loop = GoalLoop::new(
        goal_criteria,
        global_config.experimental.max_goal_loops,
        global_config.experimental.max_goal_tokens,
    );
    let user_prompt = resolve_goal_user_prompt(&request)?;
    let mut previous_failure = None;

    loop {
        if let GoalDecision::BudgetExhausted(reason) = goal_loop.should_continue() {
            eprintln!(
                "goal loop stopped before dispatch: budget exhausted ({reason}); loops={} tokens={}",
                goal_loop.loops_used(),
                goal_loop.tokens_used()
            );
            return Ok(1);
        }

        let iteration = goal_loop.next_iteration();
        let prompt = build_goal_prompt(goal_loop.goal_criteria(), &user_prompt, &previous_failure);
        let result = run_goal_iteration(&request, &project_root, prompt, iteration == 1).await?;
        goal_loop.record_iteration(result.tokens_used);

        if result.exit_code == 0 {
            eprintln!(
                "goal loop complete: status=success loops={} tokens={} final_session={}",
                goal_loop.loops_used(),
                goal_loop.tokens_used(),
                result.session_id.as_deref().unwrap_or("(none)")
            );
            return Ok(0);
        }

        previous_failure = Some(FailureContext {
            iteration,
            exit_code: result.exit_code,
            session_id: result.session_id,
            summary: result.summary,
        });

        if let GoalDecision::BudgetExhausted(reason) = goal_loop.should_continue() {
            eprintln!(
                "goal loop stopped: budget exhausted ({reason}); loops={} tokens={} final_status=failure",
                goal_loop.loops_used(),
                goal_loop.tokens_used()
            );
            return Ok(result.exit_code);
        }
    }
}

fn resolve_goal_user_prompt(request: &GoalRunRequest) -> Result<String> {
    let prompt = crate::run_helpers::resolve_positional_stdin_sentinel(request.prompt.clone())?
        .or_else(|| request.prompt_flag.clone());

    if request.prompt_file.is_some() {
        return crate::run_helpers::resolve_prompt_with_file(
            prompt,
            request.prompt_file.as_deref(),
        );
    }

    if prompt.is_some() {
        return crate::run_helpers::read_prompt(prompt);
    }

    if request.skill.is_some() {
        return Ok(String::new());
    }

    crate::run_helpers::read_prompt(None)
}

fn build_goal_prompt(
    goal_criteria: &str,
    user_prompt: &str,
    previous_failure: &Option<FailureContext>,
) -> String {
    let mut prompt = format!(
        "<goal-mode>\nGoal criteria:\n{goal_criteria}\n\nSuccess detection for this CSA version is deterministic: exit code 0 means success; non-zero means retry while budget remains.\n</goal-mode>\n\nUser task:\n{user_prompt}"
    );

    if let Some(failure) = previous_failure {
        prompt.push_str(&format!(
            "\n\n<goal-loop-feedback iteration=\"{}\" exit_code=\"{}\" session=\"{}\">\nPrevious attempt did not meet the goal. Use this failure context to correct the next attempt.\n\nSummary:\n{}\n</goal-loop-feedback>",
            failure.iteration,
            failure.exit_code,
            failure.session_id.as_deref().unwrap_or("(none)"),
            failure.summary
        ));
    }

    prompt
}

async fn run_goal_iteration(
    request: &GoalRunRequest,
    project_root: &Path,
    prompt: String,
    first_iteration: bool,
) -> Result<IterationResult> {
    let before_sessions = snapshot_session_ids(project_root)?;
    let exit_code = crate::run_cmd::handle_run(
        request.tool.clone(),
        request.auto_route.clone(),
        request.hint_difficulty.clone(),
        request.skill.clone(),
        Some(prompt),
        None,
        None,
        request.inline_context_from_review_session.clone(),
        first_iteration.then(|| request.session.clone()).flatten(),
        first_iteration && request.last,
        first_iteration.then(|| request.fork_from.clone()).flatten(),
        first_iteration && request.fork_last,
        request.description.clone(),
        request.fork_call,
        request.return_to.clone(),
        request.parent.clone(),
        request.ephemeral,
        request.allow_base_branch_working,
        request.cd.clone(),
        request.model_spec.clone(),
        request.model.clone(),
        request.thinking.clone(),
        request.force,
        request.force_override_user_config,
        request.no_failover,
        request.wait,
        request.idle_timeout,
        request.initial_response_timeout,
        request.timeout,
        request.no_idle_timeout,
        request.no_memory,
        request.memory_query.clone(),
        request.current_depth,
        request.output_format,
        request.stream_mode,
        request.tier.clone(),
        request.force_ignore_tier_setting,
        request.no_fs_sandbox,
        request.extra_writable.clone(),
        request.extra_readable.clone(),
    )
    .await?;

    let session_id = newest_created_session_id(project_root, &before_sessions)?;
    let summary = session_id
        .as_deref()
        .and_then(|sid| load_result_summary(project_root, sid).ok().flatten())
        .unwrap_or_else(|| format!("run exited with code {exit_code}; session result unavailable"));
    let tokens_used = session_id
        .as_deref()
        .and_then(|sid| load_session_token_usage(project_root, sid).ok())
        .unwrap_or(0);

    Ok(IterationResult {
        exit_code,
        session_id,
        summary,
        tokens_used,
    })
}

fn snapshot_session_ids(project_root: &Path) -> Result<HashSet<String>> {
    Ok(csa_session::list_sessions(project_root, None)?
        .into_iter()
        .map(|session| session.meta_session_id)
        .collect())
}

fn newest_created_session_id(
    project_root: &Path,
    before_sessions: &HashSet<String>,
) -> Result<Option<String>> {
    Ok(csa_session::list_sessions(project_root, None)?
        .into_iter()
        .filter(|session| !before_sessions.contains(&session.meta_session_id))
        .max_by_key(|session| session.created_at)
        .map(|session| session.meta_session_id))
}

fn load_result_summary(project_root: &Path, session_id: &str) -> Result<Option<String>> {
    Ok(csa_session::load_result(project_root, session_id)?
        .map(|result| result.summary)
        .filter(|summary| !summary.trim().is_empty()))
}

fn load_session_token_usage(project_root: &Path, session_id: &str) -> Result<u64> {
    let session = csa_session::load_session(project_root, session_id)?;
    if let Some(usage) = session.total_token_usage.as_ref() {
        return Ok(token_usage_total(usage));
    }

    Ok(session
        .tools
        .values()
        .filter_map(|tool| tool.token_usage.as_ref())
        .map(token_usage_total)
        .sum())
}

fn token_usage_total(usage: &csa_session::TokenUsage) -> u64 {
    usage
        .total_tokens
        .or_else(|| match (usage.input_tokens, usage.output_tokens) {
            (Some(input), Some(output)) => Some(input.saturating_add(output)),
            (Some(input), None) => Some(input),
            (None, Some(output)) => Some(output),
            (None, None) => None,
        })
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn goal_loop_continues_within_budget() {
        let goal_loop = GoalLoop::new("pass tests".to_string(), 3, 500_000);

        assert_eq!(goal_loop.should_continue(), GoalDecision::Continue);
    }

    #[test]
    fn goal_loop_stops_when_loops_exhausted() {
        let mut goal_loop = GoalLoop::new("pass tests".to_string(), 1, 500_000);
        goal_loop.record_iteration(10);

        assert_eq!(
            goal_loop.should_continue(),
            GoalDecision::BudgetExhausted("max loops")
        );
    }

    #[test]
    fn goal_loop_stops_when_tokens_exhausted() {
        let mut goal_loop = GoalLoop::new("pass tests".to_string(), 3, 10);
        goal_loop.record_iteration(10);

        assert_eq!(
            goal_loop.should_continue(),
            GoalDecision::BudgetExhausted("max tokens")
        );
    }
}
