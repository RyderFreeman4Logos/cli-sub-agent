use super::*;
use crate::cli::ReviewMode;

#[test]
fn test_build_review_instruction_contains_safety_preamble() {
    let result = build_review_instruction(
        "uncommitted",
        "review-only",
        "auto",
        ReviewMode::Standard,
        None,
    );
    assert!(result.contains("INSIDE a CSA subprocess"));
    assert!(
        result.contains("REVIEW-ONLY SAFETY")
            && result.contains("READ-ONLY analysis session")
            && result.contains("Do NOT modify, create, or delete any files")
    );
    assert!(
        !result.contains("Do NOT invoke"),
        "Legacy blanket anti-csa text must not be reintroduced (breaks fractal recursion contract)"
    );
}
