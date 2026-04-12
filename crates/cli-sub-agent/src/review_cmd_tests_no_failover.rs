//! Tests for --no-failover / --tier / --model-spec conflict handling in csa review.

use super::*;

#[test]
fn resolve_review_tool_rejects_model_spec_with_tier() {
    let global = GlobalConfig::default();
    let cfg = project_config_with_enabled_tools(&["codex"]);
    let err = resolve_review_tool(
        None,
        Some("codex/openai/gpt-5.4/xhigh"),
        Some(&cfg),
        &global,
        Some("claude-code"),
        std::path::Path::new("/tmp/test-project"),
        false,
        Some("tier-2-standard"),
        false,
    )
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("--model-spec and --tier are mutually exclusive"),
        "unexpected error: {err:#}"
    );
}
