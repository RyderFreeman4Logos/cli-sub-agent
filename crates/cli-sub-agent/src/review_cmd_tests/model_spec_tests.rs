#[test]
fn review_rejects_unknown_codex_model_at_clap_parse() {
    let err = super::parse_review_error(&[
        "csa",
        "review",
        "--model-spec",
        "codex/openai/o3/xhigh",
        "--diff",
    ]);
    let msg = err.to_string();
    assert!(msg.contains("o3"), "missing offending model: {msg}");
    assert!(msg.contains("gpt-5.5"), "missing valid alternative: {msg}");
}

#[test]
fn review_accepts_valid_codex_model_at_clap_parse() {
    let args = super::parse_review_args(&[
        "csa",
        "review",
        "--model-spec",
        "codex/openai/gpt-5.5/xhigh",
        "--diff",
    ]);
    assert_eq!(
        args.model_spec.as_deref(),
        Some("codex/openai/gpt-5.5/xhigh")
    );
}
