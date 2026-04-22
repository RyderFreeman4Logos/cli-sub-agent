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

fn project_config_with_codex_transport(transport: TransportKind) -> ProjectConfig {
    project_config_with_tool_transport("codex", transport)
}

fn project_config_with_claude_code_transport(transport: TransportKind) -> ProjectConfig {
    project_config_with_tool_transport("claude-code", transport)
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
    assert!(status.version.is_none());
}

#[cfg(unix)]
#[test]
fn default_build_doctor_accepts_codex_acp_runtime() {
    with_stubbed_codex_on_path(|| {
        let status = check_tool_status("codex", None);
        assert!(
            status.is_ready(),
            "doctor should accept the default codex ACP transport"
        );
        assert_eq!(status.binary_name, "codex-acp");
        assert!(status.hint.is_none());
    });
}

#[cfg(unix)]
#[test]
fn default_build_doctor_text_output_reports_codex_transport_details() {
    with_stubbed_codex_on_path(|| {
        let status = check_tool_status("codex", None);
        let rendered = render_tool_status_lines(&status).join("\n");

        assert!(
            rendered.contains("Active transport: acp"),
            "doctor text should report active codex transport: {rendered}"
        );
        assert!(
            rendered.contains("ACP compiled in:"),
            "doctor text should report ACP compile status: {rendered}"
        );
        assert!(
            rendered.contains("Probed binary: codex-acp"),
            "doctor text should report the probed codex ACP binary: {rendered}"
        );
    });
}

