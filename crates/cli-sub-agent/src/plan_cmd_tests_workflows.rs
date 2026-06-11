use std::path::{Path, PathBuf};
use weave::compiler::plan_from_toml;

use crate::plan_cmd::extract_bash_code_block;

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
fn dev2merge_forwards_impl_executor_overrides_to_mktd() {
    let workflow_path = workspace_root().join("patterns/dev2merge/workflow.toml");
    let workflow = std::fs::read_to_string(&workflow_path).unwrap();
    let plan = plan_from_toml(&workflow).unwrap();

    for name in ["IMPL_TIER", "IMPL_TOOL"] {
        let variable = plan
            .variables
            .iter()
            .find(|variable| variable.name == name)
            .unwrap_or_else(|| panic!("dev2merge must declare {name}"));
        assert_eq!(
            variable.default.as_deref(),
            Some(""),
            "{name} must default to empty so callers keep existing behavior unless they opt in"
        );
    }

    let plan_step = plan
        .steps
        .iter()
        .find(|step| step.title == "Plan with mktd")
        .expect("missing dev2merge plan step");
    for required in [
        r#"--var IMPL_TIER="${IMPL_TIER:-}""#,
        r#"--var IMPL_TOOL="${IMPL_TOOL:-}""#,
    ] {
        assert!(
            plan_step.prompt.contains(required),
            "Step 7 must forward implementation override to mktd: {required}"
        );
    }

    let mktsk_step = plan
        .steps
        .iter()
        .find(|step| step.title == "Execute Plan with mktsk")
        .expect("missing dev2merge mktsk step");
    for required in [
        "impl `${IMPL_TIER}`/`${IMPL_TOOL}`",
        "`[CSA:<value>]` impl tasks use `csa run`",
        "tier-*→`--tier`",
        "other→`--tool`",
        "Implementation override: csa run ...",
    ] {
        assert!(
            mktsk_step.prompt.contains(required),
            "Step 8 must tell mktsk how to honor implementation override tags: {required}"
        );
    }
}

