use super::*;
use crate::test_env_lock::{ScopedTestEnvVar, TEST_ENV_LOCK};
use csa_config::{ProjectMeta, ResourcesConfig, ToolConfig, TransportKind};
use serde_json::Value;
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::Path;

fn project_config_with_tool_transport(tool_name: &str, transport: TransportKind) -> ProjectConfig {
    let mut tools = HashMap::new();
    tools.insert(
        tool_name.to_string(),
        ToolConfig {
            transport: Some(transport),
            ..Default::default()
        },
    );

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
        github: None,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        run: Default::default(),
        execution: Default::default(),
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        tool_state_dirs: HashMap::new(),
        filesystem_sandbox: Default::default(),
    }
}

fn project_config_with_codex_transport(transport: TransportKind) -> ProjectConfig {
    project_config_with_tool_transport("codex", transport)
}

fn project_config_with_claude_code_transport(transport: TransportKind) -> ProjectConfig {
    project_config_with_tool_transport("claude-code", transport)
}

fn project_config_with_disabled_tool(tool_name: &str, transport: TransportKind) -> ProjectConfig {
    let mut config = project_config_with_tool_transport(tool_name, transport);
    config
        .tools
        .get_mut(tool_name)
        .expect("tool config should exist")
        .enabled = false;
    config
}

fn write_project_config(project_root: &Path, contents: &str) {
    let config_dir = project_root.join(".csa");
    std::fs::create_dir_all(&config_dir).expect("create config dir");
    std::fs::write(config_dir.join("config.toml"), contents).expect("write config");
}

