use super::*;

#[test]
fn native_codex_pr_bot_entrypoint_injects_parent_provider() {
    let mut vars = Vec::new();

    inject_pr_bot_parent_provider(
        &None,
        &Some("pr-bot".to_string()),
        &mut vars,
        Some("codex"),
        None,
    );

    assert_eq!(vars, ["CSA_MODEL_PROVIDER=openai"]);
}

#[test]
fn native_claude_file_entrypoint_injects_parent_provider() {
    let mut vars = Vec::new();

    inject_pr_bot_parent_provider(
        &Some("patterns/pr-bot/workflow.toml".to_string()),
        &None,
        &mut vars,
        Some("claude-code"),
        None,
    );

    assert_eq!(vars, ["CSA_MODEL_PROVIDER=claude"]);
}

#[test]
fn explicit_pr_bot_provider_key_wins_unchanged() {
    let mut vars = vec!["CSA_MODEL_PROVIDER=anthropic".to_string()];

    inject_pr_bot_parent_provider(
        &None,
        &Some("pr-bot".to_string()),
        &mut vars,
        Some("claude-code"),
        None,
    );

    assert_eq!(vars, ["CSA_MODEL_PROVIDER=anthropic"]);
}

#[test]
fn hermes_config_provider_requires_detected_hermes_parent() {
    let mut native_vars = Vec::new();
    inject_pr_bot_parent_provider(
        &None,
        &Some("pr-bot".to_string()),
        &mut native_vars,
        Some("opencode"),
        Some("xai"),
    );
    assert!(native_vars.is_empty());

    let mut hermes_vars = Vec::new();
    inject_pr_bot_parent_provider(
        &None,
        &Some("pr-bot".to_string()),
        &mut hermes_vars,
        Some("hermes"),
        Some("xai"),
    );
    assert_eq!(hermes_vars, ["CSA_MODEL_PROVIDER=xai"]);
}

#[test]
fn every_pr_bot_wait_routes_through_the_provider_qualified_helper() {
    let workflow = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../patterns/pr-bot/workflow.toml"),
    )
    .expect("read pr-bot workflow");

    assert_eq!(
        workflow.matches("session-wait-until-done.sh").count(),
        5,
        "every documented pr-bot wait must route through the shared provider helper"
    );
    assert!(
        !workflow.contains("csa session wait"),
        "pr-bot must not bypass the provider-qualified helper"
    );
}
