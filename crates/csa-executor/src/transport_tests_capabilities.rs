// Tests for Transport trait capabilities + TransportMode serde + factory create_with_mode.

#[test]
fn test_transport_capabilities_serde_roundtrip() {
    let caps = super::TransportCapabilities {
        streaming: true,
        session_resume: true,
        session_fork: false,
        typed_events: true,
    };
    let json = serde_json::to_string(&caps).expect("serialize TransportCapabilities");
    let deserialized: super::TransportCapabilities =
        serde_json::from_str(&json).expect("deserialize TransportCapabilities");
    assert_eq!(caps, deserialized);
}

#[test]
fn test_transport_mode_serde_matches_config_keys() {
    let legacy_json = serde_json::to_string(&super::TransportMode::Legacy).unwrap();
    assert_eq!(legacy_json, r#""cli""#);

    let acp_json = serde_json::to_string(&super::TransportMode::Acp).unwrap();
    assert_eq!(acp_json, r#""acp""#);

    let openai_json = serde_json::to_string(&super::TransportMode::OpenaiCompat).unwrap();
    assert_eq!(openai_json, r#""openai_compat""#);

    // `cli` and `acp` match the existing `ToolTransport` config vocabulary in
    // `csa-config::config_tool`. `openai_compat` is Phase-2 vocabulary —
    // `ToolTransport` will widen to include it when OpenaiCompat ships in config.
    #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
    struct Wrapper {
        mode: super::TransportMode,
    }
    for mode in [
        super::TransportMode::Legacy,
        super::TransportMode::Acp,
        super::TransportMode::OpenaiCompat,
    ] {
        let w = Wrapper { mode };
        let toml_str = toml::to_string(&w).expect("serialize to TOML");
        let rt: Wrapper = toml::from_str(&toml_str).expect("deserialize from TOML");
        assert_eq!(w, rt, "TOML round-trip failed for {mode:?}");
    }
}

#[test]
fn test_create_with_mode_explicit_override() {
    // create_with_mode validates the (executor, mode) combination before instantiation.
    let executor = crate::executor::Executor::Codex {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: crate::codex_runtime::codex_runtime_metadata(),
    };

    // Legacy mode succeeds for codex.
    let result =
        super::TransportFactory::create_with_mode(&executor, super::TransportMode::Legacy, None);
    assert!(result.is_ok(), "Legacy mode should succeed for codex executor");

    // Verify the created transport has the correct mode.
    let transport = result.unwrap();
    assert_eq!(transport.mode(), super::TransportMode::Legacy);
}

#[test]
fn test_create_with_mode_allows_codex_acp() {
    let executor = crate::executor::Executor::Codex {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: crate::codex_runtime::codex_runtime_metadata(),
    };

    let result =
        super::TransportFactory::create_with_mode(&executor, super::TransportMode::Acp, None);
    assert!(result.is_ok(), "codex ACP transport should be supported");
}

// ---------------------------------------------------------------------------
// Full executor × mode validation matrix tests
// ---------------------------------------------------------------------------
//
// Compatibility matrix:
//
// | Executor     | Legacy | Acp                       | OpenaiCompat |
// |--------------|--------|---------------------------| -------------|
// | ClaudeCode   | Yes    | Yes                       | No           |
// | Codex        | Yes    | Yes (feature `codex-acp`) | No           |
// | GeminiCli    | Yes    | Yes                       | No           |
// | Opencode     | Yes    | No                        | No           |
// | OpenaiCompat | No     | No                        | Yes          |

fn make_claude_code() -> crate::executor::Executor {
    crate::executor::Executor::ClaudeCode {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: crate::claude_runtime::claude_runtime_metadata(),
    }
}

fn make_codex() -> crate::executor::Executor {
    crate::executor::Executor::Codex {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: crate::codex_runtime::codex_runtime_metadata(),
    }
}

fn make_gemini_cli() -> crate::executor::Executor {
    crate::executor::Executor::GeminiCli {
        model_override: None,
        thinking_budget: None,
    }
}

fn make_opencode() -> crate::executor::Executor {
    crate::executor::Executor::Opencode {
        model_override: None,
        agent: None,
        thinking_budget: None,
    }
}

fn make_openai_compat() -> crate::executor::Executor {
    crate::executor::Executor::OpenaiCompat {
        model_override: None,
        thinking_budget: None,
    }
}

/// Assert that `create_with_mode` returns `Err(TransportFactoryError::UnsupportedTransport)`
/// with matching executor and requested mode fields.
fn assert_unsupported(
    executor: &crate::executor::Executor,
    mode: super::TransportMode,
    expected_reason_substr: &str,
) {
    let result = super::TransportFactory::create_with_mode(executor, mode, None);
    match result {
        Ok(_) => panic!(
            "expected UnsupportedTransport for {:?} + {:?}, got Ok",
            executor.tool_name(),
            mode,
        ),
        Err(err) => {
            let factory_err = err
                .downcast_ref::<super::TransportFactoryError>()
                .unwrap_or_else(|| {
                    panic!(
                        "expected TransportFactoryError for {:?} + {:?}, got: {err:#}",
                        executor.tool_name(),
                        mode,
                    )
                });
            match factory_err {
                super::TransportFactoryError::UnsupportedTransport {
                    requested,
                    executor: exec_name,
                    reason,
                } => {
                    assert_eq!(*requested, mode);
                    assert_eq!(exec_name, executor.tool_name());
                    assert!(
                        reason.contains(expected_reason_substr),
                        "reason {reason:?} should contain {expected_reason_substr:?}"
                    );
                }
            }
        }
    }
}

fn assert_supported(executor: &crate::executor::Executor, mode: super::TransportMode) {
    let result = super::TransportFactory::create_with_mode(executor, mode, None);
    match result {
        Err(err) => panic!(
            "expected Ok for {:?} + {:?}, got: {err:#}",
            executor.tool_name(),
            mode,
        ),
        Ok(transport) => assert_eq!(transport.mode(), mode),
    }
}

// --- ClaudeCode ---

// ACP for claude-code requires the `claude-code-acp` cargo feature (default OFF,
// gated due to startup-crash bugs #1115/#1117).
#[test]
#[cfg(feature = "claude-code-acp")]
fn test_matrix_claude_code_acp_supported() {
    assert_supported(&make_claude_code(), super::TransportMode::Acp);
}

#[test]
#[cfg(not(feature = "claude-code-acp"))]
fn test_matrix_claude_code_acp_rejected_without_feature() {
    assert_unsupported(
        &make_claude_code(),
        super::TransportMode::Acp,
        "claude-code-acp",
    );
}

#[test]
fn test_matrix_claude_code_legacy_supported() {
    assert_supported(&make_claude_code(), super::TransportMode::Legacy);
}

#[test]
fn test_matrix_claude_code_openai_compat_rejected() {
    assert_unsupported(
        &make_claude_code(),
        super::TransportMode::OpenaiCompat,
        "only supports cli or acp",
    );
}

// --- Codex ---

#[test]
fn test_matrix_codex_legacy_supported() {
    assert_supported(&make_codex(), super::TransportMode::Legacy);
}

#[test]
fn test_matrix_codex_acp_supported() {
    assert_supported(&make_codex(), super::TransportMode::Acp);
}

#[test]
fn test_matrix_codex_openai_compat_rejected() {
    assert_unsupported(
        &make_codex(),
        super::TransportMode::OpenaiCompat,
        "only supports cli or acp",
    );
}

// --- GeminiCli ---

#[test]
fn test_matrix_gemini_cli_legacy_supported() {
    assert_supported(&make_gemini_cli(), super::TransportMode::Legacy);
}

#[test]
fn test_matrix_gemini_cli_acp_supported() {
    assert_supported(&make_gemini_cli(), super::TransportMode::Acp);
}

#[test]
fn test_matrix_gemini_cli_openai_compat_rejected() {
    assert_unsupported(
        &make_gemini_cli(),
        super::TransportMode::OpenaiCompat,
        "only supports cli or acp",
    );
}

// --- Opencode ---

#[test]
fn test_matrix_opencode_legacy_supported() {
    assert_supported(&make_opencode(), super::TransportMode::Legacy);
}

#[test]
fn test_matrix_opencode_acp_rejected() {
    assert_unsupported(
        &make_opencode(),
        super::TransportMode::Acp,
        "no acp transport",
    );
}

#[test]
fn test_matrix_opencode_openai_compat_rejected() {
    assert_unsupported(
        &make_opencode(),
        super::TransportMode::OpenaiCompat,
        "only supports cli",
    );
}

// --- OpenaiCompat ---

#[test]
fn test_matrix_openai_compat_openai_compat_supported() {
    assert_supported(&make_openai_compat(), super::TransportMode::OpenaiCompat);
}

#[test]
fn test_matrix_openai_compat_legacy_rejected() {
    assert_unsupported(
        &make_openai_compat(),
        super::TransportMode::Legacy,
        "no cli binary",
    );
}

#[test]
fn test_matrix_openai_compat_acp_rejected() {
    assert_unsupported(
        &make_openai_compat(),
        super::TransportMode::Acp,
        "does not support acp",
    );
}

#[test]
fn test_create_feature_gate_structured_error() {
    let executor = crate::executor::Executor::Codex {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: crate::codex_runtime::CodexRuntimeMetadata::from_transport(
            crate::codex_runtime::CodexTransport::Acp,
        ),
    };

    let result = super::TransportFactory::create(&executor, None);
    assert!(result.is_ok(), "codex ACP transport should build in auto mode");
}

#[test]
fn test_each_impl_capabilities_matches_spec() {
    // Legacy
    let legacy = super::LegacyTransport::new(crate::executor::Executor::Codex {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: crate::codex_runtime::codex_runtime_metadata(),
    });
    let legacy_caps = <super::LegacyTransport as super::Transport>::capabilities(&legacy);
    assert_eq!(
        legacy_caps,
        super::TransportCapabilities {
            streaming: false,
            session_resume: true,
            session_fork: false,
            typed_events: false,
        }
    );

    let legacy_claude_cli = super::LegacyTransport::new(crate::executor::Executor::ClaudeCode {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: crate::claude_runtime::ClaudeCodeRuntimeMetadata::from_transport(
            crate::claude_runtime::ClaudeCodeTransport::Cli,
        ),
    });
    let legacy_claude_cli_caps =
        <super::LegacyTransport as super::Transport>::capabilities(&legacy_claude_cli);
    assert_eq!(
        legacy_claude_cli_caps,
        super::TransportCapabilities {
            streaming: false,
            session_resume: false,
            session_fork: false,
            typed_events: false,
        }
    );

    // ACP with claude-code: session_fork is true (native fork supported).
    let acp_claude = super::AcpTransport::new("claude-code", None);
    let acp_claude_caps =
        <super::AcpTransport as super::Transport>::capabilities(&acp_claude);
    assert_eq!(
        acp_claude_caps,
        super::TransportCapabilities {
            streaming: true,
            session_resume: true,
            session_fork: true,
            typed_events: true,
        }
    );

    // ACP with gemini-cli: session_fork is false (only soft fork).
    let acp_gemini = super::AcpTransport::new("gemini-cli", None);
    let acp_gemini_caps =
        <super::AcpTransport as super::Transport>::capabilities(&acp_gemini);
    assert_eq!(
        acp_gemini_caps,
        super::TransportCapabilities {
            streaming: true,
            session_resume: true,
            session_fork: false,
            typed_events: true,
        }
    );

    // ACP with codex: session_fork depends on codex-pty-fork feature.
    let acp_codex = super::AcpTransport::new("codex", None);
    let acp_codex_caps =
        <super::AcpTransport as super::Transport>::capabilities(&acp_codex);
    #[cfg(feature = "codex-pty-fork")]
    assert!(acp_codex_caps.session_fork, "codex should have native fork with codex-pty-fork");
    #[cfg(not(feature = "codex-pty-fork"))]
    assert!(!acp_codex_caps.session_fork, "codex should not have native fork without codex-pty-fork");

    // OpenaiCompat
    let openai = crate::transport_openai_compat::OpenaiCompatTransport::new(None);
    let openai_caps = <crate::transport_openai_compat::OpenaiCompatTransport as super::Transport>::capabilities(&openai);
    assert_eq!(
        openai_caps,
        super::TransportCapabilities {
            streaming: false,
            session_resume: false,
            session_fork: false,
            typed_events: false,
        }
    );

    // TransportMode::capabilities() returns conservative defaults.
    // ACP mode-level returns session_fork: false because the actual value
    // is tool-dependent; concrete AcpTransport instances are authoritative.
    assert_eq!(super::TransportMode::Legacy.capabilities(), legacy_caps);
    assert_eq!(
        super::TransportMode::Acp.capabilities(),
        super::TransportCapabilities {
            streaming: true,
            session_resume: true,
            session_fork: false,
            typed_events: true,
        }
    );
    assert_eq!(super::TransportMode::OpenaiCompat.capabilities(), openai_caps);
}