#[test]
fn mktsk_consumes_impl_executor_tier_and_override_contract() {
    let workflow_path = workspace_root().join("patterns/mktsk/workflow.toml");
    let workflow = std::fs::read_to_string(&workflow_path).unwrap();
    let plan = plan_from_toml(&workflow).unwrap();
    let execute_step = plan
        .steps
        .iter()
        .find(|step| step.title == "Execute Checklist Serially")
        .expect("missing mktsk execution step");
    let pattern = std::fs::read_to_string(workspace_root().join("patterns/mktsk/PATTERN.md"))
        .expect("read mktsk pattern");

    for required in [
        "optional Implementation override directive",
        "Priority: Implementation override > [CSA:tier-*] > [CSA:<tool>] > default",
        "Implementation override: csa run ... => run that exact csa run command; it wins over any executor tag",
        "[CSA:tier-*] => csa run --tier <tier-name>",
        "[CSA:<tool>] => csa run --tool <tool-name>",
    ] {
        assert!(
            workflow.contains(required),
            "mktsk workflow must consume implementation override dispatch contract: {required}"
        );
    }

    for required in [
        "optional `Implementation override: csa run ...` directive",
        "Priority: `Implementation override:` > `[CSA:tier-*]` > `[CSA:<tool>]` > default.",
        "`[CSA:tier-*]`: run `csa run --tier <tier-name>`",
        "`[CSA:<tool>]`: run `csa run --tool <tool-name>`",
        "`Implementation override: csa run --tier tier-4-critical --tool claude-code` dispatches `csa run --tier tier-4-critical --tool claude-code`.",
    ] {
        assert!(
            pattern.contains(required),
            "mktsk PATTERN.md must document implementation override dispatch contract: {required}"
        );
    }

    assert!(
        execute_step
            .prompt
            .contains("[CSA:tier-4-critical] => csa run --tier tier-4-critical"),
        "`[CSA:tier-4-critical]` tasks must dispatch as `csa run --tier tier-4-critical`"
    );
    assert!(
        execute_step.prompt.contains(
            "Implementation override: csa run --tier tier-4-critical --tool claude-code => run csa run --tier tier-4-critical --tool claude-code"
        ),
        "`Implementation override: csa run --tier tier-4-critical --tool claude-code` must dispatch that exact command"
    );
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
fn mktd_generates_impl_executor_tags_from_overrides() {
    let workflow_path = workspace_root().join("patterns/mktd/workflow.toml");
    let workflow = std::fs::read_to_string(&workflow_path).unwrap();
    let plan = plan_from_toml(&workflow).unwrap();

    for name in ["IMPL_TIER", "IMPL_TOOL"] {
        let variable = plan
            .variables
            .iter()
            .find(|variable| variable.name == name)
            .unwrap_or_else(|| panic!("mktd must declare {name}"));
        assert_eq!(
            variable.default.as_deref(),
            Some(""),
            "{name} must default to empty so non-dev2merge callers keep existing executor tags"
        );
    }
    for name in ["IMPL_EXECUTOR_TAG", "IMPL_EXECUTOR_DIRECTIVE"] {
        assert!(
            plan.variables.iter().any(|variable| variable.name == name),
            "mktd must declare generated variable {name}"
        );
    }

    let language_step = plan
        .steps
        .iter()
        .find(|step| step.title == "Phase 1.5 — Language Detection")
        .expect("missing mktd language step");
    for required in [
        "[CSA:${IMPL_TIER}]",
        "[CSA:${IMPL_TOOL}]",
        "[Sub:developer]",
        "CSA_VAR:IMPL_EXECUTOR_TAG=${IMPL_EXECUTOR_TAG}",
        "Implementation override: csa run",
        "--tier ${IMPL_TIER}",
        "--tool ${IMPL_TOOL}",
    ] {
        assert!(
            language_step.prompt.contains(required),
            "mktd Step 1.5 must compute implementation executor overrides: {required}"
        );
    }

    let draft_step = plan
        .steps
        .iter()
        .find(|step| step.title == "Phase 2 — DRAFT TODO")
        .expect("missing mktd draft step");
    for required in [
        "Pre-assign: [Main], ${IMPL_EXECUTOR_TAG}, [Skill:commit], [CSA:tool].",
        "Use `${IMPL_EXECUTOR_TAG}` for implementation tasks;",
        "include that line in each such task",
    ] {
        assert!(
            draft_step.prompt.contains(required),
            "mktd draft prompt must require override-aware implementation task tags: {required}"
        );
    }

    let pattern = std::fs::read_to_string(workspace_root().join("patterns/mktd/PATTERN.md"))
        .expect("read mktd pattern");
    for required in [
        "IMPL_TIER",
        "IMPL_TOOL",
        "IMPL_EXECUTOR_TAG",
        "IMPL_EXECUTOR_DIRECTIVE",
        "Use `${IMPL_EXECUTOR_TAG}` for implementation tasks;",
    ] {
        assert!(
            pattern.contains(required),
            "PATTERN.md must stay synced with mktd implementation override workflow contract: {required}"
        );
    }
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
    let extracted_save_script =
        extract_bash_code_block(&save_step.prompt).expect("mktd save step must have bash block");
    let pattern = std::fs::read_to_string(workspace_root().join("patterns/mktd/PATTERN.md"))
        .expect("read mktd pattern");

    assert!(
        extracted_save_script.contains(r#"csa todo persist -t "${TODO_TS}""#),
        "mktd Save TODO bash extraction must not stop at markdown fence literals in sed expressions"
    );

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

// #1843 ─────────────────────────────────────────────────────────────────────
//
// Step 7 (`Plan with mktd`) must validate DONE WHEN *per task*, not via a single
// global mention. This is the 4th sibling of the #1822 per-task conversion (the
// other three: csa-todo `validate_generated_plan_content`, mktd `workflow.toml`,
// mktd `PATTERN.md`). A global `grep -q 'DONE WHEN'` passes a multi-task plan
// where only one open task carries a clause; the per-task awk rejects it.
#[test]
fn dev2merge_plan_step_done_when_gate_is_per_task() {
    let workflow_path = workspace_root().join("patterns/dev2merge/workflow.toml");
    let workflow = std::fs::read_to_string(&workflow_path).unwrap();
    let plan = plan_from_toml(&workflow).unwrap();
    let prompt = &plan
        .steps
        .iter()
        .find(|step| step.title == "Plan with mktd")
        .expect("missing dev2merge plan step")
        .prompt;

    assert!(
        !prompt.contains("grep -q 'DONE WHEN'"),
        "Step 7 must NOT use the weak global `grep -q 'DONE WHEN'` check (#1843)"
    );
    assert!(
        prompt.contains("index(text, \"DONE WHEN:\")") && prompt.contains("is_open"),
        "Step 7 must use the per-task awk DONE WHEN gate (#1843)"
    );
}

// #1843 behavioral check: the extracted awk gate must reject any open task that
// lacks its own `DONE WHEN:` clause — including the empty `- [ ] ` placeholder
// `csa todo create` leaves on the branch when mktd fails before persist — and
// accept a plan where every open task carries one. Unix-only: relies on `awk`.
#[cfg(unix)]
#[test]
fn dev2merge_done_when_awk_gate_enforces_per_task() {
    let workflow_path = workspace_root().join("patterns/dev2merge/workflow.toml");
    let workflow = std::fs::read_to_string(&workflow_path).unwrap();
    let plan = plan_from_toml(&workflow).unwrap();
    let prompt = plan
        .steps
        .iter()
        .find(|step| step.title == "Plan with mktd")
        .expect("missing dev2merge plan step")
        .prompt
        .clone();
    // Extract the exact awk program bash hands to `awk` (the bytes between
    // `awk '` and `' "${TODO_PATH}"`), so this exercises the real gate.
    let awk_program = prompt
        .split("awk '")
        .nth(1)
        .and_then(|rest| rest.split("' \"${TODO_PATH}\"").next())
        .expect("Step 7 must invoke `awk '<program>' \"${TODO_PATH}\"`");

    let run = |todo: &str| -> i32 {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let mut child = Command::new("awk")
            .arg(awk_program)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("awk must be available on unix test hosts");
        child
            .stdin
            .take()
            .expect("awk stdin")
            .write_all(todo.as_bytes())
            .expect("write todo to awk stdin");
        child.wait().expect("await awk").code().unwrap_or(1)
    };

    // Accept: every open task carries its own clause (following-line and same-line).
    assert_eq!(
        run("# Plan\n- [ ] a\n  DONE WHEN: tests pass\n- [ ] b DONE WHEN: lint clean\n"),
        0,
        "fully per-task plan must pass"
    );
    // Reject: a single open task with no clause.
    assert_ne!(
        run("# Plan\n- [ ] do thing\n"),
        0,
        "open task missing DONE WHEN must fail"
    );
    // Reject: multi-task plan where only one task carries a clause — the per-task
    // gain a global `grep -q 'DONE WHEN'` would miss.
    assert_ne!(
        run("# Plan\n- [ ] a\n  DONE WHEN: x\n- [ ] b\n"),
        0,
        "partial per-task coverage must fail"
    );
    // Reject: a bare keyword mention in a subject is not a clause (#1822 case).
    assert_ne!(
        run("# Plan\n- [ ] Document DONE WHEN policy.\n"),
        0,
        "bare `DONE WHEN` subject mention without a colon clause must fail"
    );
    // Reject: a clause on a completed task must not cover a sibling open task.
    assert_ne!(
        run("# Plan\n- [x] done\n  DONE WHEN: x\n- [ ] open\n"),
        0,
        "completed-task clause must not satisfy an open task"
    );
    // Reject: the empty `- [ ] ` placeholder `csa todo create` writes — preserves
    // the rejection the prior global check gave when mktd fails before persist.
    assert_ne!(
        run("# TODO: feature\n\n## Tasks\n\n- [ ] \n"),
        0,
        "empty csa-todo-create scaffold must fail (no regression vs global check)"
    );
}
