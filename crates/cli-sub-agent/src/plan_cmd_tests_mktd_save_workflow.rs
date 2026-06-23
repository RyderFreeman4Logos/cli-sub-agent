use std::path::{Path, PathBuf};
use weave::compiler::plan_from_toml;

use crate::plan_cmd::extract_bash_code_block;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
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
        extracted_save_script.contains(r#""${CSA_BIN}" todo persist -t "${TODO_TS}""#),
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
            r#"RAW_SPEC_ARTIFACT="${SAVE_DIR}/spec.raw.txt""#,
            r#"FENCE=$(printf '\140\140\140')"#,
            r#"awk -v s="${FENCE}epic-plan.toml" -v e="${FENCE}" '$0 == s"#,
            r#"CSA_BIN="${CSA_BIN:-csa}""#,
            r#"extract_spec_toml() {"#,
            r#"perl -0CSDA -we"#,
            r#"expected raw TOML or fenced TOML"#,
            r#"Raw spec artifact path: %s"#,
            r#"read raw spec artifact failed"#,
            r#"k="non-TOML""#,
            r#"k="CSA section marker""#,
            r#""${FENCE}"*) k="Markdown code fence" ;;"#,
            r#"spec artifact-shape error: expected raw TOML or fenced TOML"#,
            r#"first content: %s"#,
            r#"Spec artifact path: %s"#,
            r#"grep -qE '^kind = "(scenario|property|check)"$' "${SPEC_ARTIFACT}""#,
            r#"perl -CSDA -ne '$found ||= /\p{Han}/; END { exit($found ? 0 : 1) }'"#,
            r#""${CSA_BIN}" todo persist -t "${TODO_TS}""#,
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
            r#"Artifact preview:"#,
            r#"sed -n '1,8p' "${SPEC_ARTIFACT}""#,
            r#"sed -n '/^```epic-plan.toml$/,/^```$/p'"#,
            r#"'```'*) SPEC_MARKER_KIND="Markdown code fence" ;;"#,
            r#"LOWER_SPEC_LINE="${FIRST_SPEC_LINE,,}""#,
            r#"rg -q '^kind = "(scenario|property|check)"$' "${SPEC_ARTIFACT}""#,
            r#"printf '%s\n' "${SUMMARY_LINE}" | rg -q '[\p{Han}]'"#,
            r#"HAN_COUNT=$(rg -o '[\p{Han}]' "${TODO_ARTIFACT}" | wc -l | tr -d '[:space:]')"#,
            r#"CJK_COUNT=$(rg -o '[\p{Han}\p{Hiragana}\p{Katakana}]' "${TODO_ARTIFACT}" | wc -l | tr -d '[:space:]')"#,
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
            .find(r#""${CSA_BIN}" todo persist -t "${TODO_TS}""#)
            .unwrap_or_else(|| panic!("{name} missing csa todo persist"));
        let validate_idx = content
            .find(r#"grep -qE '^- \[ \] .+' "${TODO_ARTIFACT}""#)
            .unwrap_or_else(|| panic!("{name} must validate the TODO artifact before persist"));
        let shape_idx = content
            .find(r#"if ! extract_spec_toml "${RAW_SPEC_ARTIFACT}" > "${SPEC_ARTIFACT}"; then"#)
            .unwrap_or_else(|| panic!("{name} must shape-check spec artifact before persist"));
        assert!(
            validate_idx < persist_idx,
            "{name} must validate artifacts BEFORE csa todo persist (commit), not after"
        );
        assert!(
            shape_idx < persist_idx,
            "{name} must reject markdown/HTML-shaped spec artifacts BEFORE csa todo persist"
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
