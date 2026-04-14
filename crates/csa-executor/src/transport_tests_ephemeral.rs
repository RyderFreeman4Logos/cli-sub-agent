#[test]
fn test_executor_ephemeral_transport_honors_codex_cli_runtime_transport_override() {
    let cli_executor = Executor::Codex {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: crate::codex_runtime::CodexRuntimeMetadata::from_transport(
            crate::codex_runtime::CodexTransport::Cli,
        ),
    };
    let cli_transport = cli_executor
        .transport(None)
        .expect("ephemeral transport should build");
    assert!(
        cli_transport.as_ref().as_any().is::<LegacyTransport>(),
        "codex cli override should keep ephemeral paths on LegacyTransport"
    );
}

#[cfg(feature = "codex-acp")]
#[test]
fn test_executor_ephemeral_transport_honors_codex_acp_runtime_transport_override() {
    let acp_executor = Executor::Codex {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: crate::codex_runtime::CodexRuntimeMetadata::from_transport(
            crate::codex_runtime::CodexTransport::Acp,
        ),
    };
    let acp_transport = acp_executor
        .transport(None)
        .expect("ephemeral transport should build");
    assert!(
        acp_transport.as_ref().as_any().is::<AcpTransport>(),
        "codex acp override should keep ephemeral paths on AcpTransport"
    );
}

#[cfg(not(feature = "codex-acp"))]
#[test]
fn test_executor_ephemeral_transport_rejects_codex_acp_runtime_transport_override() {
    let acp_executor = Executor::Codex {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: crate::codex_runtime::CodexRuntimeMetadata::from_transport(
            crate::codex_runtime::CodexTransport::Acp,
        ),
    };

    let err = acp_executor
        .transport(None)
        .err()
        .expect("ephemeral codex ACP must fail closed when feature is disabled");
    let rendered = format!("{err:#}");
    assert!(
        rendered.contains("codex-acp"),
        "error should mention the required cargo feature: {rendered}"
    );
    assert!(
        rendered.contains("cargo build --features codex-acp"),
        "error should include a rebuild hint: {rendered}"
    );
}
