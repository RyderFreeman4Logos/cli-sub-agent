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

    let event_type = event.get("type").and_then(serde_json::Value::as_str);
    // Claude stream-json tool_use / tool_call carry the invoked command in
    // `input.command` (and tool name). That is provenance — same as Codex
    // command_execution.command — not live shell output.
    if matches!(event_type, Some("tool_use") | Some("tool_call")) {
        if let Some(output) = claude_tool_use_output_text(&event) {
            return ClassifiedStdoutLine::ToolOrCommandOutput(output);
        }
        return ClassifiedStdoutLine::CommandExecutionProvenance;
    }
    // Claude tool_result / tool_call_result: scan real tool output fields only.
    // Prefer content/output/text/result in order, extracting Claude content-block
    // arrays; empty/unparsed fields fall through so a later populated field is
    // not shadowed (#2806 R7-001).
    if matches!(event_type, Some("tool_result") | Some("tool_call_result")) {
        if let Some(text) = claude_tool_result_output_text(&event) {
            return ClassifiedStdoutLine::ToolOrCommandOutput(text);
        }
        return ClassifiedStdoutLine::CommandExecutionProvenance;
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
                if let Some(text) = claude_tool_result_output_text(item) {
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

/// Real tool output on a Claude tool_use envelope, if any.
///
/// Claude Bash-class tool_use normally only carries `input.command` (provenance).
/// When a provider embeds actual output fields, prefer those over provenance.
fn claude_tool_use_output_text(event: &serde_json::Value) -> Option<String> {
    for key in [
        "aggregated_output",
        "stdout",
        "stderr",
        "output",
        "text",
        "content",
    ] {
        if let Some(text) = event.get(key).and_then(json_text_field) {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return Some(text);
            }
        }
    }
    None
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

/// Non-empty tool/command text from a Claude tool_result-shaped object.
///
/// Claude content may be a plain string, a string array, or content blocks
/// `[{"type":"text","text":"..."}]`. Empty or unparsed fields fall through so a
/// later `output`/`text`/`result` value is still classified as live tool output.
fn claude_tool_result_output_text(event: &serde_json::Value) -> Option<String> {
    for key in ["content", "output", "text", "result"] {
        if let Some(text) = event.get(key).and_then(json_text_field) {
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
            let mut buf = String::new();
            for part in parts {
                if let Some(text) = part.as_str() {
                    if !buf.is_empty() {
                        buf.push('\n');
                    }
                    buf.push_str(text);
                    continue;
                }
                // Claude content blocks: {"type":"text","text":"..."}.
                if part.get("type").and_then(serde_json::Value::as_str) == Some("text")
                    && let Some(text) = part.get("text").and_then(serde_json::Value::as_str)
                {
                    buf.push_str(text);
                }
            }
            (!buf.is_empty()).then_some(buf)
        }
        _ => None,
    }
}

fn agent_message_event_text_from_value(event: &serde_json::Value) -> Option<String> {
    if let Some(message) = event.get("agent_message") {
        return agent_message_value_text(message);
    }

    let event_type = event.get("type").and_then(serde_json::Value::as_str);
    // Claude stream-json assistant envelopes: extract prose only so resolved
    // historical gate text can use the current-receipt suppression path, and so
    // the full JSON (including unrelated fields) is not scanned as Raw.
    if matches!(event_type, Some("assistant") | Some("assistant_message")) {
        return claude_assistant_message_text(event);
    }
    // Claude terminal result envelope carries the final agent prose in
    // `result`. Treat successful envelopes like assistant messages so resolved
    // historical gate text is not re-scanned as live Raw diagnostics (#2806
    // R6-003). Error result envelopes stay unclassified (fail-closed → Raw).
    if event_type == Some("result") {
        return claude_result_envelope_text(event);
    }

    (event_type == Some("item.completed")).then_some(())?;
    let item = event.get("item")?;
    (item.get("type").and_then(serde_json::Value::as_str) == Some("agent_message")).then_some(())?;
    item.get("text")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}

/// Claude `{"type":"result","result":"..."}` terminal prose.
///
/// Successful envelopes yield agent-message text. Error envelopes (`is_error`,
/// `subtype` starting with `error` such as `error_api`, or non-bool `is_error`)
/// return `None` so classification falls through to Raw (fail-closed). Matches
/// `review_cmd_output_text::claude_result_is_error` subtype handling (#2806 R7-002).
fn claude_result_envelope_text(event: &serde_json::Value) -> Option<String> {
    if let Some(is_error) = event.get("is_error") {
        match is_error.as_bool() {
            Some(true) => return None,
            Some(false) => {}
            // Missing bool parse (string/number/null): do not assume success.
            None => return None,
        }
    }
    if event
        .get("subtype")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|subtype| subtype.to_ascii_lowercase().starts_with("error"))
    {
        return None;
    }
    event
        .get("result")
        .and_then(json_text_field)
        .filter(|text| !text.trim().is_empty())
}

fn agent_message_value_text(message: &serde_json::Value) -> Option<String> {
    match message {
        serde_json::Value::String(text) => Some(text.to_owned()),
        serde_json::Value::Object(_) => message
            .get("text")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
            .or_else(|| claude_message_content_text(message)),
        _ => None,
    }
}

/// Claude assistant / assistant_message text, matching transport_cli extraction.
fn claude_assistant_message_text(event: &serde_json::Value) -> Option<String> {
    if let Some(text) = event
        .get("message")
        .and_then(claude_message_content_text)
        .filter(|text| !text.is_empty())
    {
        return Some(text);
    }
    event
        .get("text")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .filter(|text| !text.is_empty())
}

fn claude_message_content_text(message: &serde_json::Value) -> Option<String> {
    let content = message.get("content").unwrap_or(message);
    if let Some(text) = content.as_str() {
        return Some(text.to_owned());
    }
    let arr = content.as_array()?;
    let mut buf = String::new();
    for block in arr {
        if block.get("type").and_then(serde_json::Value::as_str) == Some("text")
            && let Some(text) = block.get("text").and_then(serde_json::Value::as_str)
        {
            buf.push_str(text);
        }
    }
    (!buf.is_empty()).then_some(buf)
}

pub(super) fn message_reports_gate_resolution(message: &str) -> bool {
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

/// Pass/passed/passes only count when syntactically bound to a gate, status, or
/// result outcome. Bare `now passed` / `reports passed` / `report passed` are
/// unbound English and must not suppress unresolved gate diagnostics (#2806
/// R6-002).
fn gate_bound_pass_signal(lower: &str) -> bool {
    // Tokenize so bare "passed the logs" / "password" cannot match.
    let tokens: Vec<&str> = lower
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .collect();

    for window in tokens.windows(2) {
        match window {
            ["gate", pass] | ["status", pass] | ["result", pass] if is_pass_token(pass) => {
                return true;
            }
            [pass, "gate"] if is_pass_token(pass) => return true,
            _ => {}
        }
    }
    for window in tokens.windows(3) {
        match window {
            [pass, "the", "gate"] if is_pass_token(pass) => return true,
            // "gate status: passed" / "gate status passed"
            ["gate", "status", pass] if is_pass_token(pass) => return true,
            // "status is pass" / "result is passed"
            ["status", "is", pass] | ["result", "is", pass] if is_pass_token(pass) => {
                return true;
            }
            // "now PASSes the gate" — pass is bound via "the gate", not bare "now"
            _ => {}
        }
    }
    // "gate status: passed" may tokenize as gate/status/passed (covered above)
    // or as longer phrases; also accept "status: pass" already via 2-token.
    false
}

fn is_pass_token(token: &str) -> bool {
    matches!(token, "pass" | "passed" | "passes")
}

fn message_reports_unresolved_gate_outcome(lower: &str) -> bool {
    let unresolved_signal = [
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
    .any(|signal| lower.contains(signal));
    // "gate passed, but tests and commit omitted" is not a full resolution —
    // current omitted required work vetoes a positive pass claim (#2806 R6-002).
    // Historical "previously omitted" that was fixed/completed this turn must
    // not veto a genuine gate-bound pass.
    unresolved_signal || message_reports_current_omitted_required_work(lower)
}

/// True when the message still claims tests/commit were omitted (present
/// omission), not when it only narrates previously-omitted work that was fixed.
fn message_reports_current_omitted_required_work(lower: &str) -> bool {
    if !(lower.contains("omitted") && lower.contains("test") && lower.contains("commit")) {
        return false;
    }
    // Historical narration: "fixed the previously omitted tests and commit".
    if lower.contains("previously omitted") {
        return false;
    }
    true
}

/// Narrower than [`message_reports_unresolved_gate_outcome`]: matches only
/// specific failure signals that reliably indicate an unresolved gate. Bare
/// `"failed"`/`"failure"` is intentionally excluded because it appears in
/// benign prose such as `"commit failed"` (require-commit rescue) and must not
/// by itself turn a summary into a hard unconfirmed-gate blocker. The broad
/// variant is retained inside [`message_reports_gate_resolution`] as a veto
/// against false-positive "resolved" claims.
pub(super) fn line_reports_unresolved_gate_outcome(lower: &str) -> bool {
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
#[path = "pipeline_post_exec_blocked_tests.rs"]
mod tests;
