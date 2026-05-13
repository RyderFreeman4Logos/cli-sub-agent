use super::*;
use std::collections::HashMap;
use weave::compiler::{ExecutionPlan, FailAction, PlanStep};

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
            },
        ],
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
        tool_override: None,
        model_spec_override: None,
        journal: &mut journal,
        journal_path: Some(&journal_path),
        resume_completed_steps: &completed,
        chunked: false,
        no_fs_sandbox: false,
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
            tool_override: None,
            model_spec_override: None,
            journal: &mut journal,
            journal_path: Some(&journal_path),
            resume_completed_steps: &completed,
            chunked: false,
            no_fs_sandbox: false,
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
        tool_override: None,
        model_spec_override: None,
        journal: &mut resumed_journal,
        journal_path: Some(&journal_path),
        resume_completed_steps: &resumed_completed,
        chunked: false,
        no_fs_sandbox: false,
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
