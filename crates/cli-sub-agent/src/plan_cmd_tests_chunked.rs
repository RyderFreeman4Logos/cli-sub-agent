use super::*;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use weave::compiler::{FailAction, PlanStep};

// ---- Chunked execution mode tests ----

/// Helper to create a PlanRunContext with chunked mode and optional journal persistence.
fn make_chunked_run_ctx<'a>(
    project_root: &'a Path,
    workflow_path: &'a Path,
    journal: &'a mut PlanRunJournal,
    journal_path: Option<&'a Path>,
    resume_completed_steps: &'a HashSet<usize>,
    chunked: bool,
) -> PlanRunContext<'a> {
    PlanRunContext {
        project_root,
        workflow_path,
        config: None,
        tool_override: None,
        journal,
        journal_path,
        resume_completed_steps,
        chunked,
    }
}

#[tokio::test]
async fn execute_plan_chunked_single_step() {
    // A 2-step bash workflow in chunked mode should only execute the first step.
    let plan = ExecutionPlan {
        name: "chunked-test".into(),
        description: String::new(),
        variables: vec![],
        steps: vec![
            PlanStep {
                id: 1,
                title: "step one".into(),
                tool: Some("bash".into()),
                prompt: "```bash\necho step-one-ran\n```".into(),
                tier: None,
                depends_on: vec![],
                on_fail: FailAction::Abort,
                condition: None,
                loop_var: None,
                session: None,
            },
            PlanStep {
                id: 2,
                title: "step two".into(),
                tool: Some("bash".into()),
                prompt: "```bash\necho step-two-ran\n```".into(),
                tier: None,
                depends_on: vec![],
                on_fail: FailAction::Abort,
                condition: None,
                loop_var: None,
                session: None,
            },
        ],
    };
    let vars = HashMap::new();
    let tmp = tempfile::tempdir().unwrap();
    let workflow_path = tmp.path().join("workflow.toml");
    let journal_path = tmp.path().join("chunked-test.journal.json");
    let completed = HashSet::new();
    let mut journal = PlanRunJournal::new("chunked-test", &workflow_path, vars.clone());
    let mut run_ctx = make_chunked_run_ctx(
        tmp.path(),
        &workflow_path,
        &mut journal,
        Some(&journal_path),
        &completed,
        true,
    );

    let results = execute_plan_with_journal(&plan, &vars, &mut run_ctx)
        .await
        .unwrap();

    // Chunked mode: only 1 step executed, then break
    assert_eq!(results.len(), 1, "chunked mode must execute exactly 1 step");
    assert_eq!(results[0].step_id, 1, "first step should be step 1");
    assert_eq!(results[0].exit_code, 0, "step 1 should succeed");
    assert!(!results[0].skipped);
}

