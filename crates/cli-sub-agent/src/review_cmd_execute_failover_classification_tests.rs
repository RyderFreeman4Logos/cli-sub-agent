use super::super::*;

fn execution_with(
    output: &str,
    stderr: &str,
    summary: &str,
    exit_code: i32,
) -> crate::pipeline::SessionExecutionResult {
    crate::pipeline::SessionExecutionResult {
        execution: csa_process::ExecutionResult {
            output: output.to_string(),
            stderr_output: stderr.to_string(),
            summary: summary.to_string(),
            exit_code,
            peak_memory_mb: None,
            ..Default::default()
        },
        meta_session_id: "01TESTREVIEWFAILOVER".to_string(),
        provider_session_id: None,
        changed_paths: None,
        commit_created: None,
    }
}

#[test]
fn classify_review_failover_reason_uses_summary_http_400_for_review_fallback() {
    let execution = execution_with("", "", "Gemini request failed: status: 400 Bad Request", 1);

    let failure = classify_review_failover_reason(
        ToolName::GeminiCli,
        Some("gemini-cli/google/gemini-3.1-pro-preview/xhigh"),
        &execution,
        None,
        Some(std::time::Duration::from_secs(2)),
    )
    .expect("HTTP 400 summary should be failover eligible during init window");

    assert_eq!(failure.reason, "HTTP 400");
    assert_eq!(failure.quota_exhausted, Some(false));
}

#[test]
fn classify_review_failover_reason_uses_late_gemini_http_400_for_review_fallback() {
    let execution = execution_with("", "", "status: 400", 1);

    let failure = classify_review_failover_reason(
        ToolName::GeminiCli,
        Some("gemini-cli/google/gemini-3.1-pro-preview/xhigh"),
        &execution,
        None,
        Some(std::time::Duration::from_secs(39)),
    )
    .expect(
        "review-specific Gemini status:400 failure must fall back even past init window (#1958)",
    );

    assert_eq!(failure.reason, "HTTP 400");
    assert_eq!(failure.quota_exhausted, Some(false));
}

#[test]
fn classify_review_failover_error_uses_late_gemini_http_400_for_review_fallback() {
    let failure = classify_review_failover_error(
        ToolName::GeminiCli,
        Some("gemini-cli/google/gemini-3.1-pro-preview/xhigh"),
        "provider failed after startup: status: 400 Bad Request",
        Some(std::time::Duration::from_secs(39)),
    )
    .expect("review-specific Gemini status:400 errors must remain fallbackable past init window");

    assert_eq!(failure.reason, "HTTP 400");
    assert_eq!(failure.quota_exhausted, Some(false));
}

#[test]
fn classify_review_failover_error_detects_memory_soft_limit_admission() {
    let failure = classify_review_failover_error(
        ToolName::Codex,
        Some("codex/openai/gpt-5.5/xhigh"),
        "CSA: memory_soft_limit_admission denied -- codex reviewer soft memory threshold is 5734MB",
        Some(std::time::Duration::from_secs(1)),
    )
    .expect("reviewer memory admission should be failover eligible");

    assert_eq!(
        failure.reason,
        crate::resource_admission_soft_limit::MEMORY_SOFT_LIMIT_ADMISSION_REASON
    );
    assert_eq!(failure.quota_exhausted, Some(false));
}

#[test]
fn classify_no_provider_launch_error_text_detects_slot_unavailable() {
    assert_eq!(
        classify_no_provider_launch_error_text(
            "All 5 slots for 'codex' occupied (5/5). Retry later, free slots with `csa gc`, or wait for an in-flight session to finish."
        ),
        Some(crate::pipeline::SLOT_UNAVAILABLE_REASON)
    );
    assert_eq!(
        classify_no_provider_launch_error_text(
            "All 5 slots for 'codex' occupied (5/5). Try again later or use --tool to switch."
        ),
        Some(crate::pipeline::SLOT_UNAVAILABLE_REASON)
    );
    assert_ne!(
        classify_no_provider_launch_error_text("tool launch metadata missing"),
        Some(crate::pipeline::SLOT_UNAVAILABLE_REASON)
    );
}

#[test]
fn classify_review_failover_error_detects_host_memory_admission() {
    let failure = classify_review_failover_error(
        ToolName::Codex,
        Some("codex/openai/gpt-5.5/xhigh"),
        "CSA: host memory admission denied — available=11385MB < required=12858MB",
        Some(std::time::Duration::from_secs(1)),
    )
    .expect("reviewer host admission should be failover eligible");

    assert_eq!(
        failure.reason,
        crate::no_provider_launch::HOST_MEMORY_ADMISSION_REASON
    );
    assert_eq!(failure.quota_exhausted, Some(false));
}

