use super::*;
use crate::test_env_lock::{TEST_ENV_LOCK, isolate_user_config};
use std::collections::HashMap;
use std::sync::{Arc, Barrier};
use weave::compiler::{ExecutionPlan, FailAction, PlanStep, plan_from_toml};

fn manual_handoff_plan() -> ExecutionPlan {
    ExecutionPlan {
        name: "manual-handoff".into(),
        description: String::new(),
        variables: vec![],
        steps: vec![
            PlanStep {
                id: 1,
                title: "manual-step".into(),
                tool: Some("manual".into()),
                prompt: "Use mktsk in the main agent, then resume.".into(),
                tier: None,
                depends_on: vec![],
                on_fail: FailAction::Abort,
                condition: None,
                loop_var: None,
                session: None,
                workspace_access: None,
            },
            PlanStep {
                id: 2,
                title: "should-not-run".into(),
                tool: Some("bash".into()),
                prompt: "```bash\ntouch should-not-run\n```".into(),
                tier: None,
                depends_on: vec![],
                on_fail: FailAction::Abort,
                condition: None,
                loop_var: None,
                session: None,
                workspace_access: None,
            },
        ],
    }
}

fn write_manual_handoff_workflow(project_root: &Path) -> std::path::PathBuf {
    write_manual_handoff_workflow_with_manual_step_id(project_root, 1)
}

fn write_manual_handoff_append_workflow(project_root: &Path) -> std::path::PathBuf {
    let workflow_path = project_root.join("workflow.toml");
    std::fs::write(
        &workflow_path,
        r#"[workflow]
name = "manual-handoff"

[[workflow.steps]]
id = 1
title = "manual-step"
tool = "manual"
prompt = """
Use mktsk in the main agent, then resume.
"""
on_fail = "abort"

[[workflow.steps]]
id = 2
title = "append-after-manual"
tool = "bash"
prompt = '''
```bash
sleep 0.1
printf 'continued\n' >> continued.txt
```
'''
on_fail = "abort"
"#,
    )
    .unwrap();
    workflow_path
}

fn write_manual_handoff_workflow_with_manual_step_id(
    project_root: &Path,
    manual_step_id: usize,
) -> std::path::PathBuf {
    let workflow_path = project_root.join("workflow.toml");
    std::fs::write(
        &workflow_path,
        format!(
            r#"[workflow]
name = "manual-handoff"

[[workflow.steps]]
id = {manual_step_id}
title = "manual-step"
tool = "manual"
prompt = """
Use mktsk in the main agent, then resume.
"""
on_fail = "abort"

[[workflow.steps]]
id = 2
title = "continue-after-manual"
tool = "bash"
prompt = '''
```bash
printf 'continued\n' > continued.txt
```
'''
on_fail = "abort"
"#,
        ),
    )
    .unwrap();
    workflow_path
}

fn plan_run_args(project_root: &Path, workflow_path: &Path) -> PlanRunArgs {
    PlanRunArgs {
        file: Some(workflow_path.display().to_string()),
        pattern: None,
        vars: vec![],
        tool_override: None,
        model_spec_override: None,
        dry_run: false,
        chunked: false,
        resume: None,
        complete_manual_step: None,
        cd: Some(project_root.display().to_string()),
        no_fs_sandbox: false,
        resources: RunResourceOverrides::absent(),
        current_depth: 0,
        pipeline_source: PlanRunPipelineSource::DirectPlanRun,
        startup_env: crate::startup_env::StartupSubtreeEnv::default(),
    }
}

#[tokio::test]
async fn execute_plan_stops_after_manual_handoff() {
    let plan = manual_handoff_plan();
    let vars = HashMap::new();
    let tmp = tempfile::tempdir().unwrap();
    let workflow_path = tmp.path().join("workflow.toml");
    let journal_path = tmp.path().join("manual-handoff.journal.json");
    std::fs::write(&workflow_path, "[workflow]\nname='manual-handoff'\n").unwrap();
    let completed = std::collections::HashSet::new();
    let mut journal = PlanRunJournal::new("manual-handoff", &workflow_path, vars.clone());
    let mut run_ctx = PlanRunContext {
        project_root: tmp.path(),
        workflow_path: &workflow_path,
        config: None,
        global_config: super::test_global_config(),
        model_catalog: super::test_model_catalog(),
        tool_override: None,
        model_spec_override: None,
        journal: &mut journal,
        journal_path: Some(&journal_path),
        resume_completed_steps: &completed,
        chunked: false,
        no_fs_sandbox: false,
        resources: RunResourceOverrides::absent(),
        startup_env: &crate::startup_env::EMPTY_STARTUP_SUBTREE_ENV,
    };

    let results = execute_plan_with_journal(&plan, &vars, &mut run_ctx)
        .await
        .expect("manual handoff plan should execute");

    assert_eq!(results.len(), 1, "manual handoff must pause the workflow");
    assert_eq!(journal.status, "manual-handoff");
    assert!(
        !journal.completed_steps.contains(&1),
        "manual handoff should not mark the step completed"
    );
    assert!(!tmp.path().join("should-not-run").exists());
}

