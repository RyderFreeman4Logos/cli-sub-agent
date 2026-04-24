// --- Retryable crash scenarios ---

#[test]
fn test_server_shut_down_unexpectedly_is_retryable() {
    let err = "ACP prompt failed: server shut down unexpectedly";
    assert!(is_retryable_acp_crash(err));
}

#[test]
fn test_internal_error_is_retryable() {
    let err = "ACP transport failed: ACP prompt failed: Internal error";
    assert!(is_retryable_acp_crash(err));
}

#[test]
fn test_process_exited_non_oom_is_retryable() {
    // SIGSEGV (signal 11) — not in OOM_SIGNALS, so retryable
    let err = "ACP process exited unexpectedly: killed by signal 11 (SIGSEGV)";
    assert!(is_retryable_acp_crash(err));
}

#[test]
fn test_broken_pipe_is_retryable() {
    let err = "ACP transport failed: Broken pipe";
    assert!(is_retryable_acp_crash(err));
}

#[test]
fn test_connection_reset_is_retryable() {
    let err = "sandboxed ACP: ACP prompt failed: connection reset by peer";
    assert!(is_retryable_acp_crash(err));
}

#[test]
fn test_prompt_failed_with_shut_down_is_retryable() {
    let err = "ACP prompt failed: the server shut down while processing";
    assert!(is_retryable_acp_crash(err));
}

// --- Non-retryable: OOM ---

#[test]
fn test_signal_9_oom_is_not_retryable() {
    let err = "ACP process exited unexpectedly: killed by signal 9 (SIGKILL)";
    assert!(!is_retryable_acp_crash(err));
}

#[test]
fn test_oom_detected_is_not_retryable() {
    let err = "sandboxed ACP: ACP process exited unexpectedly: code 137; \
               stderr:\nOOM detected: cgroup memory.max exceeded";
    assert!(!is_retryable_acp_crash(err));
}

#[test]
fn test_out_of_memory_is_not_retryable() {
    let err = "ACP transport failed: out of memory";
    assert!(!is_retryable_acp_crash(err));
}

#[test]
fn test_oom_word_boundary_cases() {
    assert!(is_oom_error("ACP transport failed: out of memory"));
    assert!(is_oom_error("kernel logged: oom killed process X"));
    assert!(!is_oom_error("room available"));
    assert!(!is_oom_error("zoom level"));
}

#[test]
fn test_signal_90_is_not_oom_false_positive() {
    // "signal 90" should NOT match the OOM pattern for signal 9.
    let err = "ACP process exited unexpectedly: killed by signal 90";
    assert!(!is_oom_error(err));
    // It should be retryable since it's an unexpected exit, not OOM.
    assert!(is_retryable_acp_crash(err));
}

#[test]
fn test_memory_max_exceeded_is_oom() {
    let err = "cgroup memory.max exceeded, process killed";
    assert!(is_oom_error(err));
    assert!(!is_retryable_acp_crash(err));
}

// --- Non-retryable: Config/Spawn ---

#[test]
fn test_config_error_is_not_retryable() {
    let err = "Configuration error: missing API key";
    assert!(!is_retryable_acp_crash(err));
}

#[test]
fn test_spawn_failed_is_not_retryable() {
    let err = "ACP subprocess spawn failed: No such file or directory";
    assert!(!is_retryable_acp_crash(err));
}

#[test]
fn test_binary_not_found_is_not_retryable() {
    let err = "ACP subprocess spawn failed: binary not found";
    assert!(!is_retryable_acp_crash(err));
}

#[test]
fn test_unauthorized_missing_scope_is_not_retryable() {
    let err = "ACP prompt failed: Internal error: unexpected status 401 Unauthorized: \
               You have insufficient permissions for this operation. \
               Missing scopes: api.responses.write";
    assert!(is_auth_error(err));
    assert!(!is_retryable_acp_crash(err));
}

#[test]
fn test_missing_scope_marker_is_not_retryable() {
    let err = "codex responses error: code=missing_scope";
    assert!(is_auth_error(err));
    assert!(!is_retryable_acp_crash(err));
}

// --- Non-retryable: Timeout ---

#[test]
fn test_timed_out_is_not_retryable() {
    let err = "ACP prompt failed: timed out waiting for response";
    assert!(!is_retryable_acp_crash(err));
}

#[test]
fn test_idle_timeout_is_not_retryable() {
    let err = "ACP prompt failed: idle timeout exceeded";
    assert!(!is_retryable_acp_crash(err));
}

// --- Non-retryable: unrelated errors ---

#[test]
fn test_generic_error_is_not_retryable() {
    let err = "some random error message";
    assert!(!is_retryable_acp_crash(err));
}

