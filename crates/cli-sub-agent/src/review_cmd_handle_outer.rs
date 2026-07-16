use super::*;

pub(crate) async fn handle_review(
    args: ReviewArgs,
    current_depth: u32,
    startup_env: &StartupSubtreeEnv,
) -> Result<i32> {
    let convergence = args.converge;
    let execute_completion = args.execute_completion;
    match handle_review_inner(args, current_depth, startup_env).await {
        Ok(exit_code) => Ok(exit_code),
        Err(error) if convergence && execute_completion => {
            review_convergence::emit_completion_setup_block("execution_setup_failure", &error)
        }
        Err(error) if convergence => review_convergence::emit_setup_block("setup_failure", &error),
        Err(error) => Err(error),
    }
}