#[tokio::test]
async fn handle_plan_run_complete_manual_step_continues_after_pending_handoff() {
    let tmp = tempfile::tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.lock().await;
    let _user_config_env = isolate_user_config(tmp.path());
    let workflow_path = write_manual_handoff_workflow(tmp.path());

    handle_plan_run(plan_run_args(tmp.path(), &workflow_path))
        .await
        .expect("initial run should pause at manual handoff");

    let journal_path = plan_journal_path(tmp.path(), "manual-handoff");
    let paused_journal: PlanRunJournal =
        serde_json::from_slice(&std::fs::read(&journal_path).unwrap()).unwrap();
    assert_eq!(paused_journal.status, "manual-handoff");
    assert!(
        !paused_journal.completed_steps.contains(&1),
        "manual step must remain pending until the caller explicitly completes it"
    );
    assert!(
        !tmp.path().join("continued.txt").exists(),
        "workflow must not advance past manual handoff on the first run"
    );

    let mut resume_args = plan_run_args(tmp.path(), &workflow_path);
    resume_args.file = None;
    resume_args.resume = Some(journal_path.display().to_string());
    resume_args.complete_manual_step = Some(1);

    handle_plan_run(resume_args)
        .await
        .expect("explicit manual completion should continue the workflow");

    assert_eq!(
        std::fs::read_to_string(tmp.path().join("continued.txt")).unwrap(),
        "continued\n"
    );
    let completed_journal: PlanRunJournal =
        serde_json::from_slice(&std::fs::read(&journal_path).unwrap()).unwrap();
    assert_eq!(completed_journal.status, "completed");
    assert!(completed_journal.completed_steps.contains(&1));
    assert!(completed_journal.completed_steps.contains(&2));
}

#[tokio::test]
async fn handle_plan_run_concurrent_complete_manual_step_runs_post_manual_steps_once() {
    let tmp = tempfile::tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.lock().await;
    let _user_config_env = isolate_user_config(tmp.path());
    let workflow_path = write_manual_handoff_append_workflow(tmp.path());

    handle_plan_run(plan_run_args(tmp.path(), &workflow_path))
        .await
        .expect("initial run should pause at manual handoff");

    let journal_path = plan_journal_path(tmp.path(), "manual-handoff");
    let first_args = {
        let mut args = plan_run_args(tmp.path(), &workflow_path);
        args.file = None;
        args.resume = Some(journal_path.display().to_string());
        args.complete_manual_step = Some(1);
        args
    };
    let barrier = Arc::new(Barrier::new(3));
    let first_barrier = Arc::clone(&barrier);
    let first = std::thread::spawn(move || {
        first_barrier.wait();
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(handle_plan_run(first_args))
    });
    let second_barrier = Arc::clone(&barrier);
    let second_args = {
        let mut args = plan_run_args(tmp.path(), &workflow_path);
        args.file = None;
        args.resume = Some(journal_path.display().to_string());
        args.complete_manual_step = Some(1);
        args
    };
    let second = std::thread::spawn(move || {
        second_barrier.wait();
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(handle_plan_run(second_args))
    });

    barrier.wait();
    let first_result = first.join().expect("first resume thread should not panic");
    let second_result = second
        .join()
        .expect("second resume thread should not panic");
    let success_count = usize::from(first_result.is_ok()) + usize::from(second_result.is_ok());
    assert_eq!(
        success_count, 1,
        "exactly one concurrent manual completion should own the journal transition; first={first_result:?}, second={second_result:?}"
    );

    assert_eq!(
        std::fs::read_to_string(tmp.path().join("continued.txt")).unwrap(),
        "continued\n",
        "post-manual workflow steps must not rerun after a stale completion attempt"
    );
    let completed_journal: PlanRunJournal =
        serde_json::from_slice(&std::fs::read(&journal_path).unwrap()).unwrap();
    assert_eq!(completed_journal.status, "completed");
    assert!(completed_journal.completed_steps.contains(&1));
    assert!(completed_journal.completed_steps.contains(&2));
}

#[tokio::test]
async fn handle_plan_run_complete_manual_step_accepts_zero_step_id() {
    let tmp = tempfile::tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.lock().await;
    let _user_config_env = isolate_user_config(tmp.path());
    let workflow_path = write_manual_handoff_workflow_with_manual_step_id(tmp.path(), 0);

    handle_plan_run(plan_run_args(tmp.path(), &workflow_path))
        .await
        .expect("initial run should pause at step-zero manual handoff");

    let journal_path = plan_journal_path(tmp.path(), "manual-handoff");
    let mut resume_args = plan_run_args(tmp.path(), &workflow_path);
    resume_args.file = None;
    resume_args.resume = Some(journal_path.display().to_string());
    resume_args.complete_manual_step = Some(0);

    handle_plan_run(resume_args)
        .await
        .expect("step zero is a valid workflow step id and should complete explicitly");

    assert_eq!(
        std::fs::read_to_string(tmp.path().join("continued.txt")).unwrap(),
        "continued\n"
    );
    let completed_journal: PlanRunJournal =
        serde_json::from_slice(&std::fs::read(&journal_path).unwrap()).unwrap();
    assert_eq!(completed_journal.status, "completed");
    assert!(completed_journal.completed_steps.contains(&0));
    assert!(completed_journal.completed_steps.contains(&2));
}

