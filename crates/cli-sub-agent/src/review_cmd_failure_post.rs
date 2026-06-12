use std::path::Path;

use csa_core::types::{ReviewDecision, ToolName};
use csa_session::state::ReviewSessionMeta;

use crate::review_consensus::CLEAN;

pub(super) struct NonFixFailureContext<'a> {
    pub(super) project_root: &'a Path,
    pub(super) project_root_for_hooks: &'a str,
    pub(super) review_meta: &'a ReviewSessionMeta,
    pub(super) result: &'a super::execute::ReviewExecutionOutcome,
    pub(super) initial_tool: ToolName,
    pub(super) resolved_model_spec: Option<&'a str>,
    pub(super) review_model: Option<&'a str>,
    pub(super) review_thinking: Option<&'a str>,
    pub(super) sanitized: &'a str,
    pub(super) decision: ReviewDecision,
    pub(super) verdict: &'a str,
    pub(super) scope: &'a str,
    pub(super) effective_exit_code: i32,
    pub(super) empty_output: bool,
    pub(super) auth_prompt_failure: bool,
    pub(super) is_cumulative_review: bool,
    pub(super) review_session_ids: &'a [String],
}

pub(super) fn handle_non_fix_failure(ctx: NonFixFailureContext<'_>) -> i32 {
    let route = super::post_review::build_fix_finding_route(
        ctx.result,
        ctx.initial_tool,
        ctx.resolved_model_spec,
        ctx.review_model,
        ctx.review_thinking,
    );
    super::post_review::suggest_review_failure_fix(
        ctx.project_root,
        ctx.review_meta,
        ctx.sanitized,
        Some(&route),
    );
    if should_accumulate_findings(&ctx) {
        crate::review_findings::accumulate_findings(ctx.project_root, ctx.sanitized);
    }
    let hook_output = crate::pipeline::capture_observational_hook_output(
        csa_hooks::HookEvent::PostReview,
        &[
            ("session_id", ctx.result.execution.meta_session_id.as_str()),
            ("decision", ctx.decision.as_str()),
            ("verdict", ctx.verdict),
            ("scope", ctx.scope),
            ("project_root", ctx.project_root_for_hooks),
        ],
        ctx.project_root,
    );
    let output =
        super::post_review::build_post_review_output(&hook_output, ctx.decision, ctx.scope);
    super::post_review::emit_post_review_output(&output);
    super::bug_class_pipeline::maybe_extract_recurring_bug_class_skills(
        ctx.project_root,
        ctx.review_session_ids,
    );
    ctx.effective_exit_code
}

fn should_accumulate_findings(ctx: &NonFixFailureContext<'_>) -> bool {
    ctx.verdict != CLEAN
        && !ctx.empty_output
        && !ctx.auth_prompt_failure
        && !ctx.is_cumulative_review
}
