use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::Result;

use csa_config::ProjectConfig;
use csa_core::types::ToolName;
use weave::compiler::ExecutionPlan;

use super::PlanRunJournal;
use super::plan_cmd_steps::{
    PlanRunContext, StepExecutionContext, StepResult, execute_plan_with_journal,
    execute_step_with_workflow,
};

/// Execute all steps in the plan sequentially.
///
/// After each successful step, injects `STEP_<id>_OUTPUT` into the variables
/// map so subsequent steps can reference prior outputs via `${STEP_1_OUTPUT}`.
pub(crate) async fn execute_plan(
    plan: &ExecutionPlan,
    variables: &HashMap<String, String>,
    project_root: &Path,
    config: Option<&ProjectConfig>,
    tool_override: Option<&ToolName>,
) -> Result<Vec<StepResult>> {
    let workflow_path = project_root.join("workflow.toml");
    let mut journal = PlanRunJournal::new(&plan.name, &workflow_path, variables.clone());
    let completed = HashSet::new();
    let startup_env = crate::startup_env::StartupSubtreeEnv::default();
    let mut run_ctx = PlanRunContext {
        project_root,
        workflow_path: &workflow_path,
        config,
        tool_override,
        model_spec_override: None,
        journal: &mut journal,
        journal_path: None,
        resume_completed_steps: &completed,
        chunked: false,
        no_fs_sandbox: false,
        startup_env: &startup_env,
    };
    execute_plan_with_journal(plan, variables, &mut run_ctx).await
}

/// Execute a single step with on_fail handling.
pub(crate) async fn execute_step(
    step: &weave::compiler::PlanStep,
    variables: &HashMap<String, String>,
    project_root: &Path,
    config: Option<&ProjectConfig>,
    tool_override: Option<&ToolName>,
    model_spec_override: Option<&String>,
) -> StepResult {
    let workflow_path_buf = project_root.join("workflow.toml");
    let startup_env = crate::startup_env::StartupSubtreeEnv::default();
    execute_step_with_workflow(
        step,
        variables,
        &StepExecutionContext {
            project_root,
            workflow_path: &workflow_path_buf,
            config,
            tool_override,
            model_spec_override,
            no_fs_sandbox: false,
            startup_env: &startup_env,
        },
    )
    .await
}