struct EnvVarGuard {
    key: &'static str,
    original: Option<OsString>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let original = std::env::var_os(key);
        // SAFETY: test-scoped env mutation guarded by a process-wide mutex.
        unsafe { std::env::set_var(key, value) };
        Self { key, original }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        // SAFETY: restoration of test-scoped env mutation guarded by a process-wide mutex.
        unsafe {
            match self.original.take() {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

#[cfg(unix)]
fn with_stubbed_binaries_on_path<T>(binaries: &[&str], test_fn: impl FnOnce() -> T) -> T {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let td = tempfile::tempdir().expect("tempdir");
    let bin_dir = td.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("create bin dir");
    for binary in binaries {
        let binary_path = bin_dir.join(binary);
        fs::write(&binary_path, format!("#!/bin/sh\necho '{binary} 1.2.3'\n"))
            .expect("write tool stub");
        let mut perms = fs::metadata(&binary_path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&binary_path, perms).expect("chmod tool stub");
    }

    let path = std::env::var_os("PATH").unwrap_or_default();
    let joined =
        std::env::join_paths(std::iter::once(bin_dir.clone()).chain(std::env::split_paths(&path)))
            .expect("join PATH");
    let _path_guard = ScopedTestEnvVar::set("PATH", joined);

    test_fn()
}

#[cfg(unix)]
fn with_stubbed_codex_on_path<T>(test_fn: impl FnOnce() -> T) -> T {
    with_stubbed_binaries_on_path(&["codex", "codex-acp"], test_fn)
}

#[cfg(unix)]
fn with_stubbed_claude_code_on_path<T>(test_fn: impl FnOnce() -> T) -> T {
    with_stubbed_binaries_on_path(&["claude", "claude-code-acp"], test_fn)
}

#[cfg(unix)]
fn with_stubbed_hermes_on_path<T>(test_fn: impl FnOnce() -> T) -> T {
    with_stubbed_binaries_on_path(&["hermes"], test_fn)
}

#[test]
fn test_format_bytes() {
    assert_eq!(format_bytes(0), "0.0 GB");
    assert_eq!(format_bytes(1024 * 1024 * 1024), "1.0 GB");
    assert_eq!(format_bytes(8 * 1024 * 1024 * 1024), "8.0 GB");
    assert_eq!(format_bytes(8589934592), "8.0 GB"); // 8 GB in bytes
}

#[test]
fn test_check_tool_version_nonexistent() {
    let version = check_tool_version("nonexistent-tool-12345");
    assert!(version.is_none());
}

#[test]
fn test_check_tool_status_nonexistent() {
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let status = check_tool_status("nonexistent-tool", None);
    assert!(!status.is_ready());
    assert!(status.config_enabled);
    assert!(!status.binary_available());
    assert!(status.version.is_none());
}

// Regression for #1714: in the default build the `codex-acp` cargo feature is
// OFF, so codex actually runs via the `codex` CLI (`mode_for_executor` routes
// `None`/`Some(Cli)` through the CLI transport). The doctor MUST therefore
// report the CLI transport / `codex` binary it will really use — reporting the
// `codex-acp` build default was a false-negative that silently dropped codex
// from tier failover.
#[cfg(unix)]
#[test]
fn default_build_doctor_accepts_codex_cli_runtime() {
    with_stubbed_codex_on_path(|| {
        let status = check_tool_status("codex", None);
        assert!(
            status.is_ready(),
            "doctor should accept the default codex CLI transport"
        );
        assert_eq!(status.binary_name, "codex");
        assert!(status.config_enabled);
        assert!(status.binary_available());
        assert!(status.hint.is_none());
    });
}

#[cfg(unix)]
#[test]
fn doctor_status_uses_runtime_gate_when_tool_is_disabled_but_binary_exists() {
    with_stubbed_codex_on_path(|| {
        let config = project_config_with_disabled_tool("codex", TransportKind::Cli);
        let status = check_tool_status("codex", Some(&config));
        let rendered = render_tool_status_lines(&status).join("\n");
        let json = tool_status_json(&status);

        assert!(
            !status.is_ready(),
            "doctor readiness must reject tools disabled in runtime config"
        );
        assert!(!status.config_enabled);
        assert!(status.binary_available());
        assert_eq!(status.availability, ToolAvailabilityState::Installed);
        assert!(
            rendered.contains("Enabled: no"),
            "overall doctor Enabled line must match runtime gate: {rendered}"
        );
        assert!(
            rendered.contains("Config enabled: no"),
            "doctor text should show the config condition: {rendered}"
        );
        assert!(
            rendered.contains("Binary available: yes"),
            "doctor text should show the binary condition: {rendered}"
        );
        assert_eq!(json["enabled"], Value::Bool(false));
        assert_eq!(json["config_enabled"], Value::Bool(false));
        assert_eq!(json["binary_available"], Value::Bool(true));
        assert_eq!(json["installed"], Value::Bool(true));
    });
}

#[cfg(unix)]
#[test]
fn default_build_doctor_text_output_reports_codex_transport_details() {
    with_stubbed_codex_on_path(|| {
        let status = check_tool_status("codex", None);
        let rendered = render_tool_status_lines(&status).join("\n");

        assert!(
            rendered.contains("Active transport: cli"),
            "doctor text should report active codex transport: {rendered}"
        );
        assert!(
            rendered.contains("ACP compiled in:"),
            "doctor text should report ACP compile status: {rendered}"
        );
        assert!(
            rendered.contains("Probed binary: codex"),
            "doctor text should report the probed codex CLI binary: {rendered}"
        );
        assert!(
            !rendered.contains("codex-acp"),
            "default build doctor must not name the codex-acp binary: {rendered}"
        );
    });
}

#[cfg(unix)]
#[test]
fn default_build_doctor_json_output_reports_codex_transport_details() {
    with_stubbed_codex_on_path(|| {
        let status = check_tool_status("codex", None);
        let json = tool_status_json(&status);

        assert_eq!(json["transport_active"], Value::String("cli".to_string()));
        assert_eq!(
            json["acp_compiled_in"],
            Value::Bool(csa_executor::CodexRuntimeMetadata::acp_compiled_in())
        );
        assert_eq!(json["probed_binary"], Value::String("codex".to_string()));
        // With codex on the CLI transport the override hint depends on whether
        // the `codex-acp` cargo feature is compiled in: with it OFF (default
        // build) there is no ACP to opt into, so the hint is omitted; with it
        // ON (`--all-features`) the doctor surfaces the ACP opt-in hint.
        if csa_executor::CodexRuntimeMetadata::acp_compiled_in() {
            assert_eq!(
                json.get("acp_override_hint"),
                Some(&Value::String(
                    "set [tools.codex].transport = \"acp\"".to_string()
                )),
                "doctor JSON should surface the ACP opt-in hint when ACP is compiled in but CLI is active: {json}"
            );
        } else {
            assert!(
                json.get("acp_override_hint").is_none(),
                "doctor JSON should omit ACP override hints when ACP is not compiled in: {json}"
            );
        }
    });
}

#[cfg(unix)]
#[test]
fn doctor_reports_hermes_binary_hint_and_acp_transport() {
    with_stubbed_hermes_on_path(|| {
        let status = check_tool_status("hermes", None);
        let rendered = render_tool_status_lines(&status).join("\n");

        assert!(status.is_ready(), "Hermes stub should satisfy doctor");
        assert_eq!(status.binary_name, "hermes");
        assert!(status.binary_available());
        assert!(status.hint.is_none());
        assert!(
            rendered.contains("Active transport: acp"),
            "Hermes doctor should report ACP transport: {rendered}"
        );
        assert!(
            rendered.contains("Probed binary: hermes"),
            "Hermes doctor should probe the hermes binary: {rendered}"
        );
        assert!(
            rendered.contains("ACP compiled in:"),
            "Hermes doctor should report ACP compile status: {rendered}"
        );
    });
}

#[cfg(unix)]
#[test]
fn explicit_codex_acp_transport_reports_install_hint() {
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let td = tempfile::tempdir().expect("tempdir");
    let _path_guard = EnvVarGuard::set("PATH", td.path().join("bin"));
    let config = project_config_with_codex_transport(TransportKind::Acp);
    let status = check_tool_status("codex", Some(&config));
    let rendered = render_tool_status_lines(&status).join("\n");

    assert_eq!(status.availability, ToolAvailabilityState::Missing);
    assert!(
        rendered.contains("Active transport: acp"),
        "doctor text should report the requested ACP transport: {rendered}"
    );
    assert!(
        rendered.contains("@zed-industries/codex-acp"),
        "doctor text should surface the ACP adapter install hint: {rendered}"
    );
}

#[cfg(unix)]
#[test]
fn codex_cli_transport_json_matches_text_override_hint_visibility() {
    with_stubbed_codex_on_path(|| {
        let config = project_config_with_codex_transport(TransportKind::Cli);
        let status = check_tool_status("codex", Some(&config));
        let rendered = render_tool_status_lines(&status).join("\n");
        let json = tool_status_json(&status);
        let expected_hint = Value::String("set [tools.codex].transport = \"acp\"".to_string());

        assert_eq!(json["transport_active"], Value::String("cli".to_string()));
        assert_eq!(json["probed_binary"], Value::String("codex".to_string()));

        if rendered.contains("ACP override: set [tools.codex].transport = \"acp\"") {
            assert_eq!(
                json.get("acp_override_hint"),
                Some(&expected_hint),
                "doctor JSON should include the same ACP override hint shown in text output: {json}"
            );
        } else {
            assert!(
                json.get("acp_override_hint").is_none(),
                "doctor JSON should omit ACP override hints when text output has none: {json}"
            );
        }
    });
}

#[cfg(unix)]
#[test]
fn explicit_claude_code_cli_transport_reports_transport_details() {
    with_stubbed_claude_code_on_path(|| {
        let config = project_config_with_claude_code_transport(TransportKind::Cli);
        let status = check_tool_status("claude-code", Some(&config));
        let rendered = render_tool_status_lines(&status).join("\n");

        assert!(
            status.is_ready(),
            "doctor should accept the requested claude-code CLI transport"
        );
        assert_eq!(status.binary_name, "claude");
        assert!(
            rendered.contains("Active transport: cli"),
            "doctor text should report active claude-code transport: {rendered}"
        );
        assert!(
            rendered.contains("Probed binary: claude"),
            "doctor text should report the probed claude-code CLI binary: {rendered}"
        );
        assert!(
            !rendered.contains("ACP compiled in:"),
            "claude-code transport details should not print a codex-only ACP build line: {rendered}"
        );
    });
}

#[cfg(unix)]
#[test]
fn explicit_claude_code_cli_transport_reports_json_transport_details() {
    with_stubbed_claude_code_on_path(|| {
        let config = project_config_with_claude_code_transport(TransportKind::Cli);
        let status = check_tool_status("claude-code", Some(&config));
        let json = tool_status_json(&status);

        assert_eq!(json["transport_active"], Value::String("cli".to_string()));
        assert_eq!(json["probed_binary"], Value::String("claude".to_string()));
        assert!(
            json.get("acp_compiled_in").is_none(),
            "claude-code JSON transport details should omit codex-only ACP build metadata: {json}"
        );
        assert!(
            json.get("acp_override_hint").is_none(),
            "claude-code JSON transport details should omit codex-only ACP override hints: {json}"
        );
    });
}

#[cfg(unix)]
#[test]
fn doctor_json_includes_install_provenance_surface() {
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let td = tempfile::tempdir().expect("tempdir");
    let report = build_doctor_json(td.path());
    let install = &report["install"];
    assert!(
        install.is_object(),
        "doctor JSON must expose additive install surface: {report}"
    );
    assert!(
        install.get("status").and_then(|v| v.as_str()).is_some(),
        "install.status must be present: {install}"
    );
    assert!(
        install
            .get("intended_target")
            .and_then(|v| v.as_str())
            .is_some(),
        "install.intended_target must be present: {install}"
    );
    assert!(
        install.get("current").and_then(|v| v.as_bool()).is_some(),
        "install.current must be present: {install}"
    );
}

include!("doctor_tests_split.rs");
