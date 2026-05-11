use super::*;

#[test]
fn review_cli_parses_hint_difficulty_flag() {
    let args = parse_review_args(&[
        "csa",
        "review",
        "--tool",
        "claude-code",
        "--hint-difficulty",
        "code_review",
        "--diff",
    ]);
    assert_eq!(args.hint_difficulty.as_deref(), Some("code_review"));
}

#[test]
fn review_cli_parses_fast_but_more_cost_flag() {
    let args = parse_review_args(&["csa", "review", "--diff", "--fast-but-more-cost"]);

    assert!(args.fast_but_more_cost);
}
