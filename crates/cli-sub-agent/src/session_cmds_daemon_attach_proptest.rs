use super::*;

// ── Attach routing property-based coverage (#762) ───────────────────────────
//
// Generates the cartesian product of
//   runtime_binary in { None, codex, codex-acp, gemini, gemini-acp }
//   tool in { codex, gemini-cli, claude-code, opencode }
//   output_log_exists: bool
//   session_active: bool
// and asserts documented invariants of `attach_primary_output_from_metadata`.
// Each property runs at least 256 iterations; proptest persists shrunk
// counterexamples under `crates/cli-sub-agent/proptest-regressions/`.

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(256))]

    #[test]
    fn prop_attach_routing_codex_acp_always_output_log(
        runtime_binary in attach_runtime_binary_strategy(),
        tool in attach_tool_strategy(),
        output_log_exists in proptest::prelude::any::<bool>(),
        session_active in proptest::prelude::any::<bool>(),
    ) {
        let result = attach_route_for(tool, runtime_binary, output_log_exists, session_active);
        if tool == "codex" && runtime_binary == Some("codex-acp") {
            proptest::prop_assert_eq!(
                result,
                AttachPrimaryOutput::OutputLog,
                "codex-acp runtime must always attach to output.log"
            );
        }
    }

    #[test]
    fn prop_attach_routing_claude_code_always_output_log(
        runtime_binary in attach_runtime_binary_strategy(),
        output_log_exists in proptest::prelude::any::<bool>(),
        session_active in proptest::prelude::any::<bool>(),
    ) {
        let result = attach_route_for("claude-code", runtime_binary, output_log_exists, session_active);
        let expected = if runtime_binary.is_none() {
            if output_log_exists {
                AttachPrimaryOutput::OutputLog
            } else {
                AttachPrimaryOutput::StdoutLog
            }
        } else {
            AttachPrimaryOutput::OutputLog
        };
        proptest::prop_assert_eq!(result, expected);
    }

    #[test]
    fn prop_attach_routing_codex_legacy_active_session_output_log(
        output_log_exists in proptest::prelude::any::<bool>(),
    ) {
        // Pre-upgrade sessions without a persisted runtime binary should route
        // by on-disk transcript presence, regardless of liveness.
        let result = attach_route_for("codex", None, output_log_exists, true);
        let expected = if output_log_exists {
            AttachPrimaryOutput::OutputLog
        } else {
            AttachPrimaryOutput::StdoutLog
        };
        proptest::prop_assert_eq!(result, expected);
    }

    #[test]
    fn prop_attach_routing_codex_legacy_terminal_follows_output_log(
        output_log_exists in proptest::prelude::any::<bool>(),
    ) {
        // tool=codex + runtime_binary=None + !session_active ⇒
        //   OutputLog when output.log exists, StdoutLog otherwise.
        let result = attach_route_for("codex", None, output_log_exists, false);
        let expected = if output_log_exists {
            AttachPrimaryOutput::OutputLog
        } else {
            AttachPrimaryOutput::StdoutLog
        };
        proptest::prop_assert_eq!(
            result,
            expected,
            "terminated codex legacy session must follow output.log presence"
        );
    }

    #[test]
    fn prop_attach_routing_legacy_runtime_binary_missing_follows_output_log_presence(
        tool in proptest::sample::select(vec!["codex", "gemini-cli", "opencode"]),
        output_log_exists in proptest::prelude::any::<bool>(),
        session_active in proptest::prelude::any::<bool>(),
    ) {
        let result = attach_route_for(tool, None, output_log_exists, session_active);
        let expected = if output_log_exists {
            AttachPrimaryOutput::OutputLog
        } else {
            AttachPrimaryOutput::StdoutLog
        };
        proptest::prop_assert_eq!(result, expected);
    }

    #[test]
    fn prop_attach_routing_legacy_runtime_binary_present_uses_transport_defaults(
        tool in proptest::sample::select(vec!["gemini-cli", "opencode"]),
        output_log_exists in proptest::prelude::any::<bool>(),
        session_active in proptest::prelude::any::<bool>(),
    ) {
        let runtime_binary = if tool == "gemini-cli" {
            Some("gemini")
        } else {
            Some("opencode")
        };
        let result = attach_route_for(tool, runtime_binary, output_log_exists, session_active);
        proptest::prop_assert_eq!(result, AttachPrimaryOutput::StdoutLog);
    }

    #[test]
    fn prop_attach_routing_never_returns_await_metadata(
        runtime_binary in attach_runtime_binary_strategy(),
        tool in attach_tool_strategy(),
        output_log_exists in proptest::prelude::any::<bool>(),
        session_active in proptest::prelude::any::<bool>(),
    ) {
        // attach_primary_output_from_metadata must resolve to a concrete log —
        // AwaitMetadata is only produced by the higher-level fallback path.
        let result = attach_route_for(tool, runtime_binary, output_log_exists, session_active);
        proptest::prop_assert_ne!(result, AttachPrimaryOutput::AwaitMetadata);
    }
}

fn attach_route_for(
    tool: &str,
    runtime_binary: Option<&str>,
    output_log_exists: bool,
    session_active: bool,
) -> AttachPrimaryOutput {
    let metadata = csa_session::metadata::SessionMetadata {
        tool: tool.to_string(),
        tool_locked: true,
        runtime_binary: runtime_binary.map(std::string::ToString::to_string),
    };
    attach_primary_output_from_metadata(&metadata, output_log_exists, session_active)
}

fn attach_runtime_binary_strategy() -> impl proptest::prelude::Strategy<Value = Option<&'static str>>
{
    proptest::prelude::prop_oneof![
        proptest::prelude::Just(None),
        proptest::prelude::Just(Some("codex")),
        proptest::prelude::Just(Some("codex-acp")),
        proptest::prelude::Just(Some("gemini")),
        proptest::prelude::Just(Some("gemini-acp")),
    ]
}

fn attach_tool_strategy() -> impl proptest::prelude::Strategy<Value = &'static str> {
    proptest::prelude::prop_oneof![
        proptest::prelude::Just("codex"),
        proptest::prelude::Just("gemini-cli"),
        proptest::prelude::Just("claude-code"),
        proptest::prelude::Just("opencode"),
    ]
}
