use super::*;

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
