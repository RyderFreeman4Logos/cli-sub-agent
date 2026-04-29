//! Unit tests for [`super::TransportFactory`] mode selection and feature-gating.
//!
//! Extracted into a sibling file (referenced via `#[cfg(test)] #[path =
//! "transport_factory_tests.rs"] mod tests` in `transport_factory.rs`) to keep the
//! source file well under the 800-line monolith guard while following the
//! `transport_cli_tests.rs` pattern already established in this crate.

use super::*;
use crate::claude_runtime::{ClaudeCodeRuntimeMetadata, ClaudeCodeTransport};
use crate::codex_runtime::{CodexRuntimeMetadata, CodexTransport};
use crate::executor::Executor;

fn make_claude_executor_cli() -> Executor {
    Executor::ClaudeCode {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: ClaudeCodeRuntimeMetadata::from_transport(ClaudeCodeTransport::Cli),
    }
}

fn make_claude_executor_acp() -> Executor {
    Executor::ClaudeCode {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: ClaudeCodeRuntimeMetadata::from_transport(ClaudeCodeTransport::Acp),
    }
}

fn make_codex_executor_cli() -> Executor {
    Executor::Codex {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: CodexRuntimeMetadata::from_transport(CodexTransport::Cli),
    }
}

fn make_codex_executor_acp() -> Executor {
    Executor::Codex {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: CodexRuntimeMetadata::from_transport(CodexTransport::Acp),
    }
}

// ---------------------------------------------------------------------------
// Change 1: default transport flip — explicit Cli routes to Legacy
// ---------------------------------------------------------------------------

/// `claude-code` with an explicit `Cli` transport must route to Legacy.
#[test]
fn test_mode_for_executor_claude_code_explicit_cli_is_legacy() {
    let executor = make_claude_executor_cli();
    let mode = TransportFactory::mode_for_executor_pub(&executor)
        .expect("mode_for_executor should succeed for ClaudeCode+Cli");
    assert_eq!(
        mode,
        TransportMode::Legacy,
        "explicit Cli should resolve to Legacy transport"
    );
}

/// `claude-code` with `None` transport (default) must also resolve to Legacy.
///
/// `claude_code_transport()` always returns `Some(...)` from the metadata, so the
/// `None` arm in the pattern is purely defensive. This test exercises the explicit
/// `Some(Cli)` path which is the effective default after the flip.
#[test]
fn test_mode_for_executor_claude_code_default_is_legacy() {
    // Build with Cli metadata to represent the "user did not request ACP" case.
    let executor = make_claude_executor_cli();
    let mode = TransportFactory::mode_for_executor_pub(&executor)
        .expect("default Cli metadata should resolve to Legacy");
    assert_eq!(
        mode,
        TransportMode::Legacy,
        "default Cli metadata must route to Legacy, not ACP"
    );
}

// ---------------------------------------------------------------------------
// Change 3: feature-gate — ACP without feature => error
// ---------------------------------------------------------------------------