#[test]
fn test_session_failed_is_not_retryable() {
    let err = "ACP session creation failed: invalid session ID";
    assert!(!is_retryable_acp_crash(err));
}

// --- Error formatting ---

#[test]
fn test_format_crash_retry_exhausted_contains_key_info() {
    let err = anyhow::anyhow!("server shut down unexpectedly");
    let formatted = format_crash_retry_exhausted(err, "claude-code", 2);
    let msg = formatted.to_string();
    assert!(msg.contains("2 attempts"));
    assert!(msg.contains("claude-code"));
    assert!(msg.contains("567"));
    assert!(msg.contains("--tier"));
}

#[test]
fn test_format_oom_crash_contains_key_info() {
    let err = anyhow::anyhow!("killed by signal 9 (SIGKILL)");
    let formatted = format_oom_crash(err, "codex", Some(4096));
    let msg = formatted.to_string();
    assert!(msg.contains("codex"));
    assert!(msg.contains("memory_max_mb"));
    assert!(msg.contains("4096MB"));
    assert!(msg.contains("Suggestions:"));
    assert!(msg.contains("1. Increase"));
    assert!(msg.contains("2. Reduce the task context/diff size"));
    assert!(msg.contains("3. Switch to a lower-memory tool via --tier or --tool"));
    assert!(msg.contains("--tier"));
    assert!(msg.contains("--tool"));
}

#[test]
fn test_format_oom_crash_without_explicit_limit_mentions_system_default() {
    let err = anyhow::anyhow!("killed by signal 9 (SIGKILL)");
    let formatted = format_oom_crash(err, "codex", None);
    let msg = formatted.to_string();
    assert!(msg.contains("(no explicit limit set — using system default)"));
}

#[test]
fn test_format_auth_failure_contains_key_info() {
    let err = anyhow::anyhow!("401 Unauthorized: Missing scopes: api.responses.write");
    let formatted = format_auth_failure(err, "codex");
    let msg = formatted.to_string();
    assert!(msg.contains("codex"));
    assert!(msg.contains("authentication or permission error"));
    assert!(msg.contains("scopes"));
    assert!(msg.contains("--tier"));
}

#[test]
fn test_classify_codex_acp_crash_oom_from_killed_stderr() {
    let classification = classify_codex_acp_crash(
        "server shut down unexpectedly\nKilled\n",
        None,
        None,
        Some(6144),
    );
    assert_eq!(classification.kind, CodexAcpCrashKind::Oom);
    assert!(classification.rendered_hint.contains("codex_acp_crash_oom"));
    assert!(classification.rendered_hint.contains("6144MB"));
    assert!(
        classification
            .rendered_hint
            .contains("[tools.codex].memory_max_mb")
    );
    assert!(
        classification
            .rendered_hint
            .contains("claude-code native Agent tool")
    );
}

#[test]
fn test_classify_codex_acp_crash_oom_from_stderr_tail_killed() {
    let classification = classify_codex_acp_crash(
        "ACP prompt failed: Internal error: \"server shut down unexpectedly\"; stderr: Killed",
        None,
        None,
        Some(6144),
    );
    assert_eq!(classification.kind, CodexAcpCrashKind::Oom);
}

#[test]
fn test_classify_codex_acp_crash_oom_from_cgroup_state() {
    let classification = classify_codex_acp_crash(
        "server shut down unexpectedly",
        None,
        Some("memory.max exceeded"),
        Some(4096),
    );
    assert_eq!(classification.kind, CodexAcpCrashKind::Oom);
    assert!(classification.rendered_hint.contains("4096MB"));
}

#[test]
fn test_classify_codex_acp_crash_runtime_without_oom_signature() {
    let classification = classify_codex_acp_crash(
        "ACP prompt failed: Internal error: \"server shut down unexpectedly\"",
        None,
        None,
        Some(3072),
    );
    assert_eq!(classification.kind, CodexAcpCrashKind::Runtime);
    assert!(classification.rendered_hint.contains("codex_acp_crash_runtime"));
    assert!(classification.rendered_hint.contains("3072MB"));
    assert!(
        classification
            .rendered_hint
            .contains("[tools.codex].memory_max_mb")
    );
}

#[test]
fn test_classify_codex_acp_crash_signal_11_stays_runtime() {
    let classification = classify_codex_acp_crash(
        "ACP process exited unexpectedly: killed by signal 11 (SIGSEGV)",
        None,
        None,
        Some(3072),
    );
    assert_eq!(classification.kind, CodexAcpCrashKind::Runtime);
}

#[test]
fn test_classify_codex_acp_crash_signal_90_stays_runtime() {
    let classification = classify_codex_acp_crash(
        "ACP process exited unexpectedly: killed by signal 90",
        None,
        None,
        Some(3072),
    );
    assert_eq!(classification.kind, CodexAcpCrashKind::Runtime);
}

