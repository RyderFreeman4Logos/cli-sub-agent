use super::*;
use std::collections::HashMap;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use weave::compiler::{FailAction, PlanStep, VariableDecl, plan_from_toml};

#[tokio::test]
async fn execute_step_skips_when_condition_is_false() {
    let step = PlanStep {
        id: 1,
        title: "conditional".into(),
        tool: Some("bash".into()),
        prompt: "echo test".into(),
        tier: None,
        depends_on: vec![],
        on_fail: FailAction::Abort,
        condition: Some("${SOME_VAR}".into()),
        loop_var: None,
        session: None,
    };
    let vars = HashMap::new();
    let tmp = tempfile::tempdir().unwrap();
    let result = execute_step(&step, &vars, tmp.path(), None, None).await;
    assert!(result.skipped, "unset condition var must skip");
    assert_eq!(
        result.exit_code, 0,
        "condition-false skip is intentional, not a failure"
    );
}

#[tokio::test]
async fn execute_step_runs_when_condition_is_true() {
    let step = PlanStep {
        id: 1,
        title: "conditional".into(),
        tool: Some("bash".into()),
        prompt: "```bash\necho hello\n```".into(),
        tier: None,
        depends_on: vec![],
        on_fail: FailAction::Abort,
        condition: Some("${FLAG}".into()),
        loop_var: None,
        session: None,
    };
    let mut vars = HashMap::new();
    vars.insert("FLAG".into(), "yes".into());
    let tmp = tempfile::tempdir().unwrap();
    let result = execute_step(&step, &vars, tmp.path(), None, None).await;
    assert!(!result.skipped, "true condition must execute step");
    assert_eq!(result.exit_code, 0, "bash echo should succeed");
}

#[tokio::test]
async fn execute_step_skips_loop_with_nonzero_exit() {
    let step = PlanStep {
        id: 1,
        title: "loop".into(),
        tool: Some("bash".into()),
        prompt: "echo test".into(),
        tier: None,
        depends_on: vec![],
        on_fail: FailAction::Abort,
        condition: None,
        loop_var: Some(weave::compiler::LoopSpec {
            variable: "item".into(),
            collection: "${ITEMS}".into(),
            max_iterations: 10,
        }),
        session: None,
    };
    let vars = HashMap::new();
    let tmp = tempfile::tempdir().unwrap();
    let result = execute_step(&step, &vars, tmp.path(), None, None).await;
    assert!(result.skipped);
    assert_ne!(result.exit_code, 0);
}

#[tokio::test]
async fn execute_step_skips_weave_include() {
    let step = PlanStep {
        id: 1,
        title: "include security-audit".into(),
        tool: Some("weave".into()),
        prompt: "INCLUDE security-audit".into(),
        tier: None,
        depends_on: vec![],
        on_fail: FailAction::Abort,
        condition: None,
        loop_var: None,
        session: None,
    };
    let vars = HashMap::new();
    let tmp = tempfile::tempdir().unwrap();
    let result = execute_step(&step, &vars, tmp.path(), None, None).await;
    assert!(result.skipped);
    assert_eq!(
        result.exit_code, 0,
        "INCLUDE skip should be success (harmless)"
    );
}

#[tokio::test]
async fn execute_step_bash_runs_code_block() {
    let step = PlanStep {
        id: 1,
        title: "echo test".into(),
        tool: Some("bash".into()),
        prompt:
            "Run this:\n```bash\nprintf 'hello' > code_block_output.txt\necho code-block-ran\n```\n"
                .into(),
        tier: None,
        depends_on: vec![],
        on_fail: FailAction::Abort,
        condition: None,
        loop_var: None,
        session: None,
    };
    let vars = HashMap::new();
    let tmp = tempfile::tempdir().unwrap();
    let result = execute_step(&step, &vars, tmp.path(), None, None).await;
    assert_eq!(
        result.exit_code, 0,
        "error={:?} output={:?}",
        result.error, result.output
    );
    assert!(
        result
            .output
            .as_deref()
            .unwrap_or("")
            .contains("code-block-ran"),
        "output should prove the fenced bash block ran"
    );
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("code_block_output.txt")).unwrap(),
        "hello"
    );
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn load_pr_bot_step_by_title(title: &str) -> PlanStep {
    let workflow_path = workspace_root().join("patterns/pr-bot/workflow.toml");
    let workflow = std::fs::read_to_string(&workflow_path).unwrap();
    let plan = plan_from_toml(&workflow).unwrap();
    let step = plan
        .steps
        .into_iter()
        .find(|step| step.title == title)
        .unwrap_or_else(|| panic!("missing pr-bot step '{title}'"));
    PlanStep {
        condition: None,
        loop_var: None,
        ..step
    }
}