#[test]
fn classify_review_failover_reason_detects_gemini_noninteractive_manual_auth() {
    let stderr = "\
Error authenticating: FatalAuthenticationError: Manual authorization is required but the current session is non-interactive. \
Please run the Gemini CLI in an interactive terminal to log in, provide a GEMINI_API_KEY, \
or ensure Application Default Credentials are configured.
    at async main (file:///usr/local/share/mise/installs/npm-google-gemini-cli/0.45.2/lib/node_modules/@google/gemini-cli/bundle/gemini-75GSY6S7.js:16052:9)";
    let execution = execution_with("", stderr, "", 41);

    let failure = classify_review_failover_reason(
        ToolName::GeminiCli,
        Some("gemini-cli/google/gemini-3.1-pro-preview/xhigh"),
        &execution,
        None,
        Some(std::time::Duration::from_secs(39)),
    )
    .expect("Gemini non-interactive manual auth must fall back to the next reviewer (#1949)");

    assert_eq!(failure.reason, "auth_unavailable");
    assert_eq!(failure.quota_exhausted, Some(false));
}

#[test]
fn classify_review_failover_reason_detects_gemini_stack_frame_crash() {
    let execution = execution_with(
        "",
        "",
        "at async main (file:///usr/local/share/mise/installs/npm-google-gemini-cli/0.45.2/lib/node_modules/@google/gemini-cli/bundle/gemini-75GSY6S7.js:16120:5)",
        1,
    );

    let failure = classify_review_failover_reason(
        ToolName::GeminiCli,
        Some("gemini-cli/google/gemini-3.1-pro-preview/xhigh"),
        &execution,
        None,
        Some(std::time::Duration::from_secs(39)),
    )
    .expect("Gemini CLI internal JS stack crash must be fallbackable (#1936)");

    assert_eq!(failure.reason, "gemini_cli_crash");
    assert_eq!(failure.quota_exhausted, None);
}

#[test]
fn classify_review_failover_reason_detects_gemini_runtime_home_enospc() {
    let stderr = "\
Failed to save project registry to /state/sessions/01TEST/runtime/gemini-home/.gemini/projects.json: \
Error: ENOSPC: no space left on device, rename '/state/sessions/01TEST/runtime/gemini-home/.gemini/projects.json.tmp' \
-> '/state/sessions/01TEST/runtime/gemini-home/.gemini/projects.json'
An unexpected critical error occurred:Error: ENOSPC: no space left on device, rename '/state/sessions/01TEST/runtime/gemini-home/.gemini/projects.json.tmp' \
-> '/state/sessions/01TEST/runtime/gemini-home/.gemini/projects.json'";
    let execution = execution_with("", stderr, "", 1);

    let failure = classify_review_failover_reason(
        ToolName::GeminiCli,
        Some("gemini-cli/google/gemini-3.1-pro-preview/xhigh"),
        &execution,
        None,
        Some(std::time::Duration::from_secs(39)),
    )
    .expect("Gemini runtime-home ENOSPC must fall back to the next reviewer (#1938)");

    assert_eq!(failure.reason, "gemini_runtime_home_unavailable");
    assert_eq!(failure.quota_exhausted, None);
}

#[test]
fn classify_review_failover_reason_preserves_completed_structured_review() {
    let output = "\
<!-- CSA:SECTION:summary -->
HAS_ISSUES
<!-- CSA:SECTION:summary:END -->
<!-- CSA:SECTION:details -->
- Critical: real review finding.
<!-- CSA:SECTION:details:END -->";
    let stderr = "\
An unexpected critical error occurred:Error: ENOSPC: no space left on device, rename \
'/state/sessions/01TEST/runtime/gemini-home/.gemini/projects.json.tmp' \
-> '/state/sessions/01TEST/runtime/gemini-home/.gemini/projects.json'";
    let execution = execution_with(output, stderr, "Review completed with findings", 1);

    let failure = classify_review_failover_reason(
        ToolName::GeminiCli,
        Some("gemini-cli/google/gemini-3.1-pro-preview/xhigh"),
        &execution,
        None,
        Some(std::time::Duration::from_secs(39)),
    );

    assert!(
        failure.is_none(),
        "a completed structured review must not be reclassified as reviewer-tool unavailable"
    );
}

#[test]
fn classify_review_failover_reason_detects_gemini_initial_stall() {
    let stderr = "\
[csa-heartbeat] tool still running: elapsed=594s idle=589s idle-timeout=600s
gemini_legacy_initial_stall: no stdout within 600s";
    let execution = execution_with(
        "",
        stderr,
        "gemini_legacy_initial_stall: no stdout within 600s",
        137,
    );

    let failure = classify_review_failover_reason(
        ToolName::GeminiCli,
        Some("gemini-cli/google/gemini-3.1-pro-preview/xhigh"),
        &execution,
        None,
        Some(std::time::Duration::from_secs(602)),
    )
    .expect("Gemini initial stall must fall back to the next reviewer (#1987)");

    assert_eq!(failure.reason, "gemini_stall_timeout");
    assert_eq!(failure.quota_exhausted, None);
}

#[test]
fn classify_review_failover_reason_detects_gemini_idle_timeout() {
    let stderr = "idle_timeout: gemini-cli produced no output for 600s";
    let execution = execution_with("", stderr, "", 137);

    let failure = classify_review_failover_reason(
        ToolName::GeminiCli,
        Some("gemini-cli/google/gemini-3.1-pro-preview/xhigh"),
        &execution,
        None,
        Some(std::time::Duration::from_secs(602)),
    )
    .expect("Gemini idle timeout must fall back to the next reviewer (#1987)");

    assert_eq!(failure.reason, "gemini_stall_timeout");
    assert_eq!(failure.quota_exhausted, None);
}
