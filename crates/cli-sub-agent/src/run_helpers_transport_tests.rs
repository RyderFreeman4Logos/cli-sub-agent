use super::{build_executor, is_tool_binary_available_for_config, resolve_tool_and_model};
use csa_config::{ProjectConfig, ProjectMeta, ResourcesConfig, ToolConfig, ToolTransport};
use csa_core::types::ToolName;
use csa_executor::{CodexTransport, Executor};
use std::collections::HashMap;

fn project_config_with_codex_tool(tool_config: ToolConfig) -> ProjectConfig {
    let mut tools = HashMap::new();
    tools.insert("codex".to_string(), tool_config);

    ProjectConfig {
        schema_version: 1,
        project: ProjectMeta::default(),
        resources: ResourcesConfig::default(),
        acp: Default::default(),
        tools,
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        tool_aliases: HashMap::new(),
        preferences: None,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        run: Default::default(),
        execution: Default::default(),
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
    }
}

#[test]
fn build_executor_codex_transport_defaults_to_build_setting_when_unset() {
    let config = project_config_with_codex_tool(ToolConfig::default());
    let exec = build_executor(&ToolName::Codex, None, None, None, Some(&config), true).unwrap();

    match exec {
        Executor::Codex {
            runtime_metadata, ..
        } => {
            assert_eq!(
                runtime_metadata.transport_mode(),
                CodexTransport::default_for_build()
            );
        }
        other => panic!("expected codex executor, got: {other:?}"),
    }
}

#[test]
fn build_executor_codex_transport_respects_explicit_cli_override() {
    let config = project_config_with_codex_tool(ToolConfig {
        transport: Some(ToolTransport::Cli),
        ..Default::default()
    });
    let exec = build_executor(&ToolName::Codex, None, None, None, Some(&config), true).unwrap();

    match exec {
        Executor::Codex {
            runtime_metadata, ..
        } => {
            assert_eq!(runtime_metadata.transport_mode(), CodexTransport::Cli);
        }
        other => panic!("expected codex executor, got: {other:?}"),
    }
}

#[test]
fn build_executor_codex_transport_respects_explicit_acp_override() {
    let config = project_config_with_codex_tool(ToolConfig {
        transport: Some(ToolTransport::Acp),
        ..Default::default()
    });
    let exec = build_executor(&ToolName::Codex, None, None, None, Some(&config), true).unwrap();

    match exec {
        Executor::Codex {
            runtime_metadata, ..
        } => {
            assert_eq!(runtime_metadata.transport_mode(), CodexTransport::Acp);
        }
        other => panic!("expected codex executor, got: {other:?}"),
    }
}

#[cfg(unix)]
#[test]
fn auto_selection_uses_codex_cli_when_transport_is_unset() {
    use crate::test_env_lock::ScopedTestEnvVar;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let td = tempfile::tempdir().expect("tempdir");
    let bin_dir = td.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("create bin dir");
    let codex_path = bin_dir.join("codex");
    fs::write(&codex_path, "#!/bin/sh\necho 'codex 9.9.9'\n").expect("write codex stub");
    let mut perms = fs::metadata(&codex_path).expect("metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&codex_path, perms).expect("chmod codex");

    let path = std::env::var_os("PATH").unwrap_or_default();
    let joined =
        std::env::join_paths(std::iter::once(bin_dir.clone()).chain(std::env::split_paths(&path)))
            .expect("join PATH");
    let _path_guard = ScopedTestEnvVar::set("PATH", joined);

    let mut config = project_config_with_codex_tool(ToolConfig::default());
    config.tools.insert(
        "gemini-cli".to_string(),
        ToolConfig {
            enabled: false,
            ..Default::default()
        },
    );
    config.tools.insert(
        "opencode".to_string(),
        ToolConfig {
            enabled: false,
            ..Default::default()
        },
    );
    config.tools.insert(
        "claude-code".to_string(),
        ToolConfig {
            enabled: false,
            ..Default::default()
        },
    );

    assert!(
        is_tool_binary_available_for_config("codex", Some(&config)),
        "unset codex transport should probe `codex`, not `codex-acp`"
    );

    let (tool, model_spec, model) = resolve_tool_and_model(
        None,
        None,
        None,
        Some(&config),
        td.path(),
        false,
        false,
        false,
        None,
        false,
        true,
    )
    .expect("resolve tool");

    assert_eq!(tool, ToolName::Codex);
    assert_eq!(model_spec, None);
    assert_eq!(model, None);
}
