#[test]
fn parse_token_usage_all_fields() {
    let output = "input_tokens: 1000\noutput_tokens: 500\ntotal_tokens: 1500\ncost: $0.05";
    let usage = super::parse_token_usage(output).unwrap();
    assert_eq!(usage.input_tokens, Some(1000));
    assert_eq!(usage.output_tokens, Some(500));
    assert_eq!(usage.total_tokens, Some(1500));
    assert!((usage.estimated_cost_usd.unwrap() - 0.05).abs() < f64::EPSILON);
}

#[test]
fn parse_token_usage_input_output_sums_to_total() {
    // When only input_tokens and output_tokens are present (no explicit total),
    // total_tokens should be their sum. The generic "tokens:" pattern must NOT
    // match "output_tokens:" or "input_tokens:".
    let output = "input_tokens: 200\noutput_tokens: 300";
    let usage = super::parse_token_usage(output).unwrap();
    assert_eq!(usage.input_tokens, Some(200));
    assert_eq!(usage.output_tokens, Some(300));
    assert_eq!(usage.total_tokens, Some(500));
}

#[test]
fn parse_token_usage_explicit_total_preferred() {
    let output = "total_tokens: 1500";
    let usage = super::parse_token_usage(output).unwrap();
    assert_eq!(usage.total_tokens, Some(1500));
}

#[test]
fn parse_token_usage_generic_tokens_field() {
    let output = "Tokens: 5000";
    let usage = super::parse_token_usage(output).unwrap();
    assert_eq!(usage.total_tokens, Some(5000));
}

#[test]
fn parse_token_usage_no_match_returns_none() {
    let output = "Hello world, no token info here.";
    assert!(super::parse_token_usage(output).is_none());
}

#[test]
fn parse_token_usage_empty_string_returns_none() {
    assert!(super::parse_token_usage("").is_none());
}

#[test]
fn parse_token_usage_with_cache_read_input_tokens() {
    // cache_read_input_tokens MUST be captured AND MUST NOT shadow input_tokens.
    let output = "input_tokens: 1000\ncache_read_input_tokens: 750\noutput_tokens: 500";
    let usage = super::parse_token_usage(output).unwrap();
    assert_eq!(usage.input_tokens, Some(1000));
    assert_eq!(usage.cache_read_input_tokens, Some(750));
    assert_eq!(usage.output_tokens, Some(500));
}

#[test]
fn parse_token_usage_with_codex_cached_input_tokens_jsonl() {
    let output = r#"{"type":"turn.completed","usage":{"input_tokens":1000,"cached_input_tokens":750,"output_tokens":500}}"#;

    let usage = super::parse_token_usage(output).unwrap();

    assert_eq!(usage.input_tokens, Some(1000));
    assert_eq!(usage.cache_read_input_tokens, Some(750));
    assert_eq!(usage.output_tokens, Some(500));
    assert_eq!(usage.total_tokens, Some(1500));
    assert_eq!(usage.uncached_input_tokens(), Some(250));
}

#[test]
fn parse_token_usage_with_reasoning_output_tokens_jsonl() {
    let output = r#"{"type":"turn.completed","usage":{"input_tokens":1000,"cached_input_tokens":700,"output_tokens":400,"reasoning_output_tokens":125}}"#;

    let usage = super::parse_token_usage(output).unwrap();

    assert_eq!(usage.reasoning_output_tokens, Some(125));
    assert_eq!(usage.output_tokens, Some(400));
}

#[test]
fn parse_token_usage_with_nested_reasoning_output_tokens_jsonl() {
    let output = r#"{"usage":{"prompt_tokens":1000,"completion_tokens":400,"prompt_tokens_details":{"cached_tokens":600},"completion_tokens_details":{"reasoning_tokens":125}}}"#;

    let usage = super::parse_token_usage(output).unwrap();

    assert_eq!(usage.input_tokens, Some(1000));
    assert_eq!(usage.output_tokens, Some(400));
    assert_eq!(usage.cache_read_input_tokens, Some(600));
    assert_eq!(usage.reasoning_output_tokens, Some(125));
}

#[test]
fn parse_token_usage_with_input_token_details_cached_tokens_jsonl() {
    let output = r#"{"usage":{"input_tokens":1000,"output_tokens":400,"input_tokens_details":{"cached_tokens":650},"output_tokens_details":{"reasoning_tokens":125}}}"#;

    let usage = super::parse_token_usage(output).unwrap();

    assert_eq!(usage.input_tokens, Some(1000));
    assert_eq!(usage.output_tokens, Some(400));
    assert_eq!(usage.cache_read_input_tokens, Some(650));
    assert_eq!(usage.reasoning_output_tokens, Some(125));
    assert_eq!(usage.uncached_input_tokens(), Some(350));
}

#[test]
fn parse_token_usage_cache_read_only_does_not_set_input() {
    // When only cache_read_input_tokens is present, the lookback guard prevents
    // the longer key from being mis-parsed as a bare input_tokens hit.
    let output = "cache_read_input_tokens: 750";
    let usage = super::parse_token_usage(output).unwrap();
    assert_eq!(usage.cache_read_input_tokens, Some(750));
    assert_eq!(usage.input_tokens, None);
}

#[test]
fn parse_token_usage_cached_input_only_does_not_set_input() {
    let output = r#"{"type":"turn.completed","usage":{"cached_input_tokens":750}}"#;
    let usage = super::parse_token_usage(output).unwrap();
    assert_eq!(usage.cache_read_input_tokens, Some(750));
    assert_eq!(usage.input_tokens, None);
}

#[test]
fn parse_token_usage_reasoning_only_does_not_set_output() {
    let output = "reasoning_output_tokens: 125";
    let usage = super::parse_token_usage(output).unwrap();
    assert_eq!(usage.reasoning_output_tokens, Some(125));
    assert_eq!(usage.output_tokens, None);
}