#[test]
fn classify_extracts_exit_137_with_empty_stderr() {
    let classification = classify_codex_acp_crash(
        "ACP process exited unexpectedly: exit code 137",
        extract_crash_exit_code("ACP process exited unexpectedly: exit code 137"),
        None,
        Some(3072),
    );
    assert_eq!(classification.kind, CodexAcpCrashKind::Oom);
}

#[test]
fn classify_runtime_on_exit_1_with_oom_stderr() {
    let classification = classify_codex_acp_crash(
        "ACP process exited unexpectedly: exit code 1\nstderr: out of memory",
        extract_crash_exit_code("ACP process exited unexpectedly: exit code 1"),
        None,
        Some(3072),
    );
    assert_eq!(classification.kind, CodexAcpCrashKind::Oom);
}

#[test]
fn classify_runtime_on_exit_1_and_clean_stderr() {
    let classification = classify_codex_acp_crash(
        "ACP process exited unexpectedly: exit code 1",
        extract_crash_exit_code("ACP process exited unexpectedly: exit code 1"),
        None,
        Some(3072),
    );
    assert_eq!(classification.kind, CodexAcpCrashKind::Runtime);
}

#[test]
fn classify_signal_9_as_oom() {
    let classification = classify_codex_acp_crash(
        "ACP process exited unexpectedly",
        extract_crash_exit_code("signal: 9"),
        None,
        Some(3072),
    );
    assert_eq!(classification.kind, CodexAcpCrashKind::Oom);
}

#[test]
fn extract_matches_oom_kill_literal() {
    assert_eq!(
        extract_crash_exit_code("cgroup memory.events reported oom-kill 1"),
        Some(CrashExitCode::OomEvent)
    );
}

#[test]
fn extract_prefers_exit_137_over_generic_pattern() {
    assert_eq!(
        extract_crash_exit_code("process exited with exit code 137 after transport failure"),
        Some(CrashExitCode::ExitCode(137))
    );
}

#[test]
fn extract_parses_bare_code_format() {
    assert_eq!(
        extract_crash_exit_code("ACP process exited unexpectedly: code 137"),
        Some(CrashExitCode::ExitCode(137))
    );
}

#[test]
fn extract_parses_code_with_colon() {
    assert_eq!(
        extract_crash_exit_code("exit code: 42"),
        Some(CrashExitCode::ExitCode(42))
    );
}

#[test]
fn extract_parses_code_with_equals() {
    assert_eq!(
        extract_crash_exit_code("exit code=42"),
        Some(CrashExitCode::ExitCode(42))
    );
}

#[test]
fn extract_parses_negative_exit_code() {
    assert_eq!(
        extract_crash_exit_code("exit code: -9"),
        Some(CrashExitCode::ExitCode(-9))
    );
}

#[test]
fn extract_rejects_oom_as_substring_of_room() {
    assert_ne!(
        extract_crash_exit_code("something involving bedroom and classroom"),
        Some(CrashExitCode::OomEvent)
    );
}

#[test]
fn extract_rejects_oom_as_substring_of_zoom() {
    assert_ne!(
        extract_crash_exit_code("zoom conference started"),
        Some(CrashExitCode::OomEvent)
    );
}

#[test]
fn extract_accepts_standalone_oom_with_word_boundary() {
    assert_eq!(
        extract_crash_exit_code("kernel logged: oom killed process X"),
        Some(CrashExitCode::OomEvent)
    );
}

#[test]
fn extract_signal_9_variants_and_sigkill() {
    assert_eq!(
        extract_crash_exit_code("ACP process exited unexpectedly: signal 9"),
        Some(CrashExitCode::Signal(9))
    );
    assert_eq!(
        extract_crash_exit_code("ACP process exited unexpectedly: signal:9"),
        Some(CrashExitCode::Signal(9))
    );
    assert_eq!(
        extract_crash_exit_code("ACP process exited unexpectedly: signal: 9"),
        Some(CrashExitCode::Signal(9))
    );
    assert_eq!(
        extract_crash_exit_code("ACP process exited unexpectedly: SIGKILL"),
        Some(CrashExitCode::Signal(9))
    );
}

#[test]
fn test_format_codex_acp_crash_includes_hint_and_original_error() {
    let classification = classify_codex_acp_crash(
        "Killed",
        Some(CrashExitCode::ExitCode(137)),
        None,
        Some(2048),
    );
    let err = anyhow::anyhow!("server shut down unexpectedly");
    let formatted = format_codex_acp_crash(&classification, err, 1);
    let msg = formatted.to_string();
    assert!(msg.contains("codex_acp_crash_oom"));
    assert!(msg.contains("2048MB"));
    assert!(msg.contains("Original error"));
}