#[test]
fn pr_bot_workflow_marks_non_ai_steps_explicitly() {
    let workflow_path = workspace_root().join("patterns/pr-bot/workflow.toml");
    let workflow = std::fs::read_to_string(&workflow_path).unwrap();
    let plan = plan_from_toml(&workflow).unwrap();

    assert!(
        plan.steps
            .iter()
            .all(|step| step.title != "Dispatcher Model Note"),
        "dispatcher note must remain informational markdown, not an executable step"
    );

    let setup_abort = plan
        .steps
        .iter()
        .find(|step| step.title == "Step 5a: Abort — Bot Needs Environment Configuration")
        .expect("missing bot setup abort step");
    assert_eq!(setup_abort.tool.as_deref(), Some("manual"));

    let clean_note = plan
        .steps
        .iter()
        .find(|step| step.title == "Step 10a: Bot Review Clean")
        .expect("missing bot review clean step");
    assert_eq!(clean_note.tool.as_deref(), Some("note"));
}

#[test]
fn dev2merge_workflow_marks_mktsk_step_manual() {
    let workflow_path = workspace_root().join("patterns/dev2merge/workflow.toml");
    let workflow = std::fs::read_to_string(&workflow_path).unwrap();
    let plan = plan_from_toml(&workflow).unwrap();

    let mktsk_step = plan
        .steps
        .iter()
        .find(|step| step.title == "Execute Plan with mktsk")
        .expect("missing dev2merge mktsk step");
    assert_eq!(mktsk_step.tool.as_deref(), Some("manual"));
}

#[test]
fn pr_bot_workflow_is_v1_loop_free() {
    let workflow_path = workspace_root().join("patterns/pr-bot/workflow.toml");
    let workflow = std::fs::read_to_string(&workflow_path).unwrap();
    let plan = plan_from_toml(&workflow).unwrap();

    let loop_steps: Vec<usize> = plan
        .steps
        .iter()
        .filter_map(|step| step.loop_var.as_ref().map(|_| step.id))
        .collect();

    assert!(
        loop_steps.is_empty(),
        "pr-bot must remain v1-compatible; loop_var found on steps {loop_steps:?}"
    );
}

fn install_fake_gh(bin_dir: &Path) -> PathBuf {
    let capture_path = bin_dir.join("gh-capture.md");
    let gh_path = bin_dir.join("gh");
    std::fs::write(
        &gh_path,
        r#"#!/usr/bin/env bash
set -euo pipefail
body_file=""
capture="${TEST_GH_CAPTURE:?missing TEST_GH_CAPTURE}"
while [ $# -gt 0 ]; do
  case "$1" in
    --body-file)
      body_file="$2"
      shift 2
      ;;
    *)
      shift
      ;;
  esac
done
if [ -z "${body_file}" ]; then
  echo "missing --body-file" >&2
  exit 1
fi
cp "${body_file}" "${capture}"
"#,
    )
    .unwrap();
    #[cfg(unix)]
    {
        let mut perms = std::fs::metadata(&gh_path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&gh_path, perms).unwrap();
    }
    capture_path
}

