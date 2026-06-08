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
