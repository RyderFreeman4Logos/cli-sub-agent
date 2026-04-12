use super::*;
#[cfg(unix)]
use crate::test_session_sandbox::ScopedSessionSandbox;
use std::collections::HashMap;
#[cfg(unix)]
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use weave::compiler::{FailAction, PlanStep, plan_from_toml};

#[cfg(unix)]
struct ScopedEnvVarRestore {
    key: &'static str,
    original: Option<String>,
}

#[cfg(unix)]
impl ScopedEnvVarRestore {
    fn set(key: &'static str, value: &str) -> Self {
        let original = std::env::var(key).ok();
        // SAFETY: test-scoped env mutation while ScopedSessionSandbox holds TEST_ENV_LOCK.
        unsafe { std::env::set_var(key, value) };
        Self { key, original }
    }
}

#[cfg(unix)]
impl Drop for ScopedEnvVarRestore {
    fn drop(&mut self) {
        // SAFETY: restoration of test-scoped env mutation while TEST_ENV_LOCK is held.
        unsafe {
            match self.original.take() {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

fn workspace_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

#[test]
fn commit_workflow_csa_dispatch_steps_have_non_empty_prompts() {
    let workflow_path = workspace_root().join("patterns/commit/workflow.toml");
    let workflow = std::fs::read_to_string(&workflow_path).unwrap();
    let plan = plan_from_toml(&workflow).unwrap();

    for title in ["Security Audit", "Pre-Commit Review"] {
        let step = plan
            .steps
            .iter()
            .find(|step| step.title == title)
            .unwrap_or_else(|| panic!("missing commit workflow step '{title}'"));
        assert_eq!(step.tool.as_deref(), Some("csa"));
        assert!(
            !step.prompt.trim().is_empty(),
            "commit workflow step '{title}' must not dispatch an empty CSA prompt"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn execute_step_csa_nested_plan_uses_fresh_child_session() {
    let tmp = tempfile::tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new(&tmp);
    let project_root = tmp.path();

    let bin_dir = project_root.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let fake_opencode = bin_dir.join("opencode");
    fs::write(&fake_opencode, "#!/bin/sh\nprintf 'plan-child-ok\\n'\n").unwrap();
    let mut perms = fs::metadata(&fake_opencode).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&fake_opencode, perms).unwrap();

    let inherited_path = std::env::var("PATH").unwrap_or_default();
    let patched_path = format!("{}:{inherited_path}", bin_dir.display());
    let _path_guard = ScopedEnvVarRestore::set("PATH", &patched_path);

    let parent =
        csa_session::create_session(project_root, Some("plan-parent"), None, Some("opencode"))
            .unwrap();
    let parent_dir = csa_session::get_session_dir(project_root, &parent.meta_session_id).unwrap();
    let _parent_lock = csa_lock::acquire_lock(&parent_dir, "opencode", "outer plan step").unwrap();

    let parent_dir_str = parent_dir.display().to_string();
    let project_root_str = project_root.display().to_string();
    let _csa_session_id = ScopedEnvVarRestore::set("CSA_SESSION_ID", &parent.meta_session_id);
    let _daemon_session_id =
        ScopedEnvVarRestore::set("CSA_DAEMON_SESSION_ID", &parent.meta_session_id);
    let _daemon_session_dir = ScopedEnvVarRestore::set("CSA_DAEMON_SESSION_DIR", &parent_dir_str);
    let _daemon_project_root =
        ScopedEnvVarRestore::set("CSA_DAEMON_PROJECT_ROOT", &project_root_str);

    let step = PlanStep {
        id: 1,
        title: "nested-plan-child".into(),
        tool: Some("opencode".into()),
        prompt: "Inspect the staged diff and report success.".into(),
        tier: None,
        depends_on: vec![],
        on_fail: FailAction::Abort,
        condition: None,
        loop_var: None,
        session: None,
    };

    let vars = HashMap::new();
    let result = execute_step(&step, &vars, project_root, None, None).await;

    assert_eq!(
        result.exit_code, 0,
        "nested plan-step CSA execution should succeed instead of self-locking: error={:?}",
        result.error
    );
    let child_id = result
        .session_id
        .as_deref()
        .expect("plan step should record child session id");
    assert_ne!(
        child_id, parent.meta_session_id,
        "plan step must allocate a fresh child session instead of reusing the daemon session"
    );

    let child = csa_session::load_session(project_root, child_id).unwrap();
    assert_eq!(
        child.genealogy.parent_session_id.as_deref(),
        Some(parent.meta_session_id.as_str()),
        "fresh plan child session should point back to the plan runner session"
    );
}
