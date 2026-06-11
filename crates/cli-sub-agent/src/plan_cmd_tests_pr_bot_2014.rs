use std::path::{Path, PathBuf};

const MERGE_COMMAND: &str = r#"gh pr merge "${MERGED_PR_VERIFY_REF}" --repo "${REPO}" --"${MERGE_STRATEGY}" ${DELETE_BRANCH_FLAG} ${CSA_REAL_GH:+--force-skip-pr-bot}"#;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn artifact_text(path: &str) -> String {
    std::fs::read_to_string(workspace_root().join(path)).unwrap()
}

fn workflow_step(text: &str, step_id: usize) -> String {
    let marker = format!("id = {step_id}");
    let start = text.find(&marker).expect("workflow step must exist");
    let body = &text[start..];
    let end = body.find("\n[[workflow.steps]]").unwrap_or(body.len());
    body[..end].to_string()
}

fn pattern_section(text: &str, heading: &str) -> String {
    let start = text.find(heading).expect("PATTERN.md heading must exist");
    let body = &text[start + heading.len()..];
    let end = body.find("\n## ").unwrap_or(body.len());
    text[start..start + heading.len() + end].to_string()
}

fn assert_order(text: &str, first: &str, second: &str, label: &str) {
    let first_idx = text
        .find(first)
        .unwrap_or_else(|| panic!("{label} missing {first}"));
    let second_idx = text
        .find(second)
        .unwrap_or_else(|| panic!("{label} missing {second}"));
    assert!(first_idx < second_idx, "{label} must guard {second}");
}

#[test]
fn pr_bot_final_merge_fails_closed_on_gh_merge_error() {
    for path in [
        "patterns/pr-bot/workflow.toml",
        "patterns/pr-bot/PATTERN.md",
    ] {
        let text = artifact_text(path);
        assert_eq!(
            text.matches(MERGE_COMMAND).count(),
            2,
            "{path} must guard both final merge commands"
        );
        assert!(
            !text.contains("${DELETE_BRANCH_FLAG} --force-skip-pr-bot"),
            "{path} must not pass the CSA-only bypass flag unconditionally"
        );

        let sections = if path.ends_with("workflow.toml") {
            vec![workflow_step(&text, 22), workflow_step(&text, 23)]
        } else {
            vec![
                pattern_section(&text, "## Step 12: Final Merge"),
                pattern_section(&text, "## Step 12b: Final Merge (Direct or Post-Rebase)"),
            ]
        };
        for section in sections {
            assert_order(&section, "set -e", MERGE_COMMAND, path);
            assert_order(&section, MERGE_COMMAND, "MERGE_COMPLETED=true", path);
        }
    }
}
