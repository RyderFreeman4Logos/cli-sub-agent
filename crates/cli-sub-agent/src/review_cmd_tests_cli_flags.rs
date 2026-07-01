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

#[test]
fn review_cli_parses_chunked_review_mode_flag() {
    let args = parse_review_args(&["csa", "review", "--diff", "--chunked-review", "always"]);

    assert_eq!(args.chunked_review, crate::cli::ReviewChunkingMode::Always);
}

#[test]
fn review_cli_parses_depth_flag() {
    let default_args = parse_review_args(&["csa", "review", "--diff"]);
    assert_eq!(default_args.depth, crate::cli::ReviewDepth::Standard);

    let standard_args = parse_review_args(&["csa", "review", "--diff", "--depth", "standard"]);
    assert_eq!(standard_args.depth, crate::cli::ReviewDepth::Standard);

    let audit_args = parse_review_args(&["csa", "review", "--diff", "--depth", "audit"]);
    assert_eq!(audit_args.depth, crate::cli::ReviewDepth::Audit);
}

#[test]
fn review_cli_rejects_audit_depth_with_security_off() {
    let err = parse_or_validate_review_error(&[
        "csa",
        "review",
        "--diff",
        "--depth",
        "audit",
        "--security-mode",
        "off",
    ]);
    assert!(
        err.to_string()
            .contains("--depth audit conflicts with --security-mode off"),
        "{err}"
    );
}

#[test]
fn review_cli_parses_fix_finding_prompt_flag() {
    let args = parse_review_args(&[
        "csa",
        "review",
        "--fix-finding",
        "--session",
        "01HREVIEWSESSION0000000000",
        "--prompt",
        "fix the confirmed finding",
    ]);

    assert!(args.fix_finding);
    assert_eq!(args.session.as_deref(), Some("01HREVIEWSESSION0000000000"));
    assert_eq!(args.prompt.as_deref(), Some("fix the confirmed finding"));
}

#[test]
fn review_cli_rejects_fix_finding_without_session() {
    let err =
        parse_or_validate_review_error(&["csa", "review", "--fix-finding", "--prompt", "fix"]);
    assert!(
        err.to_string().contains("--fix-finding requires --session"),
        "{err}"
    );
}

#[test]
fn review_cli_rejects_prompt_without_fix_finding() {
    let err = parse_or_validate_review_error(&["csa", "review", "--diff", "--prompt", "fix"]);
    assert!(
        err.to_string()
            .contains("--prompt is only valid with --fix-finding"),
        "{err}"
    );
}
