use super::*;

fn result_with(summary: &str, stderr: &str, stdout: &str) -> csa_process::ExecutionResult {
    csa_process::ExecutionResult {
        summary: summary.to_string(),
        stderr_output: stderr.to_string(),
        output: stdout.to_string(),
        exit_code: 1,
        ..Default::default()
    }
}

#[test]
fn reviewed_quota_phrase_in_stdout_is_not_permanent_exhaustion() {
    // #1736: a `csa review` of a diff that literally contains a quota phrase
    // (e.g. this failover classifier's own source / a test fixture / a commit
    // message) must NOT self-kill the session. The marker is in agent stdout
    // and the stdout-derived summary only, never the provider error channel.
    let result = result_with(
        "+    pattern: \"monthly spending cap\",",
        "",
        "Reviewing diff:\n+const PATTERN: &str = \"monthly spending cap\";\n\
         +// also matches \"usage limit\" / QUOTA_EXHAUSTED\nVerdict: PASS",
    );
    assert!(
        detect_permanent_tool_exhaustion_result("codex", &result, None).is_none(),
        "reviewed quota phrase in stdout/summary must not be permanent exhaustion"
    );
    assert!(
        detect_permanent_tool_exhaustion_result("gemini-cli", &result, None).is_none(),
        "reviewed quota phrase in stdout/summary must not be permanent exhaustion (gemini)"
    );
}

#[test]
fn provider_quota_error_on_stderr_is_permanent_exhaustion() {
    // Positive: a genuine provider quota error on the stderr (provider error)
    // channel MUST still be detected as permanent so failover handling fires.
    let result = result_with(
        "Verdict: PASS",
        "Error: Usage limit exceeded for this account",
        "Reviewing diff... Verdict: PASS",
    );
    let detected = detect_permanent_tool_exhaustion_result("codex", &result, None)
        .expect("provider usage-limit error on stderr must be permanent exhaustion");
    assert!(detected.quota_exhausted);
    assert_eq!(detected.matched_pattern, "usage limit");
}

#[test]
fn transport_error_chain_quota_is_permanent_exhaustion() {
    // The error path feeds the anyhow error chain as the provider channel; a
    // provider quota error there must still be detected.
    let detected = detect_permanent_tool_exhaustion_text(
        "codex",
        "transport: ACP prompt failed: You've hit your usage limit. \
         insufficient_quota for this account",
        1,
        None,
    )
    .expect("provider quota in transport error chain must be permanent exhaustion");
    assert!(detected.quota_exhausted);
}

#[test]
fn reviewed_quota_phrase_in_transport_error_summary_is_not_permanent() {
    // A transport error whose text merely echoes reviewed content (no real
    // provider quota framing) must not self-kill. `detect_rate_limit` requires
    // the marker in the provider channel AND will only set quota_exhausted for
    // genuine quota patterns — a bare unrelated failure does not match.
    assert!(
        detect_permanent_tool_exhaustion_text(
            "codex",
            "transport: turn failed while reviewing line `pattern: \"monthly\"`",
            1,
            None,
        )
        .is_none(),
        "unrelated transport failure must not be permanent exhaustion"
    );
}