fn step_15_env(
    bin_dir: &Path,
    capture_path: &Path,
    step_10_output: &str,
) -> HashMap<String, String> {
    let mut vars = HashMap::new();
    let existing_path = std::env::var("PATH").unwrap_or_default();
    vars.insert("STEP_10_OUTPUT".into(), step_10_output.into());
    vars.insert("PR_NUM".into(), "357".into());
    vars.insert("REPO".into(), "RyderFreeman4Logos/cli-sub-agent".into());
    vars.insert("BOT_UNAVAILABLE".into(), "false".into());
    vars.insert("BOT_HAS_ISSUES".into(), "true".into());
    vars.insert("COMMENT_IS_FALSE_POSITIVE".into(), "true".into());
    vars.insert("COMMENT_IS_STALE".into(), "false".into());
    vars.insert(
        "PATH".into(),
        format!("{}:{}", bin_dir.display(), existing_path),
    );
    vars.insert("TEST_GH_CAPTURE".into(), capture_path.display().to_string());
    vars
}

fn dismissed_debate_output() -> &'static str {
    r#"VERDICT: DISMISSED
RATIONALE: The bot misread the workflow and the fix path still runs for confirmed issues.
PR_COMMENT_START
**Local arbitration result: DISMISSED.**

## Participants
- **Author**: codex/openai/gpt-5/xhigh
- **Arbiter**: gemini-cli/google/default/xhigh

## Bot Concern
The bot warned that the workflow could skip real fixes after arbitration.

## Debate Summary
### Round 1
- **Proposer** (`codex/openai/gpt-5/xhigh`): The new reroute step preserves the fix path.
- **Critic** (`gemini-cli/google/default/xhigh`): The parser must fail closed on malformed markers.

## Conclusion
The finding is dismissed because the workflow now reroutes CONFIRMED verdicts back into the fix step and fails closed on malformed structured output.

CSA session ID: 01TESTDEBATESESSIONID
PR_COMMENT_END
"#
}

fn confirmed_debate_output() -> &'static str {
    r#"VERDICT: CONFIRMED
RATIONALE: The bot concern is valid and this comment must reroute to the fix step.
PR_COMMENT_START
Workflow should not post this text because the verdict is CONFIRMED.
PR_COMMENT_END
"#
}

#[tokio::test]
async fn execute_step_bash_posts_pr_audit_trail_for_dismissed_verdict() {
    let step = load_pr_bot_step_by_title("Step 8a: Post Debate Audit Trail Comment");
    let tmp = tempfile::tempdir().unwrap();
    let capture_path = install_fake_gh(tmp.path());
    let vars = step_15_env(tmp.path(), &capture_path, dismissed_debate_output());

    let result = execute_step(&step, &vars, tmp.path(), None, None).await;

    assert_eq!(
        result.exit_code, 0,
        "error={:?} output={:?}",
        result.error, result.output
    );
    assert!(
        result
            .output
            .as_deref()
            .unwrap_or("")
            .contains("CSA_VAR:AUDIT_TRAIL_POSTED=true")
    );

    let comment = std::fs::read_to_string(&capture_path).unwrap();
    assert!(comment.contains("## Participants"));
    assert!(comment.contains("## Bot Concern"));
    assert!(comment.contains("## Debate Summary"));
    assert!(comment.contains("## Conclusion"));
    assert!(comment.contains("CSA session ID: 01TESTDEBATESESSIONID"));
}

#[tokio::test]
async fn execute_step_bash_reroutes_confirmed_verdict_without_posting_comment() {
    let step = load_pr_bot_step_by_title("Step 8a: Post Debate Audit Trail Comment");
    let tmp = tempfile::tempdir().unwrap();
    let capture_path = install_fake_gh(tmp.path());
    let vars = step_15_env(tmp.path(), &capture_path, confirmed_debate_output());

    let result = execute_step(&step, &vars, tmp.path(), None, None).await;

    assert_eq!(
        result.exit_code, 0,
        "error={:?} output={:?}",
        result.error, result.output
    );
    assert!(
        result
            .output
            .as_deref()
            .unwrap_or("")
            .contains("CSA_VAR:AUDIT_TRAIL_POSTED=false")
    );
    assert!(
        result
            .output
            .as_deref()
            .unwrap_or("")
            .contains("CSA_VAR:COMMENT_IS_FALSE_POSITIVE=false")
    );
    assert!(
        !capture_path.exists(),
        "gh pr comment should not run for CONFIRMED verdicts"
    );
}

