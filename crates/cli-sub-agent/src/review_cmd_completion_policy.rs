use anyhow::Result;
use csa_config::{
    ConvergenceCompletionPolicy, EffectiveConvergenceCompletionPolicy, GlobalConfig,
    ProjectConvergenceCompletionPolicy,
};

use crate::cli::ReviewArgs;
use crate::review_cmd::review_convergence;

/// Resolve the policy once so execution admission and authorization evidence use the same value.
pub(super) fn resolve(
    args: &ReviewArgs,
    global: &GlobalConfig,
    project: Option<&ProjectConvergenceCompletionPolicy>,
) -> Result<EffectiveConvergenceCompletionPolicy> {
    let policy = ConvergenceCompletionPolicy::effective(&global.convergence_completion, project);
    if args.execute_completion {
        review_convergence::ensure_completion_execution_is_allowed(
            &global.convergence_completion,
            project,
        )?;
    }
    Ok(policy)
}
