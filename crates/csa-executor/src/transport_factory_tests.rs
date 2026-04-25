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

/// When `claude-code-acp` feature is OFF, requesting ACP transport must fail with
/// a clear error citing the feature flag and the issue numbers (#1115/#1117).
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
        msg.contains("claude-code-acp") || msg.contains("1115"),
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
// Codex ACP path unaffected by claude-code-acp feature
// ---------------------------------------------------------------------------

/// Codex with explicit `Acp` transport must still produce ACP mode even when the
/// `claude-code-acp` feature is OFF. The feature gate is scoped to ClaudeCode only.
#[test]
fn test_codex_acp_unchanged() {
    let executor = make_codex_executor_acp();
    let mode = TransportFactory::mode_for_executor_pub(&executor)
        .expect("codex ACP should always be available regardless of claude-code-acp feature");
    assert_eq!(
        mode,
        TransportMode::Acp,
        "codex ACP must not be gated by the claude-code-acp feature"
    );
}
