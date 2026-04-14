use super::*;
use crate::test_env_lock::ScopedTestEnvVar;
#[cfg(not(feature = "codex-acp"))]
use csa_config::{ProjectMeta, ResourcesConfig, ToolConfig, ToolTransport};
#[cfg(not(feature = "codex-acp"))]
use serde_json::Value;
#[cfg(not(feature = "codex-acp"))]
use std::collections::HashMap;
use std::path::Path;

#[cfg(not(feature = "codex-acp"))]
fn project_config_with_codex_transport(transport: ToolTransport) -> ProjectConfig {
    let mut tools = HashMap::new();
    tools.insert(
        "codex".to_string(),
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
        execution: Default::default(),
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
    }
}

fn write_project_config(project_root: &Path, contents: &str) {
    let config_dir = project_root.join(".csa");
    std::fs::create_dir_all(&config_dir).expect("create config dir");
    std::fs::write(config_dir.join("config.toml"), contents).expect("write config");
}

#[cfg(unix)]
fn with_stubbed_codex_on_path<T>(test_fn: impl FnOnce() -> T) -> T {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let td = tempfile::tempdir().expect("tempdir");
    let bin_dir = td.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("create bin dir");
    let codex_path = bin_dir.join("codex");
    fs::write(&codex_path, "#!/bin/sh\necho 'codex 1.2.3'\n").expect("write codex stub");
    let mut perms = fs::metadata(&codex_path).expect("metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&codex_path, perms).expect("chmod codex");

    let path = std::env::var_os("PATH").unwrap_or_default();
    let joined =
        std::env::join_paths(std::iter::once(bin_dir.clone()).chain(std::env::split_paths(&path)))
            .expect("join PATH");
    let _path_guard = ScopedTestEnvVar::set("PATH", joined);

    test_fn()
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
    let status = check_tool_status("nonexistent-tool", None);
    assert!(!status.is_ready());
    assert!(status.version.is_none());
}

#[cfg(all(unix, not(feature = "codex-acp")))]
#[test]
fn default_build_doctor_accepts_codex_cli_runtime() {
    with_stubbed_codex_on_path(|| {
        let status = check_tool_status("codex", None);
        assert!(
            status.is_ready(),
            "doctor should accept the default codex CLI"
        );
        assert_eq!(status.binary_name, "codex");
        assert!(status.hint.is_none());
    });
}

#[cfg(all(unix, not(feature = "codex-acp")))]
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
            rendered.contains("ACP compiled in: no"),
            "doctor text should report that ACP is not compiled in by default: {rendered}"
        );
        assert!(
            rendered.contains("Probed binary: codex"),
            "doctor text should report the probed codex binary: {rendered}"
        );
    });
}

#[cfg(all(unix, not(feature = "codex-acp")))]
#[test]
fn default_build_doctor_json_output_reports_codex_transport_details() {
    with_stubbed_codex_on_path(|| {
        let status = check_tool_status("codex", None);
        let json = tool_status_json(&status);

        assert_eq!(json["transport_active"], Value::String("cli".to_string()));
        assert_eq!(json["acp_compiled_in"], Value::Bool(false));
        assert_eq!(json["probed_binary"], Value::String("codex".to_string()));
    });
}

#[cfg(all(unix, not(feature = "codex-acp")))]
#[test]
fn explicit_codex_acp_transport_reports_rebuild_hint() {
    let config = project_config_with_codex_transport(ToolTransport::Acp);
    let status = check_tool_status("codex", Some(&config));
    let rendered = render_tool_status_lines(&status).join("\n");

    assert_eq!(status.availability, ToolAvailabilityState::Unsupported);
    assert!(
        rendered.contains("Active transport: acp"),
        "doctor text should report the requested ACP transport: {rendered}"
    );
    assert!(
        rendered.contains("ACP compiled in: no"),
        "doctor text should report missing ACP compile support: {rendered}"
    );
    assert!(
        rendered.contains("cargo build --features codex-acp"),
        "doctor text should surface the rebuild hint from tool availability plumbing: {rendered}"
    );
}

#[cfg(not(feature = "codex-acp"))]
#[test]
fn doctor_load_rejects_invalid_codex_transport_override() {
    let td = tempfile::tempdir().expect("tempdir");
    write_project_config(
        td.path(),
        r#"
[tools.codex]
transport = "acp"
"#,
    );

    let err = load_doctor_project_config_from(td.path()).unwrap_err();
    let message = err.to_string();

    assert!(
        message.contains("[tools.codex].transport"),
        "doctor should surface the exact config key: {message}"
    );
    assert!(
        message.contains("codex-acp"),
        "doctor should surface the missing feature guidance: {message}"
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
        rendered.contains("Invalid [tools.codex].transport"),
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
        error.contains("Invalid [tools.codex].transport"),
        "doctor JSON should surface the exact invalid transport key: {error}"
    );
    assert!(
        error.contains("unknown transport \"stdio\""),
        "doctor JSON should surface the invalid transport value: {error}"
    );
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
            rendered.contains("ACP override: set [tools.codex].transport = \"acp\""),
            "feature build should surface the opt-in override when active transport is CLI: {rendered}"
        );
    });
}
