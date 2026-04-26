use std::path::{Path, PathBuf};
use weave::compiler::plan_from_toml;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
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
    assert_eq!(setup_abort.tool.as_deref(), Some("await-user"));

    let fork_convention = plan
        .steps
        .iter()
        .find(|step| step.title == "Assumed Fork Convention")
        .expect("missing fork convention advisory step");
    assert_eq!(fork_convention.tool.as_deref(), Some("note"));

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

// #1118 part A ────────────────────────────────────────────────────────────────
//
// Step 7 (`Plan with mktd`) must wrap the inner `csa plan run` invocation
// with a hard wall-clock timeout so a runaway debate-loop cannot burn 26+ min
// of codex tokens before the orchestrator notices.
#[test]
fn dev2merge_plan_step_has_mktd_timeout_seconds_variable() {
    let workflow_path = workspace_root().join("patterns/dev2merge/workflow.toml");
    let workflow = std::fs::read_to_string(&workflow_path).unwrap();
    let plan = plan_from_toml(&workflow).unwrap();

    assert!(
        plan.variables
            .iter()
            .any(|v| v.name == "MKTD_TIMEOUT_SECONDS"),
        "MKTD_TIMEOUT_SECONDS variable must be declared on the dev2merge workflow"
    );

    let plan_step = plan
        .steps
        .iter()
        .find(|step| step.title == "Plan with mktd")
        .expect("missing dev2merge plan step");
    let prompt = &plan_step.prompt;
    assert!(
        prompt.contains("MKTD_TIMEOUT_SECONDS"),
        "Step 7 prompt must reference MKTD_TIMEOUT_SECONDS for hard-cap on mktd wall-clock"
    );
    assert!(
        prompt.contains("timeout") && prompt.contains("csa plan run"),
        "Step 7 prompt must wrap `csa plan run` with the shell `timeout` command"
    );
    assert!(
        prompt.contains("MKTD_TIMEOUT_SECONDS:-1800"),
        "MKTD_TIMEOUT_SECONDS must default to 1800 seconds (aligned with execution.min_timeout_seconds per #1137)"
    );
    assert!(
        prompt.contains("124") && prompt.contains("137"),
        "Step 7 prompt must surface SIGTERM (124) and SIGKILL (137) timeout exits as hard failure"
    );
}

// #1118 part B ────────────────────────────────────────────────────────────────
//
// When the user-provided FEATURE_INPUT already names concrete file:line
// targets (>= 2 hits), Step 7 must default MKTD_INTENSITY to `light` to
// avoid spawning a debate-loop the user did not need.
#[test]
fn dev2merge_plan_step_has_brief_specificity_heuristic() {
    let workflow_path = workspace_root().join("patterns/dev2merge/workflow.toml");
    let workflow = std::fs::read_to_string(&workflow_path).unwrap();
    let plan = plan_from_toml(&workflow).unwrap();

    let plan_step = plan
        .steps
        .iter()
        .find(|step| step.title == "Plan with mktd")
        .expect("missing dev2merge plan step");
    let prompt = &plan_step.prompt;

    assert!(
        prompt.contains("FEATURE_INPUT_LEN") && prompt.contains("FEATURE_FILE_LINE_HITS"),
        "Step 7 prompt must compute FEATURE_INPUT_LEN and FEATURE_FILE_LINE_HITS for the brief-specificity heuristic"
    );
    assert!(
        prompt.contains("4096"),
        "brief-specificity heuristic must apply only to short briefs (< 4096 chars)"
    );
    assert!(
        prompt.contains("FEATURE_FILE_LINE_HITS"),
        "heuristic must check the file:line hit count"
    );
    assert!(
        prompt.contains("(rs|toml|md)"),
        "file:line regex must match Rust, TOML, and Markdown paths"
    );
}
