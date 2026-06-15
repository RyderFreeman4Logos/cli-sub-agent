use super::*;
use crate::test_session_sandbox::ScopedSessionSandbox;
use csa_session::create_session;

#[test]
fn update_cumulative_tokens_does_not_zero_missing_fields() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new_blocking(&tmp);
    let project_root = tmp.path();
    let mut session =
        create_session(project_root, Some("test"), None, Some("codex")).expect("create session");

    update_cumulative_tokens(
        &mut session,
        Some(csa_session::TokenUsage {
            cache_read_input_tokens: Some(750),
            reasoning_output_tokens: Some(125),
            ..Default::default()
        }),
    );

    let usage = session
        .total_token_usage
        .as_ref()
        .expect("usage should be created");
    assert_eq!(usage.input_tokens, None);
    assert_eq!(usage.output_tokens, None);
    assert_eq!(usage.total_tokens, None);
    assert_eq!(usage.estimated_cost_usd, None);
    assert_eq!(usage.cache_read_input_tokens, Some(750));
    assert_eq!(usage.reasoning_output_tokens, Some(125));
}

#[test]
fn update_cumulative_tokens_accumulates_only_reported_fields() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new_blocking(&tmp);
    let project_root = tmp.path();
    let mut session =
        create_session(project_root, Some("test"), None, Some("codex")).expect("create session");

    update_cumulative_tokens(
        &mut session,
        Some(csa_session::TokenUsage {
            input_tokens: Some(1_000),
            output_tokens: Some(200),
            total_tokens: Some(1_200),
            cache_read_input_tokens: Some(700),
            ..Default::default()
        }),
    );
    update_cumulative_tokens(
        &mut session,
        Some(csa_session::TokenUsage {
            output_tokens: Some(50),
            reasoning_output_tokens: Some(25),
            ..Default::default()
        }),
    );

    let usage = session
        .total_token_usage
        .as_ref()
        .expect("usage should be created");
    assert_eq!(usage.input_tokens, Some(1_000));
    assert_eq!(usage.output_tokens, Some(250));
    assert_eq!(usage.total_tokens, Some(1_200));
    assert_eq!(usage.cache_read_input_tokens, Some(700));
    assert_eq!(usage.reasoning_output_tokens, Some(25));
    assert_eq!(usage.uncached_input_tokens(), Some(300));
}
