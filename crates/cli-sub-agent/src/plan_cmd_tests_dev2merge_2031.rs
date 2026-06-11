use std::path::{Path, PathBuf};
use weave::compiler::plan_from_toml;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn markdown_step_section<'a>(content: &'a str, heading: &str) -> &'a str {
    let start = content
        .find(heading)
        .unwrap_or_else(|| panic!("missing markdown step heading: {heading}"));
    let rest = &content[start..];
    let end = rest[1..]
        .find("\n## Step ")
        .map(|offset| offset + 1)
        .unwrap_or(rest.len());
    &rest[..end]
}

fn first_bash_block(content: &str) -> &str {
    let rest = content
        .split_once("```bash")
        .map(|(_, rest)| rest)
        .expect("missing bash block");
    rest.split_once("```")
        .map(|(block, _)| block.trim())
        .expect("missing bash block terminator")
}

#[test]
fn dev2merge_already_resolved_check_does_not_abort_without_merged_pr() {
    let root = workspace_root();
    let workflow = std::fs::read_to_string(root.join("patterns/dev2merge/workflow.toml")).unwrap();
    let pattern = std::fs::read_to_string(root.join("patterns/dev2merge/PATTERN.md")).unwrap();
    let plan = plan_from_toml(&workflow).unwrap();
    let already_resolved_step = plan
        .steps
        .iter()
        .find(|step| step.title == "Already-Resolved Check")
        .expect("missing dev2merge already-resolved step");
    let workflow_step_0 = first_bash_block(&already_resolved_step.prompt);
    let pattern_step_0 = first_bash_block(markdown_step_section(
        &pattern,
        "## Step 0: Already-Resolved Check",
    ));

    assert_eq!(
        pattern_step_0, workflow_step_0,
        "dev2merge PATTERN.md and workflow.toml Step 0 bash blocks must stay synced"
    );

    for content in [workflow_step_0, pattern_step_0] {
        assert!(
            !content.contains(r#"[ -n "${MERGED_PR}" ] && skip"#),
            "Step 0 must not use a set -e-sensitive short-circuit skip"
        );
        assert!(
            content.contains(r#"if [ -n "${MERGED_PR}" ]; then"#),
            "Step 0 must explicitly branch on non-empty MERGED_PR"
        );
        assert!(
            content.contains(
                r#"skip "dev2merge: branch ${BRANCH} already merged via PR #${MERGED_PR}; HEAD is ancestor of ${DEFAULT_BRANCH} — nothing to do""#,
            ),
            "merged-PR skip message must keep the documented nothing-to-do suffix"
        );
    }
}
