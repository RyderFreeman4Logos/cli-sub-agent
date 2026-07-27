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
    match classified_stdout_line(line) {
        ClassifiedStdoutLine::CommandExecutionProvenance => {
            // Command source text (item.started/completed command_execution.command)
            // is provenance, not shell/tool diagnostics. Never treat it as a
            // worker-blocked signal; STATUS: BLOCKED and unconfirmed-gate markers
            // still apply to real agent messages and tool/stdout streams below.
            false
        }
        ClassifiedStdoutLine::AgentMessage(message) => {
            line_indicates_blocked(&message)
                || (line_indicates_unconfirmed_gate(&message)
                    && (!has_positive_structured_completion
                        || !message_reports_gate_resolution(&message)))
        }
        ClassifiedStdoutLine::ToolOrCommandOutput(text) => {
            // Real tool/command output streams are never historical prose;
            // a current receipt cannot suppress them.
            line_indicates_blocked(&text) || line_indicates_unconfirmed_gate(&text)
        }
        ClassifiedStdoutLine::Raw => {
            // Non-agent raw lines are treated as live diagnostics. Only
            // agent_message prose can be suppressed as resolved history.
            line_indicates_blocked(line) || line_indicates_unconfirmed_gate(line)
        }
    }
}

enum ClassifiedStdoutLine {
    /// item.started / item.completed command_execution whose only payload is
    /// the command string itself (no stdout/aggregated_output fields).
    CommandExecutionProvenance,
    AgentMessage(String),
    ToolOrCommandOutput(String),
    Raw,
}

fn classified_stdout_line(line: &str) -> ClassifiedStdoutLine {
    let Ok(event) = serde_json::from_str::<serde_json::Value>(line) else {
        return ClassifiedStdoutLine::Raw;
    };

    if let Some(message) = agent_message_event_text_from_value(&event) {
        return ClassifiedStdoutLine::AgentMessage(message);
    }

    if let Some(item) = event.get("item") {
        let item_type = item.get("type").and_then(serde_json::Value::as_str);
        match item_type {
            Some("command_execution") => {
                // Prefer real output streams when present; otherwise the line is
                // only command provenance (search strings, argv, etc.).
                if let Some(output) = command_execution_output_text(item) {
                    return ClassifiedStdoutLine::ToolOrCommandOutput(output);
                }
                return ClassifiedStdoutLine::CommandExecutionProvenance;
            }
            Some("tool_result") | Some("function_call_output") => {
                if let Some(text) = item
                    .get("text")
                    .or_else(|| item.get("output"))
                    .or_else(|| item.get("content"))
                    .and_then(json_text_field)
                {
                    return ClassifiedStdoutLine::ToolOrCommandOutput(text);
                }
            }
            _ => {}
        }
    }

    // Unknown JSON shapes: keep scanning the original line so hard markers and
    // raw diagnostics in non-Codex providers still apply.
    ClassifiedStdoutLine::Raw
}

fn command_execution_output_text(item: &serde_json::Value) -> Option<String> {
    for key in [
        "aggregated_output",
        "stdout",
        "stderr",
        "output",
        "text",
        "content",
    ] {
        if let Some(text) = item.get(key).and_then(json_text_field) {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return Some(text);
            }
        }
    }
    None
}

fn json_text_field(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(text) => Some(text.to_owned()),
        serde_json::Value::Array(parts) => {
            let joined = parts
                .iter()
                .filter_map(|part| part.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            (!joined.is_empty()).then_some(joined)
        }
        _ => None,
    }
}

fn agent_message_event_text_from_value(event: &serde_json::Value) -> Option<String> {
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
    // Bare English "passed"/"passes" (e.g. "I passed the logs to the
    // maintainer", "parser passes data downstream") is not gate-outcome proof.
    // Require a gate/status/result-bound success signal.
    let positive_pass = gate_bound_pass_signal(&lower);
    let positive_completion = [
        "completed successfully",
        "successfully completed",
        "completion succeeded",
        "gate succeeded",
        "gate success",
        "status: success",
        "status is success",
        "now reports success",
    ]
    .iter()
    .any(|signal| lower.contains(signal));

    // A retry/fix describes an action, not its outcome. Suppress a historical
    // diagnostic only when the same message states an unambiguous success.
    (positive_pass || positive_completion) && !message_reports_unresolved_gate_outcome(&lower)
}

/// Pass/passed/passes only count when bound to a gate, status, or result outcome.
fn gate_bound_pass_signal(lower: &str) -> bool {
    // Tokenize so bare "passed the logs" / "password" cannot match.
    let tokens: Vec<&str> = lower
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .collect();

    for window in tokens.windows(2) {
        match window {
            ["now", pass]
            | ["gate", pass]
            | ["status", pass]
            | ["result", pass]
            | ["reports", pass]
            | ["report", pass]
                if is_pass_token(pass) =>
            {
                return true;
            }
            [pass, "gate"] if is_pass_token(pass) => return true,
            _ => {}
        }
    }
    for window in tokens.windows(3) {
        match window {
            [pass, "the", "gate"] if is_pass_token(pass) => return true,
            ["status", "is", pass] | ["result", "is", pass] if is_pass_token(pass) => {
                return true;
            }
            ["now", "reports", pass] | ["now", "report", pass] if is_pass_token(pass) => {
                return true;
            }
            _ => {}
        }
    }
    false
}

fn is_pass_token(token: &str) -> bool {
    matches!(token, "pass" | "passed" | "passes")
}

fn message_reports_unresolved_gate_outcome(lower: &str) -> bool {
    [
        "remains unknown",
        "still unknown",
        "remains unavailable",
        "still unavailable",
        "could not confirm",
        "unable to confirm",
        "cannot confirm",
        "remains blocked",
        "still blocked",
        "failed",
        "failure",
        "did not pass",
        "not pass",
    ]
    .iter()
    .any(|signal| lower.contains(signal))
}

