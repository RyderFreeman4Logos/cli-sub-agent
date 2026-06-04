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

#[test]
fn mktd_light_mode_skips_recon_dimensions() {
    let workflow_path = workspace_root().join("patterns/mktd/workflow.toml");
    let workflow = std::fs::read_to_string(&workflow_path).unwrap();
    let plan = plan_from_toml(&workflow).unwrap();

    for step_id in 3..=6 {
        let step = plan
            .steps
            .iter()
            .find(|step| step.id == step_id)
            .unwrap_or_else(|| panic!("missing mktd RECON step {step_id}"));
        assert_eq!(
            step.condition.as_deref(),
            Some("!(${INTENSITY_IS_LIGHT})"),
            "mktd light mode must skip RECON step {step_id}"
        );
    }

    let pattern = std::fs::read_to_string(workspace_root().join("patterns/mktd/PATTERN.md"))
        .expect("read mktd pattern");
    assert_eq!(
        pattern
            .matches("Condition: !(${INTENSITY_IS_LIGHT})")
            .count(),
        4,
        "PATTERN.md must stay synced with workflow RECON conditions"
    );
}

#[test]
fn mktd_save_step_uses_session_output_artifacts_and_persist() {
    let workflow_path = workspace_root().join("patterns/mktd/workflow.toml");
    let workflow = std::fs::read_to_string(&workflow_path).unwrap();
    let plan = plan_from_toml(&workflow).unwrap();
    let save_step = plan
        .steps
        .iter()
        .find(|step| step.id == 13)
        .expect("missing mktd save step");
    let pattern = std::fs::read_to_string(workspace_root().join("patterns/mktd/PATTERN.md"))
        .expect("read mktd pattern");

    for (name, content) in [
        ("PATTERN.md", pattern.as_str()),
        ("workflow.toml Step 13", save_step.prompt.as_str()),
    ] {
        for required in [
            r#"SAVE_DIR="${CSA_SESSION_DIR:?CSA_SESSION_DIR must be set}/output/mktd-save""#,
            r#"TODO_ARTIFACT="${SAVE_DIR}/TODO.md""#,
            r#"SPEC_ARTIFACT="${SAVE_DIR}/spec.toml""#,
            r#"csa todo persist -t "${TODO_TS}""#,
            r#"--todo-file "${TODO_ARTIFACT}""#,
            r#"--spec-file "${SPEC_ARTIFACT}""#,
        ] {
            assert!(
                content.contains(required),
                "{name} must route mktd save artifacts through session output and todo persist: missing {required}"
            );
        }

        for forbidden in [
            r#"> "${TODO_PATH}""#,
            r#"> "${SPEC_PATH}""#,
            r#"> "${EPIC_PATH}""#,
            "csa todo save -t",
        ] {
            assert!(
                !content.contains(forbidden),
                "{name} must not write generated artifacts directly into todo state before persist: found {forbidden}"
            );
        }

        // Round-5 hard-gate ordering (#1820/#1822): the artifact validation MUST
        // run BEFORE `csa todo persist` commits, so an invalid plan can never
        // enter the todos git history even if a later step aborts.
        let persist_idx = content
            .find(r#"csa todo persist -t "${TODO_TS}""#)
            .unwrap_or_else(|| panic!("{name} missing csa todo persist"));
        let validate_idx = content
            .find(r#"grep -qE '^- \[ \] .+' "${TODO_ARTIFACT}""#)
            .unwrap_or_else(|| panic!("{name} must validate the TODO artifact before persist"));
        assert!(
            validate_idx < persist_idx,
            "{name} must validate artifacts BEFORE csa todo persist (commit), not after"
        );

        for forbidden_postcommit in [
            // Post-commit content validation is forbidden: it cannot gate the
            // commit. These checks moved before persist (artifacts) or into
            // `csa todo persist` itself (spec-criteria render gate).
            r#"grep -qE '^- \[ \] .+' "${TODO_PATH}""#,
            r#"csa todo show -t "${TODO_TS}" --spec"#,
            "saved TODO has no non-empty checkbox tasks",
        ] {
            assert!(
                !content.contains(forbidden_postcommit),
                "{name} must not validate the persisted plan AFTER the commit: found {forbidden_postcommit}"
            );
        }
    }
}

#[test]
fn pr_bot_debate_audit_reads_step12_output_and_prompt_has_comment_context() {
    let workflow_path = workspace_root().join("patterns/pr-bot/workflow.toml");
    let workflow = std::fs::read_to_string(&workflow_path).unwrap();
    let plan = plan_from_toml(&workflow).unwrap();

    let step12 = plan
        .steps
        .iter()
        .find(|step| step.title == "Arbitrate via Debate")
        .expect("missing pr-bot debate step");
    let step14 = plan
        .steps
        .iter()
        .find(|step| step.title == "Step 8a: Post Debate Audit Trail Comment")
        .expect("missing pr-bot audit trail step");

    for required in [
        "CURRENT_COMMENT_ID=${CURRENT_COMMENT_ID}",
        "COMMENT_PATH=${COMMENT_PATH}",
        "COMMENT_TIMESTAMP=${COMMENT_TIMESTAMP}",
        "gh api repos/${REPO}/pulls/comments/${CURRENT_COMMENT_ID}",
        "VERDICT: DISMISSED|CONFIRMED",
        "PR_COMMENT_START",
        "PR_COMMENT_END",
    ] {
        assert!(
            step12.prompt.contains(required),
            "pr-bot debate step must pass required arbitration context or output contract: {required}"
        );
    }

    assert!(
        step14
            .prompt
            .contains(r#"DEBATE_OUTPUT="${STEP_12_OUTPUT}""#),
        "audit trail step must parse the actual debate step output"
    );
    assert!(
        !step14
            .prompt
            .contains(r#"DEBATE_OUTPUT="${STEP_11_OUTPUT}""#),
        "audit trail step must not parse the staleness-filter output"
    );

    let pattern = std::fs::read_to_string(workspace_root().join("patterns/pr-bot/PATTERN.md"))
        .expect("read pr-bot pattern");
    for required in [
        "CURRENT_COMMENT_ID=${CURRENT_COMMENT_ID}",
        "COMMENT_PATH=${COMMENT_PATH}",
        "COMMENT_TIMESTAMP=${COMMENT_TIMESTAMP}",
        "gh api repos/${REPO}/pulls/comments/${CURRENT_COMMENT_ID}",
        "DEBATE_OUTPUT=\"${STEP_12_OUTPUT}\"",
    ] {
        assert!(
            pattern.contains(required),
            "PATTERN.md must stay synced with debate audit workflow contract: {required}"
        );
    }
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