#[tokio::test]
async fn execute_plan_chunked_resume() {
    // 3-step workflow: execute step 1 (chunked), then resume+chunked for step 2, then step 3.
    let plan = ExecutionPlan {
        name: "chunked-resume".into(),
        description: String::new(),
        variables: vec![],
        steps: vec![
            PlanStep {
                id: 1,
                title: "step one".into(),
                tool: Some("bash".into()),
                prompt: "```bash\necho one\n```".into(),
                tier: None,
                depends_on: vec![],
                on_fail: FailAction::Abort,
                condition: None,
                loop_var: None,
                session: None,
            },
            PlanStep {
                id: 2,
                title: "step two".into(),
                tool: Some("bash".into()),
                prompt: "```bash\necho two\n```".into(),
                tier: None,
                depends_on: vec![],
                on_fail: FailAction::Abort,
                condition: None,
                loop_var: None,
                session: None,
            },
            PlanStep {
                id: 3,
                title: "step three".into(),
                tool: Some("bash".into()),
                prompt: "```bash\necho three\n```".into(),
                tier: None,
                depends_on: vec![],
                on_fail: FailAction::Abort,
                condition: None,
                loop_var: None,
                session: None,
            },
        ],
    };
    let vars = HashMap::new();
    let tmp = tempfile::tempdir().unwrap();
    let workflow_path = tmp.path().join("workflow.toml");
    let journal_path = tmp.path().join("chunked-resume.journal.json");

    // --- Round 1: execute step 1 ---
    {
        let completed = HashSet::new();
        let mut journal = PlanRunJournal::new("chunked-resume", &workflow_path, vars.clone());
        let mut run_ctx = make_chunked_run_ctx(
            tmp.path(),
            &workflow_path,
            &mut journal,
            Some(&journal_path),
            &completed,
            true,
        );
        let results = execute_plan_with_journal(&plan, &vars, &mut run_ctx)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].step_id, 1);
        assert_eq!(results[0].exit_code, 0);
    }

    // Read journal to get completed steps
    let journal_bytes = std::fs::read(&journal_path).unwrap();
    let saved_journal: PlanRunJournal = serde_json::from_slice(&journal_bytes).unwrap();
    assert!(
        saved_journal.completed_steps.contains(&1),
        "journal must record step 1 as completed"
    );

    // --- Round 2: resume from journal, execute step 2 ---
    {
        let completed: HashSet<usize> = saved_journal.completed_steps.iter().copied().collect();
        let mut journal =
            PlanRunJournal::new("chunked-resume", &workflow_path, saved_journal.vars.clone());
        journal.completed_steps = saved_journal.completed_steps.clone();
        let mut run_ctx = make_chunked_run_ctx(
            tmp.path(),
            &workflow_path,
            &mut journal,
            Some(&journal_path),
            &completed,
            true,
        );
        let results = execute_plan_with_journal(&plan, &saved_journal.vars, &mut run_ctx)
            .await
            .unwrap();
        assert_eq!(
            results.len(),
            1,
            "chunked resume must execute exactly 1 step"
        );
        assert_eq!(results[0].step_id, 2, "second round should execute step 2");
        assert_eq!(results[0].exit_code, 0);
    }

    // Read journal again
    let journal_bytes = std::fs::read(&journal_path).unwrap();
    let saved_journal2: PlanRunJournal = serde_json::from_slice(&journal_bytes).unwrap();
    assert!(saved_journal2.completed_steps.contains(&1));
    assert!(saved_journal2.completed_steps.contains(&2));

    // --- Round 3: resume from journal, execute step 3 (final) ---
    {
        let completed: HashSet<usize> = saved_journal2.completed_steps.iter().copied().collect();
        let mut journal = PlanRunJournal::new(
            "chunked-resume",
            &workflow_path,
            saved_journal2.vars.clone(),
        );
        journal.completed_steps = saved_journal2.completed_steps.clone();
        let mut run_ctx = make_chunked_run_ctx(
            tmp.path(),
            &workflow_path,
            &mut journal,
            Some(&journal_path),
            &completed,
            true,
        );
        let results = execute_plan_with_journal(&plan, &saved_journal2.vars, &mut run_ctx)
            .await
            .unwrap();
        assert_eq!(results.len(), 1, "final round must execute exactly 1 step");
        assert_eq!(results[0].step_id, 3, "third round should execute step 3");
        assert_eq!(results[0].exit_code, 0);
    }

    // Verify final journal has all 3 steps completed
    let journal_bytes = std::fs::read(&journal_path).unwrap();
    let final_journal: PlanRunJournal = serde_json::from_slice(&journal_bytes).unwrap();
    assert!(final_journal.completed_steps.contains(&1));
    assert!(final_journal.completed_steps.contains(&2));
    assert!(final_journal.completed_steps.contains(&3));
}

#[tokio::test]
async fn execute_plan_chunked_skips_condition_false_and_runs_next() {
    // In chunked mode, if the first eligible step is condition-false (skipped),
    // it should still only return 1 result per chunk (the first non-resume step).
    let plan = ExecutionPlan {
        name: "chunked-skip".into(),
        description: String::new(),
        variables: vec![],
        steps: vec![
            PlanStep {
                id: 1,
                title: "skipped step".into(),
                tool: Some("bash".into()),
                prompt: "```bash\necho should-not-run\n```".into(),
                tier: None,
                depends_on: vec![],
                on_fail: FailAction::Abort,
                condition: Some("${NONEXISTENT}".into()),
                loop_var: None,
                session: None,
            },
            PlanStep {
                id: 2,
                title: "real step".into(),
                tool: Some("bash".into()),
                prompt: "```bash\necho real-step-ran\n```".into(),
                tier: None,
                depends_on: vec![],
                on_fail: FailAction::Abort,
                condition: None,
                loop_var: None,
                session: None,
            },
        ],
    };
    let vars = HashMap::new();
    let tmp = tempfile::tempdir().unwrap();
    let workflow_path = tmp.path().join("workflow.toml");
    let completed = HashSet::new();
    let mut journal = PlanRunJournal::new("chunked-skip", &workflow_path, vars.clone());
    let mut run_ctx = make_chunked_run_ctx(
        tmp.path(),
        &workflow_path,
        &mut journal,
        None,
        &completed,
        true,
    );

    let results = execute_plan_with_journal(&plan, &vars, &mut run_ctx)
        .await
        .unwrap();

    // Chunked mode skips condition-false steps and continues to the next
    // executable step. Step 1 is skipped (condition-false), step 2 executes.
    assert_eq!(
        results.len(),
        2,
        "chunked mode should include skipped step + first executed step"
    );
    assert_eq!(results[0].step_id, 1);
    assert!(results[0].skipped, "condition-false step should be skipped");
    assert_eq!(results[1].step_id, 2);
    assert!(!results[1].skipped, "step 2 should actually execute");
    assert_eq!(results[1].exit_code, 0);
}

