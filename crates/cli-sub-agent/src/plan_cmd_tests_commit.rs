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

fn extract_bash_block(prompt: &str) -> String {
    let start = prompt
        .find("```bash\n")
        .expect("prompt should contain opening bash fence")
        + "```bash\n".len();
    let rest = &prompt[start..];
    let end = rest
        .find("\n```")
        .expect("prompt should contain closing fence");
    rest[..end].to_string()
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
    let _sandbox = ScopedSessionSandbox::new(&tmp).await;
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

#[cfg(unix)]
#[test]
fn commit_workflow_auto_pr_step_exits_before_push_in_executor_mode() {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    let workflow_path = workspace_root().join("patterns/commit/workflow.toml");
    let workflow = std::fs::read_to_string(&workflow_path).unwrap();
    let plan = plan_from_toml(&workflow).unwrap();
    let auto_pr_step = plan
        .steps
        .iter()
        .find(|step| step.title == "Auto PR Transaction")
        .expect("missing Auto PR Transaction step");
    let script = extract_bash_block(&auto_pr_step.prompt);

    let td = tempfile::tempdir().unwrap();
    let bin_dir = td.path().join("bin");
    let log_path = td.path().join("command.log");
    std::fs::create_dir_all(&bin_dir).unwrap();

    for tool in ["git", "gh"] {
        let tool_path = bin_dir.join(tool);
        std::fs::write(
            &tool_path,
            format!(
                "#!/bin/sh\nprintf '%s %s\\n' \"{tool}\" \"$*\" >> '{}'\nexit 99\n",
                log_path.display()
            ),
        )
        .unwrap();
        let mut perms = std::fs::metadata(&tool_path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&tool_path, perms).unwrap();
    }

    let inherited_path = std::env::var("PATH").unwrap_or_default();
    let patched_path = format!("{}:{inherited_path}", bin_dir.display());
    let status = Command::new("bash")
        .arg("-ceu")
        .arg(script)
        .current_dir(td.path())
        .env("PATH", patched_path)
        .env(
            "COMMIT_SUBJECT",
            "fix(cli-sub-agent): guard executor publish (#782)",
        )
        .env("BRANCH", "fix/executor-guard")
        .env("PR_BODY", "body")
        .env("CSA_DEPTH", "1")
        .env("CSA_INTERNAL_INVOCATION", "1")
        .status()
        .unwrap();

    assert!(status.success(), "executor-mode guard should exit cleanly");
    let log_contents = std::fs::read_to_string(&log_path).unwrap_or_default();
    assert!(
        log_contents.trim().is_empty(),
        "executor-mode auto PR step must exit before invoking git/gh, got: {log_contents}"
    );
}

#[test]
fn commit_pattern_step1_bridges_csa_skip_publish_to_skip_publish() {
    let pattern_path = workspace_root().join("patterns/commit/PATTERN.md");
    let pattern = std::fs::read_to_string(&pattern_path).unwrap();

    assert!(
        pattern.contains(": \"${FILES}\" \"${SCOPE}\" \"${BRANCH}\" \"${COMMIT_SUBJECT}\" \"${COMMIT_BODY}\" \"${COMMIT_MESSAGE_FILE}\" \"${IS_MILESTONE}\" \"${ENABLE_REVIEW_LOOP}\" \"${AUDIT_FAIL}\" \"${AUDIT_PASS_DEFERRED}\" \"${REVIEW_HAS_ISSUES}\" \"${PR_BODY}\" \"${SKIP_PUBLISH}\""),
        "PATTERN.md Step 1 must initialize SKIP_PUBLISH alongside the other mirrored workflow variables"
    );
    assert!(
        pattern.contains("if [ \"${CSA_SKIP_PUBLISH:-}\" = \"true\" ]; then\n  SKIP_PUBLISH=true"),
        "PATTERN.md Step 1 must bridge CSA_SKIP_PUBLISH=true into SKIP_PUBLISH=true"
    );
}

#[test]
fn commit_reviewer_guidance_schema_requires_regression_tests_for_timing_scenarios() {
    let commit_pattern =
        std::fs::read_to_string(workspace_root().join("patterns/commit/PATTERN.md")).unwrap();
    let commit_workflow =
        std::fs::read_to_string(workspace_root().join("patterns/commit/workflow.toml")).unwrap();
    let commit_skill =
        std::fs::read_to_string(workspace_root().join("patterns/commit/skills/commit/SKILL.md"))
            .unwrap();
    let review_pattern =
        std::fs::read_to_string(workspace_root().join("patterns/ai-reviewed-commit/PATTERN.md"))
            .unwrap();
    let review_workflow =
        std::fs::read_to_string(workspace_root().join("patterns/ai-reviewed-commit/workflow.toml"))
            .unwrap();
    let review_skill = std::fs::read_to_string(
        workspace_root().join("patterns/ai-reviewed-commit/skills/ai-reviewed-commit/SKILL.md"),
    )
    .unwrap();
    let commit_msg_script =
        std::fs::read_to_string(workspace_root().join("scripts/gen_commit_msg.sh")).unwrap();

    for content in [
        &commit_pattern,
        &commit_workflow,
        &commit_skill,
        &review_pattern,
        &review_workflow,
        &review_skill,
        &commit_msg_script,
    ] {
        assert!(
            content.contains("Regression Tests Added"),
            "updated reviewer-guidance schema must mention Regression Tests Added"
        );
    }

    for content in [&commit_pattern, &commit_workflow] {
        assert!(
            content.contains("Regression Tests Added must list concrete test names when Timing/Race Scenarios is not 'none'."),
            "commit gate must reject timing/race guidance without named regression tests"
        );
    }

    for content in [&review_pattern, &review_workflow, &review_skill] {
        assert!(
            content.contains("matching regression tests exist")
                && content.contains("Regression Tests Added"),
            "ai-reviewed-commit review guidance must require matching regression tests"
        );
    }

    assert!(
        !commit_pattern.contains("Risk Areas:") && !commit_workflow.contains("Risk Areas:"),
        "commit pattern/workflow should no longer require the old Risk Areas reviewer-guidance field"
    );
}

#[test]
fn commit_workflow_step17_requires_ai_reviewer_metadata_marker() {
    let commit_pattern =
        std::fs::read_to_string(workspace_root().join("patterns/commit/PATTERN.md")).unwrap();
    let commit_workflow =
        std::fs::read_to_string(workspace_root().join("patterns/commit/workflow.toml")).unwrap();

    for content in [&commit_pattern, &commit_workflow] {
        assert!(
            content.contains(
                "Step 17 requires upstream COMMIT_BODY from the AI-generated commit body step."
            ),
            "commit pattern/workflow step 17 must fail fast when upstream COMMIT_BODY is missing"
        );
        assert!(
            content
                .contains("Step 17: commit body missing required '### AI Reviewer Metadata' block"),
            "commit pattern/workflow step 17 must emit the explicit AI Reviewer Metadata error"
        );
        assert!(
            !content.contains("scripts/gen_commit_msg.sh --body"),
            "commit pattern/workflow step 17 must not synthesize a fallback commit body"
        );
        assert!(
            content.contains("See patterns/commit/PATTERN.md step 17/18."),
            "commit pattern/workflow step 17 must reference the renumbered PATTERN.md steps"
        );
        assert!(
            content.contains(
                "Commit body must include a descriptive summary before the AI Reviewer Metadata block."
            ),
            "commit pattern/workflow step 17 must require a descriptive summary before metadata"
        );
    }
}

#[test]
fn commit_workflow_followup_step_hints_match_renumbered_pattern() {
    let commit_pattern =
        std::fs::read_to_string(workspace_root().join("patterns/commit/PATTERN.md")).unwrap();
    let commit_workflow =
        std::fs::read_to_string(workspace_root().join("patterns/commit/workflow.toml")).unwrap();

    for content in [&commit_pattern, &commit_workflow] {
        assert!(
            content.contains("cumulative branch review (Step 21) or publish"),
            "commit pattern/workflow must point commit follow-up hints at step 21"
        );
        assert!(
            content.contains("push and create PR (Step 22)"),
            "commit pattern/workflow must point publish follow-up hints at step 22"
        );
        assert!(
            !content.contains("cumulative branch review (Step 18) or publish"),
            "commit pattern/workflow must not retain the stale step-18 follow-up hint"
        );
        assert!(
            !content.contains("push and create PR (Step 19)"),
            "commit pattern/workflow must not retain the stale step-19 follow-up hint"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn commit_workflow_test_gate_aborts_before_following_steps() {
    use std::os::unix::fs::PermissionsExt;
    use weave::compiler::ExecutionPlan;

    let td = tempfile::tempdir().unwrap();
    let bin_dir = td.path().join("bin");
    let marker = td.path().join("should-not-exist");
    std::fs::create_dir_all(&bin_dir).unwrap();

    let just_path = bin_dir.join("just");
    std::fs::write(
        &just_path,
        "#!/bin/sh\nif [ \"$1\" = \"test\" ]; then\n  echo 'failing just test' >&2\n  exit 1\nfi\nexit 0\n",
    )
    .unwrap();
    let mut just_perms = std::fs::metadata(&just_path).unwrap().permissions();
    just_perms.set_mode(0o755);
    std::fs::set_permissions(&just_path, just_perms).unwrap();

    let inherited_path = std::env::var("PATH").unwrap_or_default();
    let patched_path = format!("{}:{inherited_path}", bin_dir.display());

    let plan = ExecutionPlan {
        name: "commit-test-gate".into(),
        description: "Verify a failing just test aborts the workflow.".into(),
        variables: vec![],
        steps: vec![
            PlanStep {
                id: 1,
                title: "Run Tests".into(),
                tool: Some("bash".into()),
                prompt: "```bash\nset -o pipefail\njust test 2>&1\n```".into(),
                tier: None,
                depends_on: vec![],
                on_fail: FailAction::Abort,
                condition: None,
                loop_var: None,
                session: None,
            },
            PlanStep {
                id: 2,
                title: "Commit Marker".into(),
                tool: Some("bash".into()),
                prompt: format!("```bash\ntouch {}\n```", marker.display()),
                tier: None,
                depends_on: vec![],
                on_fail: FailAction::Abort,
                condition: None,
                loop_var: None,
                session: None,
            },
        ],
    };

    let vars = HashMap::from([("PATH".to_string(), patched_path)]);
    let results = execute_plan(&plan, &vars, td.path(), None, None)
        .await
        .unwrap();

    assert_eq!(
        results.len(),
        1,
        "workflow should stop after failing test gate"
    );
    assert_eq!(results[0].title, "Run Tests");
    assert_ne!(results[0].exit_code, 0, "test gate must report failure");
    assert!(
        !marker.exists(),
        "subsequent steps must not run after a failing just test gate"
    );
}
