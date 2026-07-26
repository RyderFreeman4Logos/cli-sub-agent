//! Worker-blocked gate for `process_execution_result` (#1483).
//!
//! Detects sessions that exit 0 but output a "STATUS: BLOCKED" marker,
//! indicating the worker could not complete the task.

#[cfg(test)]
pub(super) fn worker_output_indicates_blocked(output: &str, summary: &str) -> bool {
    worker_output_indicates_blocked_with_receipt(output, "", summary, false)
}

/// Returns true when the tool output, stderr, or summary contains a hard-blocker
/// marker or reports that a required gate did not produce a confirmed PASS. A
/// current structured success receipt suppresses historical summary and
/// resolved agent-message prose; raw shell/tool output and stderr blockers
/// still fail the worker.
pub(super) fn worker_output_indicates_blocked_with_receipt(
    output: &str,
    stderr_output: &str,
    summary: &str,
    has_positive_structured_completion: bool,
) -> bool {
    if line_indicates_blocked(summary)
        || (!has_positive_structured_completion && line_indicates_unconfirmed_gate(summary))
    {
        return true;
    }
    output
        .lines()
        .any(|line| stdout_line_indicates_blocked(line, has_positive_structured_completion))
        || stderr_output
            .lines()
            .any(|line| line_indicates_blocked(line) || line_indicates_unconfirmed_gate(line))
}

fn stdout_line_indicates_blocked(line: &str, has_positive_structured_completion: bool) -> bool {
    line_indicates_blocked(line)
        || agent_message_event_text(line).is_some_and(|message| line_indicates_blocked(&message))
        || (line_indicates_unconfirmed_gate(line)
            && (!has_positive_structured_completion
                || !agent_message_describes_resolved_historical_gate(line)))
}

fn agent_message_describes_resolved_historical_gate(line: &str) -> bool {
    agent_message_event_text(line).is_some_and(|message| {
        line_indicates_unconfirmed_gate(&message) && message_reports_gate_resolution(&message)
    })
}

fn agent_message_event_text(line: &str) -> Option<String> {
    let event = serde_json::from_str::<serde_json::Value>(line).ok()?;
    if let Some(message) = event.get("agent_message") {
        return agent_message_value_text(message);
    }

    (event.get("type").and_then(serde_json::Value::as_str) == Some("item.completed"))
        .then_some(())?;
    let item = event.get("item")?;
    (item.get("type").and_then(serde_json::Value::as_str) == Some("agent_message")).then_some(())?;
    item.get("text")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}

fn agent_message_value_text(message: &serde_json::Value) -> Option<String> {
    match message {
        serde_json::Value::String(text) => Some(text.to_owned()),
        serde_json::Value::Object(_) => message
            .get("text")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        _ => None,
    }
}

fn message_reports_gate_resolution(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    let explicit_resolution = [
        "reran",
        "rerun",
        "re-ran",
        "re-run",
        "retried",
        "retry succeeded",
        "fixed",
        "resolved",
        "completed both",
    ]
    .iter()
    .any(|signal| lower.contains(signal));
    let positive_pass = lower
        .split(|character: char| !character.is_ascii_alphanumeric())
        .any(|word| matches!(word, "pass" | "passed" | "passes"));
    let unconfirmed_pass = lower.contains("unable to confirm gate pass")
        || lower.contains("cannot confirm gate pass")
        || lower.contains("did not pass")
        || lower.contains("not pass");

    explicit_resolution || (positive_pass && !unconfirmed_pass)
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
    let readonly_status_variable = ["bash:", "zsh:"].iter().any(|shell| lower.contains(shell))
        && lower.contains("status")
        && (lower.contains("readonly variable") || lower.contains("read-only variable"));
    let shell_lost_status = ["bash:", "zsh:"].iter().any(|shell| lower.contains(shell))
        && lower.contains("status")
        && lost_status;
    let required_work_omitted =
        lower.contains("omitted") && lower.contains("test") && lower.contains("commit");
    lower.contains("unable to confirm gate pass")
        || lower.contains("cannot confirm gate pass")
        || readonly_status_variable
        || shell_lost_status
        || required_work_omitted
        || ((lower.contains("gate exit")
            || lower.contains("gate status")
            || lower.contains("exit status"))
            && lost_status)
        || (line.contains("\u{65e0}\u{6cd5}\u{786e}\u{8ba4}\u{95e8}\u{7981}")
            && lower.contains("pass"))
}

#[cfg(test)]
mod tests {
    use super::{worker_output_indicates_blocked, worker_output_indicates_blocked_with_receipt};

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
            "zsh: status: readonly variable",
            "zsh: status unknown",
            "bash: status: readonly variable",
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

    #[test]
    fn current_receipt_suppresses_historical_agent_stdout_prose_but_not_raw_diagnostics() {
        assert!(!worker_output_indicates_blocked_with_receipt(
            "",
            "",
            "The previous turn omitted tests and commit; this turn completed both.",
            true,
        ));
        assert!(!worker_output_indicates_blocked_with_receipt(
            "",
            "",
            "Initial gate status was unknown; reran the gate and it now PASSes.",
            true,
        ));
        assert!(!worker_output_indicates_blocked_with_receipt(
            r#"{"type":"item.completed","item":{"type":"agent_message","text":"Initial gate status was unknown; reran the gate and it now PASSes."}}"#,
            "",
            "The retry succeeded.",
            true,
        ));
        for resolved_historical_message in [
            r#"{"agent_message":"Initial gate status was unknown; reran the gate and it now PASSes."}"#,
            r#"{"agent_message":{"text":"Initial gate status was unknown; reran the gate and it now PASSes."}}"#,
        ] {
            assert!(
                !worker_output_indicates_blocked_with_receipt(
                    resolved_historical_message,
                    "",
                    "The retry succeeded.",
                    true,
                ),
                "resolved historical agent-message prose must not block: {resolved_historical_message}"
            );
        }
        for diagnostic in [
            r#"{"type":"item.completed","item":{"type":"agent_message","text":"zsh: read-only variable: status"}}"#,
            r#"{"agent_message":"bash: exit status unknown"}"#,
            r#"{"agent_message":{"text":"zsh: read-only variable: status"}}"#,
            r#"{"agent_message":"tests and commit omitted"}"#,
        ] {
            assert!(
                worker_output_indicates_blocked_with_receipt(
                    diagnostic,
                    "",
                    "The retry succeeded.",
                    true,
                ),
                "unresolved agent-message diagnostic must block: {diagnostic}"
            );
        }
        assert!(worker_output_indicates_blocked_with_receipt(
            r#"{"agent_message":"STATUS: BLOCKED — current gate is unavailable"}"#,
            "",
            "The retry succeeded.",
            true,
        ));
        assert!(worker_output_indicates_blocked_with_receipt(
            r#"{"type":"item.completed","item":{"type":"tool_result","text":"zsh: status unknown"}}"#,
            "",
            "The retry succeeded.",
            true,
        ));
        assert!(worker_output_indicates_blocked_with_receipt(
            "",
            "zsh: status unknown",
            "The retry succeeded.",
            true,
        ));
    }
}