/// When the ACP feature stack is OFF, requesting ACP transport must fail with
/// a clear error citing the generic or tool-specific feature gate.
#[test]
#[cfg(not(feature = "claude-code-acp"))]
fn test_validate_or_instantiate_claude_code_acp_errors_without_feature() {
    let executor = make_claude_executor_acp();

    // validate_mode_for_executor should reject it
    let result = TransportFactory::validate_mode_for_executor_pub(&executor, TransportMode::Acp);
    assert!(
        result.is_err(),
        "validate should fail for ClaudeCode+Acp without feature"
    );
    let msg = format!("{}", result.unwrap_err());
    assert!(
        msg.contains("`acp` cargo feature")
            || msg.contains("claude-code-acp")
            || msg.contains("1115"),
        "error should mention the feature flag or issue number; got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// Change 3: feature-gate — ACP with feature => succeeds
// ---------------------------------------------------------------------------

/// With `claude-code-acp` feature ON, explicit ACP executor must route to Acp.
#[test]
#[cfg(feature = "claude-code-acp")]
fn test_claude_code_acp_works_with_feature() {
    let executor = make_claude_executor_acp();
    let mode = TransportFactory::mode_for_executor_pub(&executor)
        .expect("mode_for_executor should succeed with claude-code-acp feature enabled");
    assert_eq!(
        mode,
        TransportMode::Acp,
        "explicit Acp should resolve to Acp when feature ON"
    );
}

// ---------------------------------------------------------------------------
// Codex: default transport flip (mirrors claude-code) — explicit Cli routes to Legacy
// ---------------------------------------------------------------------------

/// `codex` with an explicit `Cli` transport must route to Legacy.
#[test]
fn test_mode_for_executor_codex_explicit_cli_is_legacy() {
    let executor = make_codex_executor_cli();
    let mode = TransportFactory::mode_for_executor_pub(&executor)
        .expect("mode_for_executor should succeed for Codex+Cli");
    assert_eq!(
        mode,
        TransportMode::Legacy,
        "explicit Cli should resolve to Legacy transport"
    );
}

/// `codex` default (no explicit ACP) must resolve to Legacy.
///
/// `codex_transport()` always returns `Some(...)` from the metadata, so the
/// `None` arm in the pattern is purely defensive. This test exercises the explicit
/// `Some(Cli)` path which is the effective default after the flip.
#[test]
fn test_mode_for_executor_codex_default_is_legacy() {
    let executor = make_codex_executor_cli();
    let mode = TransportFactory::mode_for_executor_pub(&executor)
        .expect("default Cli metadata should resolve to Legacy");
    assert_eq!(
        mode,
        TransportMode::Legacy,
        "default Cli metadata must route to Legacy, not ACP"
    );
}

// ---------------------------------------------------------------------------
// Codex: feature-gate — ACP without feature => error
// ---------------------------------------------------------------------------

/// When the ACP feature stack is OFF, requesting ACP transport must fail with a
/// clear error citing the generic or tool-specific feature gate.
#[test]
#[cfg(not(feature = "codex-acp"))]
fn test_validate_or_instantiate_codex_acp_errors_without_feature() {
    let executor = make_codex_executor_acp();

    let result = TransportFactory::validate_mode_for_executor_pub(&executor, TransportMode::Acp);
    assert!(
        result.is_err(),
        "validate should fail for Codex+Acp without feature"
    );
    let msg = format!("{}", result.unwrap_err());
    assert!(
        msg.contains("`acp` cargo feature")
            || msg.contains("codex-acp")
            || msg.contains("760")
            || msg.contains("1128"),
        "error should mention the feature flag or issue number; got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// Codex: feature-gate — ACP with feature => succeeds
// ---------------------------------------------------------------------------

/// With `codex-acp` feature ON, explicit ACP executor must route to Acp.
#[test]
#[cfg(feature = "codex-acp")]
fn test_codex_acp_works_with_feature() {
    let executor = make_codex_executor_acp();
    let mode = TransportFactory::mode_for_executor_pub(&executor)
        .expect("mode_for_executor should succeed with codex-acp feature enabled");
    assert_eq!(
        mode,
        TransportMode::Acp,
        "explicit Acp should resolve to Acp when feature ON"
    );
}

// ---------------------------------------------------------------------------
// Cross-feature isolation: claude-code feature gate does not affect codex
// (and vice versa). Both gates target their own tool only.
// ---------------------------------------------------------------------------

/// gemini-cli ACP path is available when the generic ACP crate feature is enabled.
#[test]
#[cfg(feature = "acp")]
fn test_gemini_cli_acp_unaffected_by_feature_gates() {
    let executor = Executor::GeminiCli {
        model_override: None,
        thinking_budget: None,
    };
    let result = TransportFactory::validate_mode_for_executor_pub(&executor, TransportMode::Acp);
    assert!(
        result.is_ok(),
        "gemini-cli ACP must not be gated by either feature flag"
    );
}

/// Without the generic ACP crate feature, every ACP transport mode is disabled.
#[test]
#[cfg(not(feature = "acp"))]
fn test_gemini_cli_acp_requires_acp_feature() {
    let executor = Executor::GeminiCli {
        model_override: None,
        thinking_budget: None,
    };
    let result = TransportFactory::validate_mode_for_executor_pub(&executor, TransportMode::Acp);
    assert!(
        result.is_err(),
        "gemini-cli ACP must be gated by the generic acp feature"
    );
    let msg = format!("{}", result.unwrap_err());
    assert!(
        msg.contains("`acp` cargo feature"),
        "error should mention the generic acp feature; got: {msg}"
    );
}
