#[test]
fn test_execute_in_timeout_resolver_defaults_codex_to_300_seconds() {
    let codex_executor = Executor::Codex {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: crate::codex_runtime::codex_runtime_metadata(),
    };
    let gemini_executor = Executor::GeminiCli {
        model_override: None,
        thinking_budget: None,
    };

    assert_eq!(
        super::resolve_execute_in_initial_response_timeout_seconds(&codex_executor, None),
        super::ResolvedTimeout(Some(300)),
        "direct execute_in codex calls should inherit the 300s watchdog by default"
    );
    assert_eq!(
        super::resolve_execute_in_initial_response_timeout_seconds(&codex_executor, Some(0)),
        super::ResolvedTimeout(None),
        "explicit 0 should disable the watchdog for direct codex execute_in calls"
    );
    assert_eq!(
        super::resolve_execute_in_initial_response_timeout_seconds(&codex_executor, Some(42)),
        super::ResolvedTimeout(Some(42)),
        "explicit execute_in overrides should win over the codex default"
    );
    assert_eq!(
        super::resolve_execute_in_initial_response_timeout_seconds(&codex_executor, Some(450)),
        super::ResolvedTimeout(Some(450)),
        "positive execute_in overrides should pass through unchanged for codex"
    );
    assert_eq!(
        super::resolve_execute_in_initial_response_timeout_seconds(&gemini_executor, None),
        super::ResolvedTimeout(Some(120)),
        "non-codex direct execute_in calls should inherit the generic 120s watchdog by default"
    );
    assert_eq!(
        super::resolve_execute_in_initial_response_timeout_seconds(&gemini_executor, Some(0)),
        super::ResolvedTimeout(None),
        "explicit 0 should disable the watchdog for direct non-codex execute_in calls"
    );
    assert_eq!(
        super::resolve_execute_in_initial_response_timeout_seconds(&gemini_executor, Some(450)),
        super::ResolvedTimeout(Some(450)),
        "positive execute_in overrides should pass through unchanged for non-codex"
    );
}

#[test]
fn test_legacy_execute_in_consumes_resolved_timeout_without_reapplying_defaults() {
    let codex_executor = Executor::Codex {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: crate::codex_runtime::codex_runtime_metadata(),
    };

    let disabled = super::consume_resolved_execute_in_initial_response_timeout_seconds(
        super::resolve_execute_in_initial_response_timeout_seconds(&codex_executor, Some(0)),
    );
    assert_eq!(
        disabled, None,
        "Executor::execute_in(Some(0)) must stay disabled through the legacy codex path"
    );

    let defaulted = super::consume_resolved_execute_in_initial_response_timeout_seconds(
        super::resolve_execute_in_initial_response_timeout_seconds(&codex_executor, None),
    );
    assert_eq!(
        defaulted,
        Some(300),
        "Executor::execute_in(None) must arm the codex legacy watchdog at the 300s default"
    );

    let explicit = super::consume_resolved_execute_in_initial_response_timeout_seconds(
        super::resolve_execute_in_initial_response_timeout_seconds(&codex_executor, Some(450)),
    );
    assert_eq!(
        explicit,
        Some(450),
        "Executor::execute_in(Some(450)) must preserve the explicit codex legacy watchdog"
    );

    assert_eq!(
        super::consume_resolved_execute_in_initial_response_timeout_seconds(
            super::ResolvedTimeout(Some(0)),
        ),
        None,
        "legacy consumers should defensively collapse stray Some(0) to disabled"
    );
}

#[tokio::test]
async fn test_executor_execute_in_retries_codex_stall_with_direct_entry() {
    use std::collections::HashMap;
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().expect("tempdir");
    let script_path = temp.path().join("codex");
    let model_log_path = temp.path().join("codex-model.log");
    std::fs::write(
        &script_path,
        r#"#!/usr/bin/env bash
set -euo pipefail

effort="inherit"
for arg in "$@"; do
  case "$arg" in
    model_reasoning_effort=*)
      effort="${arg#model_reasoning_effort=}"
      ;;
  esac
done

echo "$effort" >> "$MODEL_LOG"

if [ "$effort" = "xhigh" ]; then
  sleep 5
  exit 0
fi

echo "ok effort=$effort"
"#,
    )
    .expect("write fake codex");
    let mut perms = std::fs::metadata(&script_path)
        .expect("metadata")
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script_path, perms).expect("chmod +x");

    let old_path = std::env::var("PATH").unwrap_or_default();
    let env = HashMap::from([
        (
            "PATH".to_string(),
            format!("{}:{old_path}", temp.path().display()),
        ),
        (
            "MODEL_LOG".to_string(),
            model_log_path.display().to_string(),
        ),
    ]);
    let executor = Executor::Codex {
        model_override: None,
        thinking_budget: Some(crate::model_spec::ThinkingBudget::Xhigh),
        runtime_metadata: crate::codex_runtime::CodexRuntimeMetadata::from_transport(
            crate::codex_runtime::CodexTransport::Cli,
        ),
    };

    let result = executor
        .execute_in(
            "trigger direct execute_in codex stall retry",
            temp.path(),
            Some(&env),
            StreamMode::BufferOnly,
            30,
            super::ResolvedTimeout(Some(1)),
        )
        .await
        .expect("execute_in should retry the codex stall and succeed");

    assert_eq!(result.exit_code, 0);
    assert!(
        result.output.contains("ok effort=high"),
        "expected downgraded retry output, got: {}",
        result.output
    );
    let attempts = std::fs::read_to_string(&model_log_path)
        .expect("read model log")
        .lines()
        .map(str::to_string)
        .collect::<Vec<_>>();
    assert_eq!(
        attempts,
        vec!["xhigh".to_string(), "high".to_string()],
        "Executor::execute_in should trigger codex stall retry via the direct entry"
    );
}

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
