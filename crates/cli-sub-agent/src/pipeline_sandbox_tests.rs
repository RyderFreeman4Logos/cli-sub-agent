use std::path::PathBuf;

use super::*;

/// Helper: parse a TOML string into a ProjectConfig for test setup.
fn parse_project_config(toml_str: &str) -> csa_config::ProjectConfig {
    toml::from_str(toml_str).expect("test TOML should parse")
}

/// Heavyweight tools (claude-code) with no project config should get setting_sources=Some(vec![]).
#[test]
fn test_none_config_sets_setting_sources_for_heavyweight() {
    let result = resolve_sandbox_options(
        None,
        "claude-code",
        "test-session",
        StreamMode::BufferOnly,
        120,
        600,
        Some(120),
        false,
        false, // readonly_project_root
    );

    let SandboxResolution::Ok(opts) = result else {
        panic!("Expected SandboxResolution::Ok");
    };

    assert_eq!(
        opts.setting_sources,
        Some(vec![]),
        "Heavyweight tool should have setting_sources=Some(vec![]) (lean mode)"
    );
}

/// Lightweight tools (opencode) with no project config should NOT get a sandbox context.
#[test]
fn test_none_config_lightweight_skips_sandbox() {
    let result = resolve_sandbox_options(
        None,
        "opencode",
        "test-session",
        StreamMode::BufferOnly,
        120,
        600,
        Some(120),
        false,
        false, // readonly_project_root
    );

    let SandboxResolution::Ok(opts) = result else {
        panic!("Expected SandboxResolution::Ok");
    };

    assert_eq!(
        opts.setting_sources, None,
        "Lightweight tool should have setting_sources=None (load everything)"
    );
    assert!(
        opts.sandbox.is_none(),
        "Lightweight tool should have no sandbox context (enforcement=Off)"
    );
}

/// Heavyweight tools (claude-code) with no project config should get a sandbox context
/// when sandbox capability is available, or at least setting_sources when it is not.
#[test]
fn test_none_config_heavyweight_gets_sandbox() {
    let result = resolve_sandbox_options(
        None,
        "claude-code",
        "test-session",
        StreamMode::BufferOnly,
        120,
        600,
        Some(120),
        false,
        false, // readonly_project_root
    );

    let SandboxResolution::Ok(opts) = result else {
        panic!("Expected SandboxResolution::Ok");
    };

    // setting_sources is always set for heavyweight regardless of sandbox capability
    assert_eq!(
        opts.setting_sources,
        Some(vec![]),
        "Heavyweight tool should have setting_sources=Some(vec![])"
    );

    let capability = csa_resource::detect_resource_capability();
    if matches!(capability, csa_resource::ResourceCapability::None) {
        // On systems without sandbox capability, sandbox context is skipped
        assert!(
            opts.sandbox.is_none(),
            "No sandbox capability — should have no sandbox context"
        );
    } else {
        // On systems with sandbox capability, sandbox context should be present
        let ctx = opts
            .sandbox
            .as_ref()
            .expect("Expected SandboxContext for heavyweight tool");
        // IsolationPlan should have a resource capability set.
        assert_ne!(
            ctx.isolation_plan.resource,
            csa_resource::ResourceCapability::None,
            "Expected resource capability for heavyweight tool"
        );
        assert!(ctx.best_effort, "Profile defaults should use best-effort");
        assert_eq!(ctx.tool_name, "claude-code");
        assert_eq!(ctx.session_id, "test-session");
    }
}

// ---------------------------------------------------------------------------
// Per-tool filesystem sandbox integration tests
// ---------------------------------------------------------------------------