#[tokio::test]
async fn complete_manual_step_rejects_non_pending_step_id() {
    let tmp = tempfile::tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.lock().await;
    let _user_config_env = isolate_user_config(tmp.path());
    let workflow_path = write_manual_handoff_workflow(tmp.path());
    let plan = plan_from_toml(&std::fs::read_to_string(&workflow_path).unwrap()).unwrap();

    handle_plan_run(plan_run_args(tmp.path(), &workflow_path))
        .await
        .expect("initial run should pause at manual handoff");

    let journal_path = plan_journal_path(tmp.path(), "manual-handoff");
    let err = complete_pending_manual_step(&plan, &workflow_path, &journal_path, 2)
        .expect_err("wrong step id must not complete the manual handoff");

    assert!(
        err.to_string().contains("pending step is 1"),
        "unexpected error: {err:#}"
    );
    let journal: PlanRunJournal =
        serde_json::from_slice(&std::fs::read(&journal_path).unwrap()).unwrap();
    assert_eq!(journal.status, "manual-handoff");
    assert!(!journal.completed_steps.contains(&1));
    assert!(!journal.completed_steps.contains(&2));
}

#[tokio::test]
async fn execute_plan_resume_replays_manual_handoff_step() {
    let plan = manual_handoff_plan();
    let vars = HashMap::new();
    let tmp = tempfile::tempdir().unwrap();
    let workflow_path = tmp.path().join("workflow.toml");
    let journal_path = tmp.path().join("manual-handoff.journal.json");
    std::fs::write(&workflow_path, "[workflow]\nname='manual-handoff'\n").unwrap();

    {
        let completed = std::collections::HashSet::new();
        let mut journal = PlanRunJournal::new("manual-handoff", &workflow_path, vars.clone());
        let mut run_ctx = PlanRunContext {
            project_root: tmp.path(),
            workflow_path: &workflow_path,
            config: None,
            global_config: super::test_global_config(),
            model_catalog: super::test_model_catalog(),
            tool_override: None,
            model_spec_override: None,
            journal: &mut journal,
            journal_path: Some(&journal_path),
            resume_completed_steps: &completed,
            chunked: false,
            no_fs_sandbox: false,
            resources: RunResourceOverrides::absent(),
            startup_env: &crate::startup_env::EMPTY_STARTUP_SUBTREE_ENV,
        };

        let results = execute_plan_with_journal(&plan, &vars, &mut run_ctx)
            .await
            .expect("initial manual handoff should execute");

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].step_id, 1);
    }

    let saved_journal: PlanRunJournal =
        serde_json::from_slice(&std::fs::read(&journal_path).unwrap()).unwrap();
    assert_eq!(saved_journal.status, "manual-handoff");
    assert!(
        !saved_journal.completed_steps.contains(&1),
        "manual handoff should stay pending in the journal"
    );

    let resumed_completed: std::collections::HashSet<usize> =
        saved_journal.completed_steps.iter().copied().collect();
    let mut resumed_journal =
        PlanRunJournal::new("manual-handoff", &workflow_path, saved_journal.vars.clone());
    resumed_journal.completed_steps = saved_journal.completed_steps.clone();
    let mut resumed_ctx = PlanRunContext {
        project_root: tmp.path(),
        workflow_path: &workflow_path,
        config: None,
        global_config: super::test_global_config(),
        model_catalog: super::test_model_catalog(),
        tool_override: None,
        model_spec_override: None,
        journal: &mut resumed_journal,
        journal_path: Some(&journal_path),
        resume_completed_steps: &resumed_completed,
        chunked: false,
        no_fs_sandbox: false,
        resources: RunResourceOverrides::absent(),
        startup_env: &crate::startup_env::EMPTY_STARTUP_SUBTREE_ENV,
    };

    let resumed_results = execute_plan_with_journal(&plan, &saved_journal.vars, &mut resumed_ctx)
        .await
        .expect("explicit resume should replay the manual handoff step");

    assert_eq!(
        resumed_results.len(),
        1,
        "resume should stop at the repeated manual handoff"
    );
    assert_eq!(resumed_results[0].step_id, 1);
    assert_eq!(resumed_journal.status, "manual-handoff");
    assert!(
        !resumed_journal.completed_steps.contains(&1),
        "replayed manual handoff should still remain pending"
    );
    assert!(!tmp.path().join("should-not-run").exists());
}