#[cfg(unix)]
#[test]
fn default_build_doctor_json_output_reports_codex_transport_details() {
    with_stubbed_codex_on_path(|| {
        let status = check_tool_status("codex", None);
        let json = tool_status_json(&status);

        assert_eq!(json["transport_active"], Value::String("acp".to_string()));
        assert_eq!(
            json["acp_compiled_in"],
            Value::Bool(csa_executor::CodexRuntimeMetadata::acp_compiled_in())
        );
        assert_eq!(
            json["probed_binary"],
            Value::String("codex-acp".to_string())
        );
        assert!(
            json.get("acp_override_hint").is_none(),
            "doctor JSON should omit ACP override hints when codex already uses ACP: {json}"
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

#[test]
fn doctor_load_rejects_invalid_codex_transport_override() {
    let td = tempfile::tempdir().expect("tempdir");
    write_project_config(
        td.path(),
        r#"
[tools.codex]
transport = "cli"
"#,
    );

    let err = load_doctor_project_config_from(td.path()).unwrap_err();
    let message = format!("{err:#}");

    assert!(
        message.contains("tools.codex.transport"),
        "doctor should surface the exact config key: {message}"
    );
    assert!(
        message.contains("#643 Phase 4"),
        "doctor should surface the codex CLI phase guidance: {message}"
    );
}

#[tokio::test]
async fn doctor_text_reports_invalid_codex_transport_without_aborting() {
    let td = tempfile::tempdir().expect("tempdir");
    write_project_config(
        td.path(),
        r#"
[tools.codex]
transport = "stdio"
"#,
    );

    let status = inspect_doctor_project_config_from(td.path());
    let rendered = render_project_config_lines(&status).join("\n");

    assert!(
        matches!(status, DoctorProjectConfigStatus::Invalid(_)),
        "doctor should classify invalid project config without aborting: {rendered}"
    );

    run_doctor_text_from(td.path())
        .await
        .expect("doctor text should keep running when project config is invalid");

    assert!(
        rendered.contains("Config:      .csa/config.toml (invalid)"),
        "doctor text should use the existing invalid config branch: {rendered}"
    );
    assert!(
        rendered.contains("Invalid tools.codex.transport"),
        "doctor text should surface the exact invalid transport key: {rendered}"
    );
    assert!(
        rendered.contains("unknown transport \"stdio\""),
        "doctor text should surface the invalid transport value: {rendered}"
    );
}

#[test]
fn doctor_json_reports_invalid_codex_transport() {
    let td = tempfile::tempdir().expect("tempdir");
    write_project_config(
        td.path(),
        r#"
[tools.codex]
transport = "stdio"
"#,
    );

    let report = build_doctor_json(td.path());
    let config = &report["config"];
    let error = config["error"]
        .as_str()
        .expect("doctor JSON should include invalid config error text");

    assert_eq!(config["found"], serde_json::json!(true));
    assert_eq!(config["valid"], serde_json::json!(false));
    assert!(
        error.contains("Invalid tools.codex.transport"),
        "doctor JSON should surface the exact invalid transport key: {error}"
    );
    assert!(
        error.contains("unknown transport \"stdio\""),
        "doctor JSON should surface the invalid transport value: {error}"
    );
}

#[test]
fn doctor_project_config_display_ignores_invalid_user_global_config() {
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let td = tempfile::tempdir().expect("tempdir");
    let config_root = td.path().join("xdg-config");
    std::fs::create_dir_all(&config_root).expect("create config root");
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _xdg_guard = EnvVarGuard::set("XDG_CONFIG_HOME", &config_root);

    let user_config_path = ProjectConfig::user_config_path().expect("resolve user config path");
    std::fs::create_dir_all(user_config_path.parent().expect("user config dir"))
        .expect("create user config dir");
    std::fs::write(
        &user_config_path,
        r#"
[tools.codex]
transport = "cli"
"#,
    )
    .expect("write invalid user config");

    write_project_config(
        td.path(),
        r#"
[tools.codex]
transport = "acp"
"#,
    );

    let merged_error = ProjectConfig::load(td.path()).expect_err("merged load should fail");
    assert!(
        format!("{merged_error:#}").contains("tools.codex.transport"),
        "test fixture should exercise an invalid user-level transport override: {merged_error}"
    );

    let status = inspect_doctor_project_config_from(td.path());
    let rendered = render_project_config_lines(&status).join("\n");

    assert!(
        matches!(status, DoctorProjectConfigStatus::Valid(_)),
        "doctor should validate .csa/config.toml independently of broken user config: {rendered}"
    );
    assert!(
        rendered.contains("Config:      .csa/config.toml (valid)"),
        "doctor should keep the project config display valid when user config is broken: {rendered}"
    );
}

#[test]
fn doctor_text_reports_invalid_effective_config() {
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let td = tempfile::tempdir().expect("tempdir");
    let config_root = td.path().join("xdg-config");
    std::fs::create_dir_all(&config_root).expect("create config root");
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _xdg_guard = EnvVarGuard::set("XDG_CONFIG_HOME", &config_root);

    let user_config_path = ProjectConfig::user_config_path().expect("resolve user config path");
    std::fs::create_dir_all(user_config_path.parent().expect("user config dir"))
        .expect("create user config dir");
    std::fs::write(
        &user_config_path,
        r#"
[tools.codex]
transport = "cli"
"#,
    )
    .expect("write invalid user config");

    write_project_config(
        td.path(),
        r#"
[tools.codex]
transport = "acp"
"#,
    );

    let effective_status = inspect_doctor_effective_config_from(td.path());
    let rendered = render_effective_config_lines(&effective_status).join("\n");
    let tool_lines = render_tool_availability_error_lines(
        &effective_status
            .tool_availability_error()
            .expect("tool availability error should be present"),
    )
    .join("\n");

    assert!(
        matches!(effective_status, DoctorEffectiveConfigStatus::Invalid(_)),
        "doctor should classify merged config failures explicitly: {rendered}"
    );
    assert!(
        rendered.contains("Effective:   merged config (invalid)"),
        "doctor text should surface the invalid effective-config branch: {rendered}"
    );
    assert!(
        rendered.contains("tools.codex.transport"),
        "doctor text should surface the exact merged-config key: {rendered}"
    );
    assert!(
        tool_lines.contains("Tool availability unknown (effective config invalid)"),
        "doctor text should not pretend defaults are ready when merged config failed: {tool_lines}"
    );

    tokio::runtime::Runtime::new()
        .expect("create tokio runtime")
        .block_on(run_doctor_text_from(td.path()))
        .expect("doctor text should keep running when effective config is invalid");
}

#[test]
fn doctor_json_reports_invalid_effective_config() {
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let td = tempfile::tempdir().expect("tempdir");
    let config_root = td.path().join("xdg-config");
    std::fs::create_dir_all(&config_root).expect("create config root");
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _xdg_guard = EnvVarGuard::set("XDG_CONFIG_HOME", &config_root);

    let user_config_path = ProjectConfig::user_config_path().expect("resolve user config path");
    std::fs::create_dir_all(user_config_path.parent().expect("user config dir"))
        .expect("create user config dir");
    std::fs::write(
        &user_config_path,
        r#"
[tools.codex]
transport = "cli"
"#,
    )
    .expect("write invalid user config");

    write_project_config(
        td.path(),
        r#"
[tools.codex]
transport = "acp"
"#,
    );

    let report = build_doctor_json(td.path());
    let effective = &report["effective_config"];
    let effective_error = effective["error"]
        .as_str()
        .expect("doctor JSON should include effective-config error text");
    let tools_error = report["tools_error"]
        .as_str()
        .expect("doctor JSON should include tool availability error text");

    assert_eq!(report["config"]["valid"], serde_json::json!(true));
    assert_eq!(effective["valid"], serde_json::json!(false));
    assert!(
        effective_error.contains("tools.codex.transport"),
        "doctor JSON should surface the exact merged-config key: {effective_error}"
    );
    assert!(
        tools_error.contains("Tool availability unknown (effective config invalid)"),
        "doctor JSON should mark tool availability unknown when merged config fails: {tools_error}"
    );
    assert_eq!(report["tools"], serde_json::json!([]));
}

#[cfg(all(unix, feature = "codex-acp"))]
#[test]
fn feature_build_doctor_text_output_reports_acp_compile_status() {
    with_stubbed_codex_on_path(|| {
        let status = check_tool_status("codex", None);
        let rendered = render_tool_status_lines(&status).join("\n");

        assert!(
            rendered.contains("ACP compiled in: yes"),
            "feature build should report that ACP support is compiled in: {rendered}"
        );
        assert!(
            rendered.contains("Active transport: acp"),
            "feature build should report the default ACP transport: {rendered}"
        );
    });
}