#[tokio::test]
async fn execute_step_bash_fails_closed_on_malformed_dismissed_output() {
    let step = load_pr_bot_step_by_title("Step 8a: Post Debate Audit Trail Comment");
    let tmp = tempfile::tempdir().unwrap();
    let capture_path = install_fake_gh(tmp.path());
    let malformed_output = r#"VERDICT: DISMISSED
RATIONALE: Missing comment end marker should abort.
PR_COMMENT_START
**Local arbitration result: DISMISSED.**

## Participants
- **Author**: codex/openai/gpt-5/xhigh
- **Arbiter**: gemini-cli/google/default/xhigh

## Bot Concern
Malformed marker contract.

## Debate Summary
### Round 1
- **Proposer** (`codex/openai/gpt-5/xhigh`): Missing end marker.
- **Critic** (`gemini-cli/google/default/xhigh`): The parser should abort.

## Conclusion
Abort rather than post an ambiguous comment.

CSA session ID: 01TESTDEBATESESSIONID
"#;
    let vars = step_15_env(tmp.path(), &capture_path, malformed_output);

    let result = execute_step(&step, &vars, tmp.path(), None, None).await;

    assert_ne!(result.exit_code, 0);
    assert!(
        !capture_path.exists(),
        "gh pr comment should not run for malformed debate output"
    );
}

#[tokio::test]
async fn execute_step_bash_fails_closed_on_duplicate_verdict_markers() {
    let step = load_pr_bot_step_by_title("Step 8a: Post Debate Audit Trail Comment");
    let tmp = tempfile::tempdir().unwrap();
    let capture_path = install_fake_gh(tmp.path());
    let duplicate_verdict_output = r#"VERDICT: DISMISSED
RATIONALE: The first verdict is stale.
VERDICT: CONFIRMED
RATIONALE: The final verdict conflicts with the first one.
PR_COMMENT_START
**Local arbitration result: DISMISSED.**

## Participants
- **Author**: codex/openai/gpt-5/xhigh
- **Arbiter**: gemini-cli/google/default/xhigh

## Bot Concern
Conflicting verdict markers must fail closed.

## Debate Summary
### Round 1
- **Proposer** (`codex/openai/gpt-5/xhigh`): Duplicate verdicts are ambiguous.
- **Critic** (`gemini-cli/google/default/xhigh`): The parser should reject them.

## Conclusion
Abort rather than posting an ambiguous dismissal.

CSA session ID: 01TESTDEBATESESSIONID
PR_COMMENT_END
"#;
    let vars = step_15_env(tmp.path(), &capture_path, duplicate_verdict_output);

    let result = execute_step(&step, &vars, tmp.path(), None, None).await;

    assert_ne!(result.exit_code, 0);
    assert!(
        !capture_path.exists(),
        "gh pr comment should not run for duplicate verdict markers"
    );
}

