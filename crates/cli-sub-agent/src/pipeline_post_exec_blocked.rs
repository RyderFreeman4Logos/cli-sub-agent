//! Worker-blocked gate for `process_execution_result` (#1483).
//!
//! Detects sessions that exit 0 but output a "STATUS: BLOCKED" marker,
//! indicating the worker could not complete the task.

/// Returns true when the tool output or summary contains a "STATUS: BLOCKED"
/// line, indicating the worker detected a hard blocker (e.g. Bash unavailable
/// due to EROFS, missing required tooling) and could not finish the task.
pub(super) fn worker_output_indicates_blocked(output: &str, summary: &str) -> bool {
    let summary_trimmed = summary.trim();
    if summary_trimmed.eq_ignore_ascii_case("STATUS: BLOCKED")
        || summary_trimmed
            .to_ascii_uppercase()
            .starts_with("STATUS: BLOCKED")
    {
        return true;
    }
    output.lines().any(|line| {
        let t = line.trim();
        t.eq_ignore_ascii_case("STATUS: BLOCKED")
            || t.to_ascii_uppercase().starts_with("STATUS: BLOCKED")
    })
}

#[cfg(test)]
mod tests {
    use super::worker_output_indicates_blocked;

    #[test]
    fn blocked_summary_exact_match() {
        assert!(worker_output_indicates_blocked("", "STATUS: BLOCKED"));
    }

    #[test]
    fn blocked_summary_case_insensitive() {
        assert!(worker_output_indicates_blocked("", "status: blocked"));
        assert!(worker_output_indicates_blocked("", "Status: Blocked"));
    }

    #[test]
    fn blocked_summary_with_trailing_text() {
        assert!(worker_output_indicates_blocked(
            "",
            "STATUS: BLOCKED — Bash tool unavailable (EROFS)"
        ));
    }

    #[test]
    fn blocked_detected_in_output_line() {
        let output = "Attempting task...\nSTATUS: BLOCKED\nSome trailing text";
        assert!(worker_output_indicates_blocked(output, "Some summary"));
    }

    #[test]
    fn non_blocked_summary_returns_false() {
        assert!(!worker_output_indicates_blocked(
            "all good",
            "Task completed successfully"
        ));
    }

    #[test]
    fn empty_inputs_return_false() {
        assert!(!worker_output_indicates_blocked("", ""));
    }

    #[test]
    fn partial_match_not_triggered() {
        // "BLOCKED" alone (without STATUS: prefix) must not trigger
        assert!(!worker_output_indicates_blocked("BLOCKED", "BLOCKED"));
    }
}
