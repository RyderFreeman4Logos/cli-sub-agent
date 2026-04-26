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

/// codex now defaults to CLI transport (#760 / #1128 transport flip). The
/// resolved binary is `codex`, not `codex-acp`, and the transport routes to
/// Legacy.
///
/// The test passes an empty project config (rather than `None`) so the
/// resolver routes through `tool_transport()` → `default_transport_for_tool()`,
/// which is the path that exercises the post-#1128 default. The `None`-config
/// branch still falls back to the metadata-level `default_for_build()` (kept
/// at Acp for serde back-compat) — that's a known papercut mirrored from
/// PR #1120's claude-code treatment, not a regression introduced here.
#[test]
fn codex_defaults_to_cli_transport_end_to_end() {
    let config = load_project_config("schema_version = 1\n");
    let executor = build_executor(&ToolName::Codex, None, None, None, Some(&config), true)
        .expect("build default codex executor");
    let transport = TransportFactory::create(&executor, Some(SessionConfig::default()))
        .expect("create codex transport");

    assert_eq!(transport.mode(), TransportMode::Legacy);
    assert_eq!(executor.runtime_binary_name(), "codex");

    assert_non_codex_transport_defaults();
}

/// Explicit `transport = "acp"` for codex must build an ACP transport — but
/// only when the `codex-acp` cargo feature is enabled (#1128 feature gate).
#[test]
#[cfg(feature = "codex-acp")]
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

/// Without the `codex-acp` feature, an explicit `transport = "acp"` codex
/// config must fail at executor/transport build time with a clear error
/// citing the feature flag and #760/#1128.
#[test]
#[cfg(not(feature = "codex-acp"))]
fn codex_acp_project_config_is_rejected_without_feature() {
    let config = load_project_config(
        r#"
[tools.codex]
transport = "acp"
"#,
    );
    let executor = build_executor(&ToolName::Codex, None, None, None, Some(&config), true)
        .expect("build codex executor");
    let result = TransportFactory::create(&executor, Some(SessionConfig::default()));

    let err = match result {
        Ok(_) => panic!("codex+ACP must fail without codex-acp feature"),
        Err(e) => e,
    };
    let message = format!("{err:#}");
    assert!(
        message.contains("codex-acp") || message.contains("760") || message.contains("1128"),
        "error must cite the feature flag or issue number: {message}"
    );
}

/// Explicit `transport = "cli"` for codex is now accepted (#760 / #1128
/// transport flip). The resolved binary is `codex` and routing is Legacy.
#[test]
fn codex_cli_project_config_builds_legacy_transport_end_to_end() {
    let config = load_project_config(
        r#"
[tools.codex]
transport = "cli"
"#,
    );
    let executor = build_executor(&ToolName::Codex, None, None, None, Some(&config), true)
        .expect("build codex executor");
    let transport = TransportFactory::create(&executor, Some(SessionConfig::default()))
        .expect("create codex transport");

    assert_eq!(transport.mode(), TransportMode::Legacy);
    assert_eq!(executor.runtime_binary_name(), "codex");
}
