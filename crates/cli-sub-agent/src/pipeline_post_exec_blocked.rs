//! Worker-blocked gate for `process_execution_result` (#1483).
//!
//! Detects sessions that exit 0 but output a "STATUS: BLOCKED" marker,
//! indicating the worker could not complete the task.

mod gate_signal {
    include!("pipeline_post_exec_blocked_gate_signal.rs");
}

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
    if text_indicates_blocked(summary)
        || (text_indicates_unconfirmed_gate(summary)
            && (!has_positive_structured_completion || !message_reports_gate_resolution(summary)))
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
            // Extracted agent prose may span multiple lines (content-block
            // arrays joined with newlines). Scan every line so a hard marker
            // on a trailing line is not missed (#2806 R8-F1).
            text_indicates_blocked(&message)
                || (text_indicates_unconfirmed_gate(&message)
                    && (!has_positive_structured_completion
                        || !message_reports_gate_resolution(&message)))
        }
        ClassifiedStdoutLine::ToolOrCommandOutput(text) => {
            // Real tool/command output streams are never historical prose;
            // a current receipt cannot suppress them. Scan every line so a
            // hard marker on a trailing line is not missed (#2806 R8-F1).
            text_indicates_blocked(&text) || text_indicates_unconfirmed_gate(&text)
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
                    // Insert a newline separator between blocks so a hard marker
                    // in a trailing element is not fused onto the previous text
                    // and lost by start-anchored line checks (#2806 R8-F1).
                    if !buf.is_empty() {
                        buf.push('\n');
                    }
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
            // Newline-separate content blocks so a trailing hard marker is not
            // fused onto earlier text and missed by start-anchored line checks
            // (#2806 R8-F1).
            if !buf.is_empty() {
                buf.push('\n');
            }
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
    let positive_pass = gate_signal::gate_bound_pass_signal(&lower);
    // R11-F1 (positive_completion trailing-token scan): these success phrases
    // were previously matched with a bare `contains()` substring test that
    // bypassed the trailing-token anchoring applied to pass signals. That let
    // forged prose such as "gate exit unavailable; gate success report
    // forwarded downstream" (matches "gate success") forge a false resolution.
    // Each phrase is now matched as a contiguous token subsequence AND gated by
    // the same `clause_is_terminal_anchored` denylist used by
    // `gate_bound_pass_signal`, so any gate-object noun (report/logs/to/...,
    // plus gate/gates/completion per R11-F2) in the trailing tokens rejects the
    // clause as mid-sentence prose (#2806 R11-F1).
    let positive_completion_phrases = [
        "completed successfully",
        "successfully completed",
        "completion succeeded",
        "gate succeeded",
        "gate success",
        "status: success",
        "status is success",
        "now reports success",
    ];
    let positive_completion =
        gate_signal::any_terminal_anchored_phrase(&lower, &positive_completion_phrases);

    // A retry/fix describes an action, not its outcome. Suppress a historical
    // diagnostic only when the same message states an unambiguous success.
    (positive_pass || positive_completion) && !message_reports_unresolved_gate_outcome(&lower)
}

/// Pass/passed/passes only count when syntactically bound to a gate, status, or
/// result outcome AND fully anchored as a terminal clause. The anchor and
/// gate-object-noun logic lives in [`gate_signal`] (#2806 R6-002, R8-F2).
///
/// R14-F2 convergence unification: EVERY veto phrase is now classified by the
/// SAME shared per-clause classifier in [`gate_signal`]. The unresolved-signal
/// list, the failed/failure path, the status-unknown path, and the omitted-
/// required-work check all share one [`gate_signal::tokenize_by_clauses`]-based
/// scope and one whole-clause historical-qualifier binding. Before R14 these
/// used three different scopes (whole-message contains, ±2-token, per-line),
/// which let mixed historical/current messages slip through in inconsistent
/// directions (#2806 R10→R14 convergence protocol).
fn message_reports_unresolved_gate_outcome(lower: &str) -> bool {
    // Unresolved signals are now matched per-clause: a historical qualifier
    // ANYWHERE in the same clause marks the occurrence as past narration, so a
    // prior clause's unresolved signal does not veto a later terminal pass
    // (#2806 R14-F2).
    let unresolved_signal = gate_signal::reports_current_unresolved_signal(lower);
    // R13-P2/R14-F2: bare failure words remain a resolution veto when current,
    // but a historical failure in an earlier clause ("Previous turn failed.
    // Gate passed.") must not reject the later terminal outcome.
    let current_failure = gate_signal::reports_current_failure(lower);
    // A CONCURRENT "status unknown/unavailable/lost" (not a historical "prior
    // status unknown") unconditionally vetoes a positive pass claim, even when
    // the pass signal is syntactically gate-bound (#2806 R8-F2, R14-F1).
    let concurrent_status_lost = gate_signal::reports_concurrent_status_unknown(lower);
    // "gate passed, but tests and commit omitted" is not a full resolution —
    // current omitted required work vetoes a positive pass claim (#2806
    // R6-002). The check is now CLAUSE-scoped: a historical marker on a line
    // exempts only the clause it sits in, not every clause on that line (#2806
    // R10-F2, R14-F3).
    let omitted_required_work = gate_signal::reports_current_omitted_required_work(lower);
    unresolved_signal || current_failure || concurrent_status_lost || omitted_required_work
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

/// Scan EVERY line of an extracted text for a hard-blocker marker. Extracted
/// tool/agent output may span multiple lines (content-block arrays joined with
/// newlines, multi-line string values); a `STATUS: BLOCKED` on a trailing line
/// must not be missed by a single start-anchored check (#2806 R8-F1).
fn text_indicates_blocked(text: &str) -> bool {
    text.lines().any(line_indicates_blocked)
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
    // Reuse the shared clause-scoped current-omission classifier so the summary
    // and per-line unconfirmed-gate paths cannot drift apart. Historical
    // narration ("previous turn omitted ...; this turn completed both") is
    // excluded clause-by-clause inside the helper (#2806 R9b, R14-F3).
    let required_work_omitted = gate_signal::reports_current_omitted_required_work(&lower);
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

/// Scan EVERY line of an extracted text for an unconfirmed-gate marker. Like
/// [`text_indicates_blocked`], this guards against hard markers hiding on a
/// trailing line of a multi-line extracted string (#2806 R8-F1).
fn text_indicates_unconfirmed_gate(text: &str) -> bool {
    text.lines().any(line_indicates_unconfirmed_gate)
}

#[cfg(test)]
#[path = "pipeline_post_exec_blocked_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "pipeline_post_exec_blocked_r8_tests.rs"]
mod r8_tests;

#[cfg(test)]
#[path = "pipeline_post_exec_blocked_r13_tests.rs"]
mod r13_tests;

#[cfg(test)]
#[path = "pipeline_post_exec_blocked_r14_tests.rs"]
mod r14_tests;

#[cfg(test)]
#[path = "pipeline_post_exec_blocked_r15_tests.rs"]
mod r15_tests;

#[cfg(test)]
#[path = "pipeline_post_exec_blocked_r16_tests.rs"]
mod r16_tests;
