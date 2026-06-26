use super::*;
use tempfile::tempdir;

fn load_error(contents: &str) -> String {
    let dir = tempdir().unwrap();
    let config_dir = dir.path().join(".csa");
    std::fs::create_dir_all(&config_dir).unwrap();
    let project_path = config_dir.join("config.toml");
    std::fs::write(&project_path, contents).unwrap();
    let err = ProjectConfig::load_with_paths(None, &project_path).unwrap_err();
    format!("{err:?}")
}

fn load_ok(contents: &str) {
    let dir = tempdir().unwrap();
    let config_dir = dir.path().join(".csa");
    std::fs::create_dir_all(&config_dir).unwrap();
    let project_path = config_dir.join("config.toml");
    std::fs::write(&project_path, contents).unwrap();
    ProjectConfig::load_with_paths(None, &project_path).unwrap();
}

fn raw_ok(contents: &str) {
    let raw = toml::from_str::<toml::Value>(contents).unwrap();
    crate::validate::reject_removed_gemini_cli_in_raw_config(&raw, "test-config").unwrap();
}

#[test]
fn load_rejects_removed_gemini_cli_tool_section() {
    let message = load_error("[tools.gemini-cli]\nenabled = true\n");
    assert!(message.contains("removed tool reference"), "{message}");
    assert!(
        message.contains("gemini-cli integration has been removed"),
        "{message}"
    );
}

#[test]
fn load_rejects_removed_gemini_cli_tier_model() {
    let message = load_error(
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
    let message = load_error("[tool_aliases]\ngem = \"gemini-cli\"\n");
    assert!(message.contains("$.tool_aliases.gem"), "{message}");
    assert!(message.contains("no longer supported"), "{message}");
}

#[test]
fn load_rejects_removed_gemini_review_tool_alias() {
    let message = load_error("[review]\ntool = \"gemini\"\n");
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
        load_error("[aliases]\nlegacy = \"gemini-cli/google/gemini-3-pro-preview/xhigh\"\n");
    assert!(message.contains("$.aliases.legacy"), "{message}");
    assert!(message.contains("removed tool reference"), "{message}");
}

#[test]
fn load_allows_unrelated_project_name_gemini() {
    load_ok("[project]\nname = \"gemini\"\n");
}

#[test]
fn raw_scan_allows_unrelated_global_review_gate_name_gemini() {
    raw_ok("[[review.gates]]\nname = \"gemini\"\ncommand = \"true\"\n");
}
