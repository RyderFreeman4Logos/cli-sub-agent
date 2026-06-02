use csa_executor::Executor;
use csa_hooks::{GuardContext, HooksConfig, format_guard_output, run_prompt_guards};
use csa_session::MetaSessionState;
use tracing::info;

use crate::pipeline::prompt_cache::PromptAssembly;
use crate::pipeline::prompt_guard::emit_prompt_guard_to_caller;

pub(super) fn inject_prompt_guards_if_needed(
    task_type: Option<&str>,
    hooks_config: &HooksConfig,
    session: &MetaSessionState,
    executor: &Executor,
    session_arg_present: bool,
    prompt_assembly: &mut PromptAssembly,
    current_depth: u32,
) {
    // Suppress guards for debate (read-only, #467); review keeps them for --fix.
    if matches!(task_type, Some("debate")) || hooks_config.prompt_guard.is_empty() {
        return;
    }

    let guard_context = GuardContext {
        project_root: session.project_path.clone(),
        session_id: session.meta_session_id.clone(),
        tool: executor.tool_name().to_string(),
        is_resume: session_arg_present,
        cwd: std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
    };
    let guard_results = run_prompt_guards(&hooks_config.prompt_guard, &guard_context);
    if let Some(guard_block) = format_guard_output(&guard_results) {
        info!(
            guard_count = guard_results.len(),
            bytes = guard_block.len(),
            "Injecting prompt guard output into effective prompt"
        );
        emit_prompt_guard_to_caller(&guard_block, guard_results.len(), current_depth);
        prompt_assembly.append_dynamic_block(&guard_block);
    }
}
