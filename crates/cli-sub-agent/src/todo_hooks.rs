use csa_hooks::{HookEvent, global_hooks_path, load_hooks_config, run_hooks_for_event};
use std::collections::HashMap;
use std::path::Path;
use tracing::warn;

pub(crate) fn emit_todo_save_hook(
    project_root: &Path,
    todo_root: &Path,
    plan_id: &str,
    message: &str,
) {
    let hooks_config = load_hooks_config(
        csa_session::get_session_root(project_root)
            .ok()
            .map(|r| r.join("hooks.toml"))
            .as_deref(),
        global_hooks_path().as_deref(),
        None,
    );
    let version = csa_todo::git::list_versions(todo_root, plan_id)
        .map(|versions| versions.len())
        .unwrap_or(1);
    let mut hook_vars = HashMap::new();
    hook_vars.insert("plan_id".to_string(), plan_id.to_string());
    hook_vars.insert(
        "plan_dir".to_string(),
        todo_root.join(plan_id).display().to_string(),
    );
    hook_vars.insert("todo_root".to_string(), todo_root.display().to_string());
    hook_vars.insert(
        "project_root".to_string(),
        project_root.display().to_string(),
    );
    hook_vars.insert("version".to_string(), version.to_string());
    hook_vars.insert("message".to_string(), message.to_string());
    if let Err(e) = run_hooks_for_event(HookEvent::TodoSave, &hooks_config, &hook_vars) {
        warn!("TodoSave hook failed: {}", e);
    }
}
