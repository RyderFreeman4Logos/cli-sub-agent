use super::*;

// --- consolidate_stderr_retries tests ---

#[test]
fn test_consolidate_retries_empty() {
    let mut r = ExecutionResult {
        output: String::new(),
        stderr_output: String::new(),
        summary: String::new(),
        exit_code: 0,
        peak_memory_mb: None,
    };
    r.consolidate_stderr_retries();
    assert_eq!(r.stderr_output, "");
}

#[test]
fn test_consolidate_retries_no_match() {
    let mut r = ExecutionResult {
        output: String::new(),
        stderr_output: "some normal message\nanother line\n".to_string(),
        summary: String::new(),
        exit_code: 0,
        peak_memory_mb: None,
    };
    r.consolidate_stderr_retries();
    assert_eq!(r.stderr_output, "some normal message\nanother line\n");
}

#[test]
fn test_consolidate_retries_non_gemini_not_matched() {
    // Generic "attempt failed retry" messages should NOT be consolidated
    let mut r = ExecutionResult {
        output: String::new(),
        stderr_output: "Attempt 1 failed: connection refused. Retry scheduled.\n".to_string(),
        summary: String::new(),
        exit_code: 0,
        peak_memory_mb: None,
    };
    r.consolidate_stderr_retries();
    assert!(
        !r.stderr_output.contains("consolidated"),
        "non-gemini retry must not be consolidated: {}",
        r.stderr_output
    );
}

#[test]
fn test_consolidate_retries_single_gemini() {
    let msg = "Attempt 1 failed: You have exhausted your capacity. Retrying after 5s...\n";
    let mut r = ExecutionResult {
        output: String::new(),
        stderr_output: msg.to_string(),
        summary: String::new(),
        exit_code: 0,
        peak_memory_mb: None,
    };
    r.consolidate_stderr_retries();
    assert!(
        r.stderr_output.contains("Attempt 1 failed"),
        "single retry preserved: {}",
        r.stderr_output
    );
    assert!(
        !r.stderr_output.contains("consolidated"),
        "single retry not consolidated"
    );
}

#[test]
fn test_consolidate_retries_multiple_gemini() {
    let stderr = "\
Attempt 1 failed: You have exhausted your capacity. Retrying after 5839ms...
Attempt 1 failed: You have exhausted your capacity. Retrying after 5107ms...
Attempt 2 failed: You have exhausted your capacity. Retrying after 11411ms...
";
    let mut r = ExecutionResult {
        output: String::new(),
        stderr_output: stderr.to_string(),
        summary: String::new(),
        exit_code: 0,
        peak_memory_mb: None,
    };
    r.consolidate_stderr_retries();
    assert!(
        r.stderr_output.contains("[3 retry messages consolidated]"),
        "should consolidate: {}",
        r.stderr_output
    );
    assert!(
        r.stderr_output.contains("Attempt 2"),
        "should keep last message"
    );
}

#[test]
fn test_consolidate_retries_interleaved() {
    let stderr = "\
Attempt 1 failed: You have exhausted your capacity. Retrying after 5s...
Attempt 2 failed: You have exhausted your capacity. Retrying after 10s...
Some real error happened
Attempt 3 failed: You have exhausted your capacity. Retrying after 15s...
";
    let mut r = ExecutionResult {
        output: String::new(),
        stderr_output: stderr.to_string(),
        summary: String::new(),
        exit_code: 0,
        peak_memory_mb: None,
    };
    r.consolidate_stderr_retries();
    assert!(
        r.stderr_output.contains("[2 retry messages consolidated]"),
        "first group consolidated: {}",
        r.stderr_output
    );
    assert!(
        r.stderr_output.contains("Some real error happened"),
        "non-retry line preserved"
    );
    let lines: Vec<&str> = r.stderr_output.lines().collect();
    assert!(lines.iter().any(|l| l.contains("Attempt 3")));
}

#[test]
fn test_consolidate_quota_reset_messages() {
    let stderr = "\
You have exhausted your capacity on this model. Your quota will reset after 1s.
You have exhausted your capacity on this model. Your quota will reset after 1s.
";
    let mut r = ExecutionResult {
        output: String::new(),
        stderr_output: stderr.to_string(),
        summary: String::new(),
        exit_code: 0,
        peak_memory_mb: None,
    };
    r.consolidate_stderr_retries();
    assert!(
        r.stderr_output.contains("[2 retry messages consolidated]"),
        "quota messages consolidated: {}",
        r.stderr_output
    );
}