#[tokio::test]
async fn execute_plan_skips_false_condition_cleanly() {
    // A plan with condition-false steps must skip them with exit_code 0
    // and continue to subsequent steps.
    let plan = ExecutionPlan {
        name: "test".into(),
        description: String::new(),
        variables: vec![],
        steps: vec![
            PlanStep {
                id: 1,
                title: "conditional step".into(),
                tool: Some("bash".into()),
                prompt: "echo hello".into(),
                tier: None,
                depends_on: vec![],
                on_fail: FailAction::Skip,
                condition: Some("${FLAG}".into()),
                loop_var: None,
                session: None,
            },
            PlanStep {
                id: 2,
                title: "unconditional step".into(),
                tool: Some("bash".into()),
                prompt: "```bash\necho ok\n```".into(),
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
    let results = execute_plan(&plan, &vars, tmp.path(), None, None)
        .await
        .unwrap();
    // Both steps should be processed
    assert_eq!(results.len(), 2, "both steps must be processed");
    // Step 1: skipped with success (condition false)
    assert!(results[0].skipped);
    assert_eq!(
        results[0].exit_code, 0,
        "condition-false skip is not a failure"
    );
    // Step 2: executed successfully
    assert!(!results[1].skipped);
    assert_eq!(results[1].exit_code, 0);
}

#[tokio::test]
async fn execute_plan_runs_true_condition_steps() {
    // When condition is true, the step must execute (not skip).
    let plan = ExecutionPlan {
        name: "test".into(),
        description: String::new(),
        variables: vec![],
        steps: vec![PlanStep {
            id: 1,
            title: "conditional step".into(),
            tool: Some("bash".into()),
            prompt: "```bash\necho hello\n```".into(),
            tier: None,
            depends_on: vec![],
            on_fail: FailAction::Abort,
            condition: Some("${FLAG}".into()),
            loop_var: None,
            session: None,
        }],
    };
    let mut vars = HashMap::new();
    vars.insert("FLAG".into(), "yes".into());
    let tmp = tempfile::tempdir().unwrap();
    let results = execute_plan(&plan, &vars, tmp.path(), None, None)
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    assert!(!results[0].skipped, "true condition must execute");
    assert_eq!(results[0].exit_code, 0);
}

#[tokio::test]
async fn execute_plan_allows_prefixed_marker_to_drive_next_condition() {
    let plan = ExecutionPlan {
        name: "marker-flow".into(),
        description: String::new(),
        variables: vec![VariableDecl {
            name: "FLAG".into(),
            default: None,
        }],
        steps: vec![
            PlanStep {
                id: 1,
                title: "emit marker".into(),
                tool: Some("bash".into()),
                prompt: "```bash\necho 'CSA_VAR:FLAG=yes'\n```".into(),
                tier: None,
                depends_on: vec![],
                on_fail: FailAction::Abort,
                condition: None,
                loop_var: None,
                session: None,
            },
            PlanStep {
                id: 2,
                title: "conditioned step".into(),
                tool: Some("bash".into()),
                prompt: "```bash\necho marker_pass > marker.txt\n```".into(),
                tier: None,
                depends_on: vec![],
                on_fail: FailAction::Abort,
                condition: Some("${FLAG}".into()),
                loop_var: None,
                session: None,
            },
        ],
    };
    let vars = HashMap::new();
    let tmp = tempfile::tempdir().unwrap();
    let results = execute_plan(&plan, &vars, tmp.path(), None, None)
        .await
        .unwrap();

    assert_eq!(results.len(), 2);
    assert_eq!(results[0].exit_code, 0);
    assert_eq!(results[1].exit_code, 0);
    assert!(!results[1].skipped);
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("marker.txt"))
            .unwrap()
            .trim(),
        "marker_pass"
    );
}

#[tokio::test]
async fn execute_plan_does_not_inject_markers_from_failed_steps() {
    let plan = ExecutionPlan {
        name: "failed-marker".into(),
        description: String::new(),
        variables: vec![VariableDecl {
            name: "FLAG".into(),
            default: None,
        }],
        steps: vec![
            PlanStep {
                id: 1,
                title: "emit then fail".into(),
                tool: Some("bash".into()),
                prompt: "```bash\necho 'CSA_VAR:FLAG=yes'\nexit 1\n```".into(),
                tier: None,
                depends_on: vec![],
                on_fail: FailAction::Skip,
                condition: None,
                loop_var: None,
                session: None,
            },
            PlanStep {
                id: 2,
                title: "must stay skipped".into(),
                tool: Some("bash".into()),
                prompt: "```bash\necho should_not_run > should_not_exist.txt\n```".into(),
                tier: None,
                depends_on: vec![],
                on_fail: FailAction::Abort,
                condition: Some("${FLAG}".into()),
                loop_var: None,
                session: None,
            },
        ],
    };
    let vars = HashMap::new();
    let tmp = tempfile::tempdir().unwrap();
    let results = execute_plan(&plan, &vars, tmp.path(), None, None)
        .await
        .unwrap();

    assert_eq!(results.len(), 2);
    assert_ne!(results[0].exit_code, 0);
    assert!(results[1].skipped);
    assert!(!tmp.path().join("should_not_exist.txt").exists());
}
