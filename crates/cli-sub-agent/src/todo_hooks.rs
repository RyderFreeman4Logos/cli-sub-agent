use csa_hooks::{HookEvent, global_hooks_path, load_hooks_config, run_hooks_for_event};
use std::collections::HashMap;
use std::path::Path;
use tracing::warn;

/// Fire the `TodoSave` hook for a completed save/persist.
///
/// `version` is the saved-version count for THIS save and MUST be supplied by
/// the caller (the value `docs/hooks.md` documents as "Number of saved versions
/// after this save"). The caller is responsible for computing it at the correct
/// moment: `csa todo persist` computes it INSIDE the held write lock (right
/// after its commit) so a concurrent TODO writer that commits another version
/// after the lock is released cannot make this hook report the later count
/// (#1822 round-6 concurrency finding). This helper deliberately does NOT
/// recompute `version` from mutable git history, because doing so after the
/// lock is released reintroduces exactly that race.
pub(crate) fn emit_todo_save_hook(
    project_root: &Path,
    todo_root: &Path,
    plan_id: &str,
    version: usize,
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