#[tokio::test]
async fn execute_plan_chunked_resume_skips_condition_false_no_infinite_loop() {
    // 3-step workflow: step 1 executes, step 2 has condition=false, step 3 executes.
    // Chunked resume after step 1 should skip step 2 and execute step 3 in one round.
    let plan = ExecutionPlan {
        name: "chunked-skip-resume".into(),
        description: String::new(),
        variables: vec![],
        steps: vec![
            PlanStep {
                id: 1,
                title: "step one".into(),
                tool: Some("bash".into()),
                prompt: "```bash\necho one\n```".into(),
                tier: None,
                depends_on: vec![],
                on_fail: FailAction::Abort,
                condition: None,
                loop_var: None,
                session: None,
            },
            PlanStep {
                id: 2,
                title: "conditional step".into(),
                tool: Some("bash".into()),
                prompt: "```bash\necho should-not-run\n```".into(),
                tier: None,
                depends_on: vec![],
                on_fail: FailAction::Abort,
                condition: Some("${NONEXISTENT}".into()),
                loop_var: None,
                session: None,
            },
            PlanStep {
                id: 3,
                title: "step three".into(),
                tool: Some("bash".into()),
                prompt: "```bash\necho three\n```".into(),
                tier: None,
                depends_on: vec![],
                on_fail: FailAction::Abort,
                condition: None,
                loop_var: None,
                session: None,
            },
        ],
    };
    let vars = HashMap::new();
    let tmp = tempfile::tempdir().unwrap();
    let workflow_path = tmp.path().join("workflow.toml");
    let journal_path = tmp.path().join("chunked-skip-resume.journal.json");

    // --- Round 1: execute step 1 (chunked) ---
    {
        let completed = HashSet::new();
        let mut journal = PlanRunJournal::new("chunked-skip-resume", &workflow_path, vars.clone());
        let mut run_ctx = make_chunked_run_ctx(
            tmp.path(),
            &workflow_path,
            &mut journal,
            Some(&journal_path),
            &completed,
            true,
        );
        let results = execute_plan_with_journal(&plan, &vars, &mut run_ctx)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].step_id, 1);
        assert_eq!(results[0].exit_code, 0);
        assert!(!results[0].skipped);
    }

    // Read journal — step 1 should be completed
    let journal_bytes = std::fs::read(&journal_path).unwrap();
    let saved_journal: PlanRunJournal = serde_json::from_slice(&journal_bytes).unwrap();
    assert!(
        saved_journal.completed_steps.contains(&1),
        "journal must record step 1 as completed"
    );

    // --- Round 2: resume → step 2 should be skipped, step 3 should execute ---
    {
        let completed: HashSet<usize> = saved_journal.completed_steps.iter().copied().collect();
        let mut journal = PlanRunJournal::new(
            "chunked-skip-resume",
            &workflow_path,
            saved_journal.vars.clone(),
        );
        journal.completed_steps = saved_journal.completed_steps.clone();
        let mut run_ctx = make_chunked_run_ctx(
            tmp.path(),
            &workflow_path,
            &mut journal,
            Some(&journal_path),
            &completed,
            true,
        );
        let results = execute_plan_with_journal(&plan, &saved_journal.vars, &mut run_ctx)
            .await
            .unwrap();

        // Step 2 is skipped (condition-false), step 3 executes — both in same chunk
        assert_eq!(
            results.len(),
            2,
            "resume round should include skipped step 2 + executed step 3"
        );
        assert_eq!(results[0].step_id, 2);
        assert!(
            results[0].skipped,
            "step 2 should be skipped (condition-false)"
        );
        assert_eq!(results[1].step_id, 3);
        assert!(!results[1].skipped, "step 3 should actually execute");
        assert_eq!(results[1].exit_code, 0);
    }

    // Verify final journal has all 3 steps completed (no infinite loop)
    let journal_bytes = std::fs::read(&journal_path).unwrap();
    let final_journal: PlanRunJournal = serde_json::from_slice(&journal_bytes).unwrap();
    assert!(final_journal.completed_steps.contains(&1));
    assert!(
        final_journal.completed_steps.contains(&2),
        "skipped step 2 must be recorded in journal to prevent infinite loop"
    );
    assert!(final_journal.completed_steps.contains(&3));
}
