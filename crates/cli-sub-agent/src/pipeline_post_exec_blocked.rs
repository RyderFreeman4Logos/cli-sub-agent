//! Worker-blocked gate for `process_execution_result` (#1483).
//!
//! Detects sessions that exit 0 but output a "STATUS: BLOCKED" marker,
//! indicating the worker could not complete the task.

/// Returns true when the tool output or summary contains a hard-blocker marker
/// or reports that a required gate did not produce a confirmed PASS. Either
/// condition means the worker could not finish the task.
pub(super) fn worker_output_indicates_blocked(output: &str, summary: &str) -> bool {
    if line_indicates_blocked(summary) || line_indicates_unconfirmed_gate(summary) {
        return true;
    }
    output
        .lines()
        .any(|line| line_indicates_blocked(line) || line_indicates_unconfirmed_gate(line))
}

fn line_indicates_blocked(line: &str) -> bool {
    let trimmed = line.trim();
    let upper = trimmed.to_ascii_uppercase();
    upper == "STATUS: BLOCKED"
        || upper.starts_with("STATUS: BLOCKED")
        || upper.starts_with("BLOCKED:")
}

fn line_indicates_unconfirmed_gate(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    let lost_status = lower.contains("unknown")
        || lower.contains("unavailable")
        || lower.contains("lost")
        || lower.contains("not available");
    lower.contains("unable to confirm gate pass")
        || lower.contains("cannot confirm gate pass")
        || lower.contains("zsh: read-only variable: status")
        || ((lower.contains("gate exit")
            || lower.contains("gate status")
            || lower.contains("exit status"))
            && lost_status)
        || (line.contains("\u{65e0}\u{6cd5}\u{786e}\u{8ba4}\u{95e8}\u{7981}")
            && lower.contains("pass"))
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

    #[test]
    fn blocked_colon_summary_detected() {
        assert!(worker_output_indicates_blocked(
            "",
            "Blocked: commit was not created because the pre-commit hook failed"
        ));
    }

    #[test]
    fn raw_shell_lost_gate_diagnostics_are_blocked() {
        for diagnostic in [
            "zsh: read-only variable: status",
            "bash: exit status unavailable",
            "bash: exit status unknown",
            "bash: exit status lost",
            "bash: exit status not available",
        ] {
            assert!(
                worker_output_indicates_blocked("", diagnostic),
                "raw lost-gate diagnostic must block: {diagnostic}"
            );
        }
    }

    #[test]
    fn unconfirmed_gate_in_reported_summary_is_blocked() {
        assert!(worker_output_indicates_blocked(
            "",
            "\u{95e8}\u{7981}\u{5df2}\u{6267}\u{884c}\u{4e00}\u{6b21}\u{ff0c}\u{4f46}\u{65e0}\u{6cd5}\u{786e}\u{8ba4}\u{95e8}\u{7981} PASS\u{ff1b}\u{672a}\u{91cd}\u{8dd1}\u{3002}"
        ));
    }
}
