use super::*;
use tempfile::tempdir;

#[test]
fn build_review_instruction_for_project_injects_review_checklist() {
    let project_dir = tempdir().unwrap();
    let csa_dir = project_dir.path().join(".csa");
    std::fs::create_dir_all(&csa_dir).unwrap();
    std::fs::write(
        csa_dir.join("review-checklist.md"),
        "# Checklist\n- [ ] Verify error paths\n",
    )
    .unwrap();

    let (instruction, _routing) = build_review_instruction_for_project(
        "uncommitted",
        "review-only",
        "auto",
        ReviewMode::Standard,
        None,
        project_dir.path(),
        resolve::ReviewProjectPromptOptions {
            project_config: None,
            prior_rounds_section: None,
        },
    );

    assert!(
        instruction.contains("<review-checklist>"),
        "instruction should contain review-checklist open tag"
    );
    assert!(
        instruction.contains("</review-checklist>"),
        "instruction should contain review-checklist close tag"
    );
    assert!(instruction.contains("Verify error paths"));
}

#[test]
fn build_review_instruction_for_project_omits_checklist_when_missing() {
    let project_dir = tempdir().unwrap();

    let (instruction, _routing) = build_review_instruction_for_project(
        "uncommitted",
        "review-only",
        "auto",
        ReviewMode::Standard,
        None,
        project_dir.path(),
        resolve::ReviewProjectPromptOptions {
            project_config: None,
            prior_rounds_section: None,
        },
    );

    assert!(
        !instruction.contains("<review-checklist>"),
        "instruction should not contain review-checklist tag when file is missing"
    );
}