/// Narrower than [`message_reports_unresolved_gate_outcome`]: matches only
/// specific failure signals that reliably indicate an unresolved gate. Bare
/// `"failed"`/`"failure"` is intentionally excluded because it appears in
/// benign prose such as `"commit failed"` (require-commit rescue) and must not
/// by itself turn a summary into a hard unconfirmed-gate blocker. The broad
/// variant is retained inside [`message_reports_gate_resolution`] as a veto
/// against false-positive "resolved" claims.
fn line_reports_unresolved_gate_outcome(lower: &str) -> bool {
    [
        "could not confirm",
        "unable to confirm",
        "cannot confirm",
        "did not pass",
        "remains blocked",
        "still blocked",
    ]
    .iter()
    .any(|signal| lower.contains(signal))
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
        || line_reports_unresolved_gate_outcome(&lower)
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
    use super::{
        line_reports_unresolved_gate_outcome, message_reports_gate_resolution,
        worker_output_indicates_blocked, worker_output_indicates_blocked_with_receipt,
    };

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
    fn bare_failed_in_summary_is_not_worker_blocked_after_require_commit_rescue() {
        // #2806: a summary like "writer completed but commit failed" contains
        // bare "failed" but no STATUS: BLOCKED marker. After a successful
        // require-commit rescue, this must NOT be rewritten to worker-blocked.
        let summary = "writer completed but commit failed";
        assert!(
            !worker_output_indicates_blocked("", summary),
            "bare 'failed' summary without STATUS: BLOCKED must not block"
        );
        assert!(
            !line_reports_unresolved_gate_outcome(summary),
            "bare 'failed' must not be treated as an unresolved gate outcome"
        );
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
        for unresolved_action_message in [
            r#"{"agent_message":"I retried the gate, but gate status remains unknown."}"#,
            r#"{"agent_message":"reran but could not confirm pass"}"#,
        ] {
            assert!(
                worker_output_indicates_blocked_with_receipt(
                    unresolved_action_message,
                    "",
                    "The retry succeeded.",
                    true,
                ),
                "action without a confirmed positive outcome must block: {unresolved_action_message}"
            );
        }
        // Bare "failed"/"failure" alone must NOT hard-block as an unconfirmed
        // gate — it is benign prose (e.g. a require-commit rescue summary such
        // as "writer completed but commit failed"). The R01 veto is retained:
        // an action+failure message still must NOT count as resolved, so a
        // historical diagnostic that reaches the suppression path is vetoed.
        let action_with_failure = r#"{"agent_message":"fixed attempt failed"}"#;
        let action_text = "fixed attempt failed";
        assert!(
            !line_reports_unresolved_gate_outcome(action_text),
            "bare 'failed' must not be an unconfirmed-gate signal"
        );
        assert!(
            !message_reports_gate_resolution(action_text),
            "action+failure must NOT count as resolved (R01 veto retained)"
        );
        assert!(
            !worker_output_indicates_blocked_with_receipt(
                action_with_failure,
                "",
                "The retry succeeded.",
                true,
            ),
            "bare 'failed' in agent prose must not block: {action_with_failure}"
        );
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

    #[test]
    fn bare_passed_prose_does_not_resolve_unconfirmed_gate() {
        // R4-001: unrelated English "passed"/"passes" is not gate-outcome proof.
        for message in [
            "Gate status unknown; I passed the logs to the maintainer.",
            "gate status unknown; parser passes data downstream",
        ] {
            assert!(
                !message_reports_gate_resolution(message),
                "bare prose must not resolve a gate: {message}"
            );
            let agent_message = format!(r#"{{"agent_message":"{message}"}}"#);
            assert!(
                worker_output_indicates_blocked_with_receipt(
                    &agent_message,
                    "",
                    "The retry succeeded.",
                    true,
                ),
                "current receipt + bare 'passed' prose must still block: {agent_message}"
            );
        }

        // Gate-bound resolution phrases remain valid suppressors.
        for message in [
            "Initial gate status was unknown; reran the gate and it now PASSes.",
            "gate status unknown earlier; gate passed after retry",
            "previous gate status unknown; status: success now",
            "prior gate status unknown; completed successfully on retry",
        ] {
            assert!(
                message_reports_gate_resolution(message),
                "gate-bound success must resolve: {message}"
            );
        }
    }

    #[test]
    fn command_execution_command_provenance_is_not_worker_blocked() {
        // R4-002: diagnostic-like text only inside command_execution.command is
        // provenance (e.g. `rg 'zsh: status unknown'`), not real shell output.
        for provenance in [
            r#"{"type":"item.started","item":{"type":"command_execution","command":"rg -n 'zsh: status unknown' crates","status":"in_progress"}}"#,
            r#"{"type":"item.completed","item":{"type":"command_execution","command":"rg -n 'zsh: status unknown' crates","exit_code":0,"status":"completed"}}"#,
            r#"{"type":"item.started","item":{"type":"command_execution","command":"grep -F 'bash: exit status unknown' logs","status":"in_progress"}}"#,
        ] {
            assert!(
                !worker_output_indicates_blocked_with_receipt(
                    provenance,
                    "",
                    "The retry succeeded.",
                    true,
                ),
                "command source must not worker-block: {provenance}"
            );
        }

        // Real command/tool output streams still block.
        assert!(worker_output_indicates_blocked_with_receipt(
            r#"{"type":"item.completed","item":{"type":"command_execution","command":"just pre-commit","aggregated_output":"zsh: status unknown","exit_code":0,"status":"completed"}}"#,
            "",
            "The retry succeeded.",
            true,
        ));
    }
}
