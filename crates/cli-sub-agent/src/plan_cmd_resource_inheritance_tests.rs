use super::plan_cmd_steps::{StepExecutionContext, execute_step_with_workflow};
use super::*;
use weave::compiler::{FailAction, PlanStep};

#[tokio::test]
async fn plan_memory_override_reaches_nested_review_command_boundary() {
    let tmp = tempfile::tempdir().unwrap();
    let workflow_path = tmp.path().join("workflow.toml");
    let step = PlanStep {
        id: 12,
        title: "Cumulative main...HEAD review".into(),
        tool: Some("bash".into()),
        prompt: r#"```bash
csa() {
    test "$1" = "review"
    value="${CSA_INHERITED_RESOURCE_OVERRIDES:-}"
    test -n "$value" || value='{"memory_max_mb":10000}'
    printf '%s' "$value"
}
csa review --range main...HEAD
```"#
            .into(),
        tier: None,
        depends_on: vec![],
        on_fail: FailAction::Abort,
        condition: None,
        loop_var: None,
        session: None,
        workspace_access: None,
    };
    let vars = HashMap::new();
    let startup_env = crate::startup_env::StartupSubtreeEnv::default();

    let result = execute_step_with_workflow(
        &step,
        &vars,
        &StepExecutionContext {
            project_root: tmp.path(),
            workflow_path: &workflow_path,
            config: None,
            global_config: test_global_config(),
            model_catalog: test_model_catalog(),
            tool_override: None,
            model_spec_override: None,
            no_fs_sandbox: false,
            resources: RunResourceOverrides::new(Some(17_000), None),
            startup_env: &startup_env,
        },
    )
    .await;

    assert_eq!(result.exit_code, 0, "nested review probe must run");
    assert_eq!(
        result.output.as_deref(),
        Some(r#"{"memory_max_mb":17000}"#),
        "the dev2merge review child must inherit the plan's explicit 17000 MB override"
    );
}

#[tokio::test]
async fn generic_plan_run_and_review_children_share_the_inherited_snapshot() {
    let tmp = tempfile::tempdir().unwrap();
    let workflow_path = tmp.path().join("workflow.toml");
    let step = PlanStep {
        id: 3,
        title: "Nested run and review".into(),
        tool: Some("bash".into()),
        prompt: r#"```bash
csa() {
    printf '%s=%s\n' "$1" "$CSA_INHERITED_RESOURCE_OVERRIDES"
}
csa run
csa review
```"#
            .into(),
        tier: None,
        depends_on: vec![],
        on_fail: FailAction::Abort,
        condition: None,
        loop_var: None,
        session: None,
        workspace_access: None,
    };
    let startup_env = crate::startup_env::StartupSubtreeEnv::default();

    let result = execute_step_with_workflow(
        &step,
        &HashMap::new(),
        &StepExecutionContext {
            project_root: tmp.path(),
            workflow_path: &workflow_path,
            config: None,
            global_config: test_global_config(),
            model_catalog: test_model_catalog(),
            tool_override: None,
            model_spec_override: None,
            no_fs_sandbox: false,
            resources: RunResourceOverrides::new(Some(17_000), Some(2048)),
            startup_env: &startup_env,
        },
    )
    .await;

    assert_eq!(result.exit_code, 0);
    assert_eq!(
        result.output.as_deref(),
        Some(
            "run={\"memory_max_mb\":17000,\"min_free_memory_mb\":2048}\n\
             review={\"memory_max_mb\":17000,\"min_free_memory_mb\":2048}\n"
        )
    );
}

#[tokio::test]
async fn plan_retry_reuses_the_same_inherited_resource_snapshot() {
    let tmp = tempfile::tempdir().unwrap();
    let workflow_path = tmp.path().join("workflow.toml");
    let marker = tmp.path().join("first-attempt");
    let step = PlanStep {
        id: 4,
        title: "Retry nested child".into(),
        tool: Some("bash".into()),
        prompt: format!(
            "```bash\nif test ! -e '{}'; then touch '{}'; exit 1; fi\nprintf '%s' \"$CSA_INHERITED_RESOURCE_OVERRIDES\"\n```",
            marker.display(),
            marker.display()
        ),
        tier: None,
        depends_on: vec![],
        on_fail: FailAction::Retry(2),
        condition: None,
        loop_var: None,
        session: None,
        workspace_access: None,
    };
    let startup_env = crate::startup_env::StartupSubtreeEnv::default();

    let result = execute_step_with_workflow(
        &step,
        &HashMap::new(),
        &StepExecutionContext {
            project_root: tmp.path(),
            workflow_path: &workflow_path,
            config: None,
            global_config: test_global_config(),
            model_catalog: test_model_catalog(),
            tool_override: None,
            model_spec_override: None,
            no_fs_sandbox: false,
            resources: RunResourceOverrides::new(Some(17_000), None),
            startup_env: &startup_env,
        },
    )
    .await;

    assert_eq!(result.exit_code, 0, "the second attempt should succeed");
    assert_eq!(result.output.as_deref(), Some(r#"{"memory_max_mb":17000}"#));
}

#[test]
fn load_plan_resume_context_restores_explicit_resource_snapshot() {
    let tmp = tempfile::tempdir().unwrap();
    let workflow_path = tmp.path().join("workflow.toml");
    std::fs::write(&workflow_path, "[workflow]\nname='test'\n").unwrap();
    let plan = ExecutionPlan {
        name: "test".into(),
        description: String::new(),
        variables: vec![],
        steps: vec![],
    };
    let journal_path = tmp.path().join("test.journal.json");
    let mut journal = PlanRunJournal::new("test", &workflow_path, HashMap::new());
    journal.resource_overrides = RunResourceOverrides::new(Some(17_000), Some(2048));
    persist_plan_journal(&journal_path, &journal).unwrap();

    let context =
        load_plan_resume_context(&plan, &workflow_path, &journal_path, &HashMap::new(), true)
            .unwrap();

    assert_eq!(
        context
            .resource_overrides
            .resolve_memory_max_mb(None, "codex"),
        Some(17_000)
    );
    assert_eq!(
        context.resource_overrides.resolve_min_free_memory_mb(None),
        2048
    );
}
