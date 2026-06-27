use super::*;
use tempfile::tempdir;

fn project_path_with_contents(contents: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempdir().unwrap();
    let config_dir = dir.path().join(".csa");
    std::fs::create_dir_all(&config_dir).unwrap();
    let project_path = config_dir.join("config.toml");
    std::fs::write(&project_path, contents).unwrap();
    (dir, project_path)
}

fn user_path_with_contents(contents: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempdir().unwrap();
    let user_path = dir.path().join("config.toml");
    std::fs::write(&user_path, contents).unwrap();
    (dir, user_path)
}

fn load_user_error(contents: &str) -> String {
    let (_dir, user_path) = user_path_with_contents(contents);
    let missing_project_path = user_path.parent().unwrap().join("missing-project.toml");
    let err = ProjectConfig::load_with_paths(Some(&user_path), &missing_project_path).unwrap_err();
    format!("{err:?}")
}

fn validate_project_error(contents: &str) -> String {
    let (_dir, project_path) = project_path_with_contents(contents);
    let err = crate::validate::validate_config_with_paths(None, &project_path).unwrap_err();
    format!("{err:?}")
}

fn validate_user_error(contents: &str) -> String {
    let (_dir, user_path) = user_path_with_contents(contents);
    let missing_project_path = user_path.parent().unwrap().join("missing-project.toml");
    let err = crate::validate::validate_config_with_paths(Some(&user_path), &missing_project_path)
        .unwrap_err();
    format!("{err:?}")
}

fn load_ok(contents: &str) {
    let (_dir, project_path) = project_path_with_contents(contents);
    ProjectConfig::load_with_paths(None, &project_path).unwrap();
}

fn load_project(contents: &str) -> ProjectConfig {
    let (_dir, project_path) = project_path_with_contents(contents);
    ProjectConfig::load_with_paths(None, &project_path)
        .unwrap()
        .expect("project config should exist")
}

fn raw_ok(contents: &str) {
    let raw = toml::from_str::<toml::Value>(contents).unwrap();
    crate::validate::reject_removed_gemini_cli_in_raw_config(&raw, "test-config").unwrap();
}

#[test]
fn load_rejects_removed_gemini_cli_tool_section() {
    let message = load_user_error("[tools.gemini-cli]\nenabled = true\n");
    assert!(message.contains("removed tool reference"), "{message}");
    assert!(
        message.contains("gemini-cli integration has been removed"),
        "{message}"
    );
}

#[test]
fn load_rejects_removed_gemini_cli_tier_model() {
    let message = load_user_error(
        "[tiers.tier-1]\ndescription = \"deprecated\"\nmodels = [\"gemini-cli/google/gemini-3-pro-preview/xhigh\"]\n",
    );
    assert!(message.contains("$.tiers.tier-1.models[0]"), "{message}");
    assert!(
        message.contains("Do not replace it with antigravity-cli"),
        "{message}"
    );
}

#[test]
fn load_rejects_removed_gemini_cli_tool_alias() {
    let message = load_user_error("[tool_aliases]\ngem = \"gemini-cli\"\n");
    assert!(message.contains("$.tool_aliases.gem"), "{message}");
    assert!(message.contains("no longer supported"), "{message}");
}

#[test]
fn load_rejects_removed_gemini_review_tool_alias() {
    let message = load_user_error("[review]\ntool = \"gemini\"\n");
    assert!(message.contains("$.review.tool"), "{message}");
    assert!(message.contains("removed tool reference"), "{message}");
}

#[test]
fn raw_scan_rejects_removed_gemini_defaults_tool() {
    let raw = toml::from_str::<toml::Value>("[defaults]\ntool = \"gemini-cli\"\n").unwrap();
    let message = crate::validate::reject_removed_gemini_cli_in_raw_config(&raw, "test-config")
        .unwrap_err()
        .to_string();
    assert!(message.contains("$.defaults.tool"), "{message}");
    assert!(message.contains("removed tool reference"), "{message}");
}

#[test]
fn load_allows_removed_gemini_cli_model_alias_key() {
    load_ok("[aliases]\ngemini-cli = \"codex/openai/gpt-5.5/xhigh\"\n");
}

#[test]
fn load_rejects_removed_gemini_cli_model_alias_value() {
    let message =
        load_user_error("[aliases]\nlegacy = \"gemini-cli/google/gemini-3-pro-preview/xhigh\"\n");
    assert!(message.contains("$.aliases.legacy"), "{message}");
    assert!(message.contains("removed tool reference"), "{message}");
}

#[test]
fn load_project_prunes_removed_and_catalog_stale_tier_models_when_fallback_exists() {
    let config = load_project(
        r#"
[review]
tier = "tier-review"

[tiers.tier-review]
description = "Review tier with stale project fallback"
models = [
    "gemini-cli/google/gemini-3-pro-preview/xhigh",
    "codex/openai/o3/medium",
    "claude-code/anthropic/claude-sonnet-4-20250514/none",
]
"#,
    );

    let tier = config
        .tiers
        .get("tier-review")
        .expect("tier-review should remain after pruning stale model");
    assert_eq!(
        tier.models,
        vec!["claude-code/anthropic/claude-sonnet-4-20250514/none"]
    );
}

#[test]
fn load_project_prunes_removed_scalar_tool_value() {
    let config = load_project(
        r#"
[review]
tool = "gemini-cli"
tier = "tier-review"

[tiers.tier-review]
description = "Review tier with valid fallback"
models = ["claude-code/anthropic/claude-sonnet-4-20250514/none"]
"#,
    );

    let review = config.review.expect("review section should remain");
    assert_eq!(
        review.tool,
        crate::ToolSelection::Single("auto".to_string())
    );
    assert_eq!(review.tier.as_deref(), Some("tier-review"));
}

#[test]
fn load_project_still_fails_when_pruning_leaves_tier_empty() {
    let message = validate_project_error(
        r#"
[tiers.tier-review]
description = "All stale"
models = ["gemini-cli/google/gemini-3-pro-preview/xhigh"]
"#,
    );

    assert!(
        message.contains("Tier 'tier-review' must have at least one model"),
        "{message}"
    );
}

#[test]
fn validate_user_config_still_rejects_catalog_stale_tier_model() {
    let message = validate_user_error(
        r#"
[tiers.tier-review]
description = "User typo should fail closed"
models = ["codex/openai/o3/medium"]
"#,
    );

    assert!(message.contains("unknown model 'o3'"), "{message}");
}

#[test]
fn load_allows_unrelated_project_name_gemini() {
    load_ok("[project]\nname = \"gemini\"\n");
}

#[test]
fn raw_scan_allows_unrelated_global_review_gate_name_gemini() {
    raw_ok("[[review.gates]]\nname = \"gemini\"\ncommand = \"true\"\n");
}
