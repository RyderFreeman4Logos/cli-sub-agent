use super::*;

#[test]
fn review_initial_response_timeout_is_resolved_per_reviewer_tool() {
    let mut cfg = super::tests::project_config_with_enabled_tools(&["gemini-cli", "codex"]);
    cfg.tools
        .get_mut("codex")
        .expect("codex config")
        .initial_response_timeout_seconds = Some(300);
    cfg.tools
        .get_mut("gemini-cli")
        .expect("gemini config")
        .initial_response_timeout_seconds = None;
    cfg.resources.initial_response_timeout_seconds = None;

    let gemini_timeout =
        resolve_review_initial_response_timeout_seconds(Some(&cfg), None, None, "gemini-cli");
    let codex_timeout =
        resolve_review_initial_response_timeout_seconds(Some(&cfg), None, None, "codex");

    assert_eq!(
        gemini_timeout,
        Some(crate::pipeline::DEFAULT_RESOURCES_INITIAL_RESPONSE_TIMEOUT_SECONDS)
    );
    assert_eq!(codex_timeout, Some(300));
}