#[test]
fn test_format_codex_acp_crash_retry_exhausted_preserves_failover_anchor() {
    let classification =
        classify_codex_acp_crash("server shut down unexpectedly", None, None, Some(2048));
    let err = anyhow::anyhow!("server shut down unexpectedly");
    let formatted = format_codex_acp_crash(&classification, err, 2);
    let msg = formatted.to_string().to_ascii_lowercase();
    assert!(msg.contains("acp crash retry exhausted"));
    assert!(msg.contains("codex_acp_crash_runtime"));
}

#[test]
fn classify_signal_9_variants_as_oom() {
    for signal_variant in ["signal 9", "signal:9", "signal: 9", "sigkill"] {
        let classification =
            classify_codex_acp_crash("ACP process exited unexpectedly", extract_crash_exit_code(signal_variant), None, Some(3072));
        assert_eq!(classification.kind, CodexAcpCrashKind::Oom);
    }
}

#[test]
fn classify_signal_negative_9_as_oom() {
    let classification = classify_codex_acp_crash(
        "ACP process exited unexpectedly: exit code -9",
        extract_crash_exit_code("ACP process exited unexpectedly: exit code -9"),
        None,
        Some(3072),
    );
    assert_eq!(classification.kind, CodexAcpCrashKind::Oom);
}

// --- Issue #766: idle disconnect detection and downshift ---

fn make_transport_result(exit_code: i32, stderr: &str) -> TransportResult {
    TransportResult {
        execution: csa_process::ExecutionResult {
            output: String::new(),
            stderr_output: stderr.to_string(),
            summary: String::new(),
            exit_code,
            peak_memory_mb: None,
        },
        provider_session_id: None,
        events: Vec::new(),
        metadata: Default::default(),
    }
}

#[test]
fn idle_disconnect_detected_on_exit_137_with_idle_timeout_stderr() {
    let result = make_transport_result(
        137,
        "idle timeout: no ACP events/stderr for 250s; process killed\n",
    );
    assert!(is_idle_disconnect(&result));
}

#[test]
fn idle_disconnect_not_detected_on_exit_0() {
    let result = make_transport_result(0, "some normal output");
    assert!(!is_idle_disconnect(&result));
}

#[test]
fn idle_disconnect_not_detected_on_exit_137_without_idle_marker() {
    // Exit 137 from OOM, not idle timeout.
    let result = make_transport_result(137, "Killed\n");
    assert!(!is_idle_disconnect(&result));
}

#[test]
fn idle_disconnect_not_detected_on_initial_response_timeout() {
    // Initial response timeout also exits 137 but different marker.
    let result = make_transport_result(
        137,
        "initial response timeout: no ACP events/stderr for 60s; process killed\n",
    );
    assert!(!is_idle_disconnect(&result));
}

#[test]
fn build_downshifted_args_injects_effort_when_absent() {
    let args: Vec<String> = vec!["--acp".into()];
    let result = build_downshifted_acp_args(&args, &ThinkingBudget::Medium);
    assert_eq!(
        result,
        vec!["--acp", "-c", "model_reasoning_effort=medium"]
    );
}

#[test]
fn build_downshifted_args_replaces_existing_effort() {
    let args: Vec<String> = vec![
        "-c".into(),
        "model_reasoning_effort=high".into(),
        "--other".into(),
    ];
    let result = build_downshifted_acp_args(&args, &ThinkingBudget::Medium);
    assert_eq!(
        result,
        vec!["-c", "model_reasoning_effort=medium", "--other"]
    );
}

#[test]
fn idle_disconnect_downshift_covers_all_levels() {
    use crate::model_spec::ThinkingBudget;

    // Max → Xhigh
    assert!(matches!(
        ThinkingBudget::Max.idle_disconnect_downshift(),
        Some(ThinkingBudget::Xhigh)
    ));
    // Xhigh → High
    assert!(matches!(
        ThinkingBudget::Xhigh.idle_disconnect_downshift(),
        Some(ThinkingBudget::High)
    ));
    // High → Medium
    assert!(matches!(
        ThinkingBudget::High.idle_disconnect_downshift(),
        Some(ThinkingBudget::Medium)
    ));
    // Medium → Low
    assert!(matches!(
        ThinkingBudget::Medium.idle_disconnect_downshift(),
        Some(ThinkingBudget::Low)
    ));
    // Low → None (already minimal)
    assert!(ThinkingBudget::Low.idle_disconnect_downshift().is_none());
    // Default → None
    assert!(
        ThinkingBudget::DefaultBudget
            .idle_disconnect_downshift()
            .is_none()
    );
    // Custom → None
    assert!(
        ThinkingBudget::Custom(5000)
            .idle_disconnect_downshift()
            .is_none()
    );
}
