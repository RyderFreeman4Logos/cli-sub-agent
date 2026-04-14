use super::*;
use crate::test_env_lock::ScopedTestEnvVar;
#[cfg(not(feature = "codex-acp"))]
use csa_config::{ProjectMeta, ResourcesConfig, ToolConfig, ToolTransport};
#[cfg(not(feature = "codex-acp"))]
use serde_json::Value;
#[cfg(not(feature = "codex-acp"))]
use std::collections::HashMap;

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