/// Per-tool writable_paths replaces project root: project root becomes read-only,
/// and only the specified paths are writable.
#[test]
fn test_per_tool_writable_paths_replace_project_root() {
    let cfg = parse_project_config(
        r#"
[tools.gemini-cli]
enabled = true
memory_max_mb = 2048

[tools.gemini-cli.filesystem_sandbox]
writable_paths = ["/tmp"]
"#,
    );

    let result = resolve_sandbox_options(
        Some(&cfg),
        "gemini-cli",
        "test-session",
        StreamMode::BufferOnly,
        120,
        600,
        Some(120),
        false,
        false, // readonly_project_root (not set by caller)
    );

    let SandboxResolution::Ok(opts) = result else {
        panic!("Expected SandboxResolution::Ok");
    };

    // On systems without sandbox capability, sandbox context may be absent.
    let resource_cap = csa_resource::detect_resource_capability();
    if matches!(resource_cap, csa_resource::ResourceCapability::None) {
        return; // Cannot verify IsolationPlan without resource capability
    }

    let ctx = opts
        .sandbox
        .as_ref()
        .expect("Expected SandboxContext with per-tool writable_paths");

    // Project root should be read-only because per-tool writable_paths are set.
    assert!(
        ctx.isolation_plan.readonly_project_root,
        "Per-tool writable_paths should make project root read-only"
    );

    // /tmp should be in the writable paths.
    assert!(
        ctx.isolation_plan
            .writable_paths
            .contains(&PathBuf::from("/tmp")),
        "writable_paths should contain /tmp, got: {:?}",
        ctx.isolation_plan.writable_paths
    );
}

/// readonly_project_root=true (from review/debate config) propagates to IsolationPlan.
#[test]
fn test_readonly_sandbox_from_review_config() {
    // Use a minimal config with memory_max_mb so sandbox resolution proceeds.
    let cfg = parse_project_config(
        r#"
[resources]
memory_max_mb = 2048
"#,
    );

    let result = resolve_sandbox_options(
        Some(&cfg),
        "claude-code",
        "test-session",
        StreamMode::BufferOnly,
        120,
        600,
        Some(120),
        false,
        true, // readonly_project_root (set by review/debate caller)
    );

    let SandboxResolution::Ok(opts) = result else {
        panic!("Expected SandboxResolution::Ok");
    };

    let resource_cap = csa_resource::detect_resource_capability();
    if matches!(resource_cap, csa_resource::ResourceCapability::None) {
        return;
    }

    let ctx = opts
        .sandbox
        .as_ref()
        .expect("Expected SandboxContext for heavyweight tool");

    assert!(
        ctx.isolation_plan.readonly_project_root,
        "readonly_project_root=true should propagate to IsolationPlan"
    );
}

/// Path validation rejects dangerous paths like "/" — resolve_sandbox_options
/// should return RequiredButUnavailable.
#[test]
fn test_path_validation_rejects_dangerous_paths() {
    let cfg = parse_project_config(
        r#"
[tools.gemini-cli]
enabled = true
memory_max_mb = 2048

[tools.gemini-cli.filesystem_sandbox]
writable_paths = ["/"]
"#,
    );

    let result = resolve_sandbox_options(
        Some(&cfg),
        "gemini-cli",
        "test-session",
        StreamMode::BufferOnly,
        120,
        600,
        Some(120),
        false,
        false,
    );

    assert!(
        matches!(result, SandboxResolution::RequiredButUnavailable(_)),
        "writable_paths = [\"/\"] should be rejected as dangerous, got Ok"
    );
}

/// Per-tool enforcement_mode override: tool-level enforcement overrides global.
#[test]
fn test_per_tool_enforcement_mode_override() {
    // Global filesystem_sandbox enforcement = off, but tool-level = best-effort.
    // The tool-level override should take effect.
    let cfg = parse_project_config(
        r#"
[resources]
memory_max_mb = 2048

[filesystem_sandbox]
enforcement_mode = "off"

[tools.claude-code]
enabled = true

[tools.claude-code.filesystem_sandbox]
enforcement_mode = "best-effort"
"#,
    );

    // Verify the config-level resolution returns the tool-level override.
    let fs_mode = cfg.tool_fs_enforcement_mode("claude-code");
    assert_eq!(
        fs_mode.as_deref(),
        Some("best-effort"),
        "Tool-level FS enforcement should override global 'off'"
    );
}
