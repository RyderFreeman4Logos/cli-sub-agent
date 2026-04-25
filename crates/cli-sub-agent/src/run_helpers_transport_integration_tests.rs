use super::build_executor;
use csa_config::ProjectConfig;
use csa_core::types::ToolName;
use csa_executor::{
    ClaudeCodeRuntimeMetadata, ClaudeCodeTransport, Executor, SessionConfig, TransportFactory,
    TransportMode,
};
use std::fs;
use std::path::Path;

fn write_project_config(project_root: &Path, config_toml: &str) {
    let config_dir = project_root.join(".csa");
    fs::create_dir_all(&config_dir).expect("create .csa");
    fs::write(config_dir.join("config.toml"), config_toml).expect("write config");
}

fn load_project_config(config_toml: &str) -> ProjectConfig {
    let dir = tempfile::tempdir().expect("tempdir");
    write_project_config(dir.path(), config_toml);
    ProjectConfig::load_project_only(dir.path())
        .expect("load project config")
        .expect("project config should exist")
}

fn assert_legacy_transport(executor: Executor, expected_binary: &str) {
    let tool_name = executor.tool_name();
    let transport = TransportFactory::create(&executor, Some(SessionConfig::default()))
        .expect("transport should build");

    assert_eq!(transport.mode(), TransportMode::Legacy);
    assert_eq!(
        executor.runtime_binary_name(),
        expected_binary,
        "unexpected runtime binary for {tool_name}"
    );
}

fn assert_non_codex_transport_defaults() {
    // claude-code defaults to CLI transport now (#1115/#1117 workaround).
    // `Executor::from_tool_name` uses the metadata-level default (Acp) which
    // would fail the feature gate without `claude-code-acp`. Build explicitly
    // with Cli metadata to test the effective default routing.
    let claude_executor = Executor::ClaudeCode {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: ClaudeCodeRuntimeMetadata::from_transport(ClaudeCodeTransport::Cli),
    };
    assert_legacy_transport(claude_executor, "claude");
    assert_legacy_transport(
        Executor::from_tool_name(&ToolName::GeminiCli, None, None),
        "gemini",
    );
    assert_legacy_transport(
        Executor::from_tool_name(&ToolName::Opencode, None, None),
        "opencode",
    );
}

#[test]
fn codex_defaults_to_acp_transport_end_to_end() {
    let executor = build_executor(&ToolName::Codex, None, None, None, None, true)
        .expect("build default codex executor");
    let transport = TransportFactory::create(&executor, Some(SessionConfig::default()))
        .expect("create codex transport");

    assert_eq!(transport.mode(), TransportMode::Acp);
    assert_eq!(executor.runtime_binary_name(), "codex-acp");

    assert_non_codex_transport_defaults();
}

#[test]
fn codex_acp_project_config_builds_acp_transport_end_to_end() {
    let config = load_project_config(
        r#"
[tools.codex]
transport = "acp"
"#,
    );
    let executor = build_executor(&ToolName::Codex, None, None, None, Some(&config), true)
        .expect("build codex executor");
    let transport = TransportFactory::create(&executor, Some(SessionConfig::default()))
        .expect("create codex transport");

    assert_eq!(transport.mode(), TransportMode::Acp);
    assert_eq!(executor.runtime_binary_name(), "codex-acp");

    assert_non_codex_transport_defaults();
}

#[test]
fn codex_cli_project_config_is_rejected_before_executor_build() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_project_config(
        dir.path(),
        r#"
[tools.codex]
transport = "cli"
"#,
    );

    let err = ProjectConfig::load_project_only(dir.path()).expect_err("config should fail");
    let message = format!("{err:#}");

    assert!(
        message.contains("tools.codex.transport"),
        "error should point to the codex transport key: {message}"
    );
    assert!(
        message.contains("#643 Phase 4"),
        "error should mention the codex CLI follow-up phase: {message}"
    );
}
