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
        "Initial gate status was unknown; reran the gate and it now PASSes the gate.",
        true,
    ));
    assert!(!worker_output_indicates_blocked_with_receipt(
        r#"{"type":"item.completed","item":{"type":"agent_message","text":"Initial gate status was unknown; reran the gate and it now PASSes the gate."}}"#,
        "",
        "The retry succeeded.",
        true,
    ));
    for resolved_historical_message in [
        r#"{"agent_message":"Initial gate status was unknown; reran the gate and it now PASSes the gate."}"#,
        r#"{"agent_message":{"text":"Initial gate status was unknown; reran the gate and it now PASSes the gate."}}"#,
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
    // R6-002: unbound "now passed" / "reports passed" / "report passed" also
    // must not resolve an unresolved gate diagnostic.
    for message in [
        "Gate status unknown; I passed the logs to the maintainer.",
        "gate status unknown; parser passes data downstream",
        "Gate status unknown; I now passed the logs to the maintainer",
        "Gate status unknown; the runner reports passed for the log handoff",
        "Gate status unknown; the runner report passed the logs upstream",
        "gate passed, but tests and commit omitted",
    ] {
        assert!(
            !message_reports_gate_resolution(message),
            "bare/unbound prose must not resolve a gate: {message}"
        );
        let agent_message = format!(r#"{{"agent_message":"{message}"}}"#);
        assert!(
            worker_output_indicates_blocked_with_receipt(
                &agent_message,
                "",
                "The retry succeeded.",
                true,
            ),
            "current receipt + unbound pass prose must still block: {agent_message}"
        );
    }

    // Gate-bound resolution phrases remain valid suppressors.
    for message in [
        "Initial gate status was unknown; reran the gate and it now PASSes the gate.",
        "gate status unknown earlier; gate passed after retry",
        "previous gate status unknown; gate status: passed",
        "previous gate status unknown; status: success now",
        "prior gate status unknown; completed successfully on retry",
        "prior gate status unknown; result: pass on retry",
    ] {
        assert!(
            message_reports_gate_resolution(message),
            "gate-bound success must resolve: {message}"
        );
    }
}

// --- R11-F1 / R11-F2: positive_completion trailing-token scan + denylist ---

#[test]
fn r11_positive_completion_gate_object_prose_does_not_resolve() {
    // R11-F1: positive_completion phrases previously used a bare `contains()`
    // substring match, bypassing the trailing-token scan that gates pass
    // signals. Forged prose that embeds "gate success" mid-sentence (followed
    // by a gate-object noun like report/logs/downstream) forged a false
    // resolution. Each phrase is now matched as a contiguous token subsequence
    // AND gated by the same clause_is_terminal_anchored denylist.
    for message in [
        // "gate success report forwarded downstream" — trailing report/downstream
        "gate exit unavailable; gate success report forwarded downstream",
        // "gate success logs to maintainer" — trailing logs/to
        "gate unavailable; gate success logs to the maintainer",
        // "status: success report" — trailing report
        "gate unavailable; status: success report forwarded",
        // "status is success data" — trailing data
        "status is success data downstream",
        // "gate succeeded output" — trailing output
        "gate unavailable; gate succeeded output captured",
        // "completed successfully to" — trailing "to" (handoff particle)
        "completed successfully to downstream",
        // "completion succeeded report" — trailing report
        "completion succeeded report attached",
        // "now reports success downstream" — trailing downstream
        "now reports success downstream",
    ] {
        assert!(
            !message_reports_gate_resolution(message),
            "positive_completion phrase followed by a gate-object noun must NOT resolve (forged prose): {message}"
        );
    }
}

#[test]
fn r11_terminal_positive_completion_phrases_still_resolve() {
    // R11-F1 must not break genuine terminal positive-completion clauses.
    for message in [
        "gate status unknown earlier; gate success",
        "previous gate status unknown; gate succeeded",
        "prior gate status unknown; status: success now",
        "prior gate status unknown; status is success now",
        "prior gate status unknown; completed successfully on retry",
        "prior gate status unknown; completion succeeded",
        "prior gate status unknown; now reports success",
        "prior gate status unknown; successfully completed",
    ] {
        assert!(
            message_reports_gate_resolution(message),
            "terminal positive-completion clause must resolve: {message}"
        );
    }
}

#[test]
fn r11_gate_object_noun_in_trailing_tokens_rejects_pass() {
    // R11-F2: "gate"/"gates"/"completion" are gate-object nouns. A pass token
    // followed by any of these references a gate artifact, not an outcome.
    // "status is pass sanitized completion gate" must NOT resolve.
    for message in [
        "status is pass sanitized completion gate",
        "gate pass completion report",
        "status is pass gates downstream",
        "result is pass completion gate artifact",
    ] {
        assert!(
            !message_reports_gate_resolution(message),
            "pass token followed by gate/gates/completion must NOT resolve: {message}"
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

#[test]
fn claude_assistant_and_tool_use_envelopes_follow_codex_classification() {
    // R5-002: Claude stream-json assistant / tool_use must not be treated as
    // Raw full-line scans. Resolved historical assistant prose with a current
    // receipt is suppressed; tool_use input.command is provenance only.
    let resolved_assistant = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Initial gate status was unknown; reran the gate and it now PASSes the gate."}]}}"#;
    let resolved_assistant_message = r#"{"type":"assistant_message","text":"Initial gate status was unknown; reran the gate and it now PASSes the gate."}"#;
    for envelope in [resolved_assistant, resolved_assistant_message] {
        assert!(
            !worker_output_indicates_blocked_with_receipt(
                envelope,
                "",
                "The retry succeeded.",
                true,
            ),
            "Claude assistant resolved history + current receipt must not block: {envelope}"
        );
    }

    let tool_use_search = r#"{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"rg -n 'zsh: status unknown' crates"}}"#;
    let tool_call_search = r#"{"type":"tool_call","id":"toolu_2","name":"Bash","input":{"command":"grep -F 'bash: exit status unknown' logs"}}"#;
    for provenance in [tool_use_search, tool_call_search] {
        assert!(
            !worker_output_indicates_blocked_with_receipt(
                provenance,
                "",
                "The retry succeeded.",
                true,
            ),
            "Claude tool_use/tool_call command source must not worker-block: {provenance}"
        );
    }

    // Live Claude tool_result output still blocks.
    assert!(worker_output_indicates_blocked_with_receipt(
        r#"{"type":"tool_result","tool_use_id":"toolu_1","content":"zsh: status unknown"}"#,
        "",
        "The retry succeeded.",
        true,
    ));
}

#[test]
fn claude_tool_result_content_array_and_fallthrough_block() {
    // R7-001: Claude tool_result content blocks must extract text; empty/unparsed
    // content must not shadow a later output/text/result field.
    let content_blocks = r#"{"type":"tool_result","tool_use_id":"toolu_1","content":[{"type":"text","text":"zsh: status unknown"}]}"#;
    assert!(
        worker_output_indicates_blocked_with_receipt(
            content_blocks,
            "",
            "The retry succeeded.",
            true,
        ),
        "Claude tool_result content text blocks must block: {content_blocks}"
    );

    let empty_content_output_fallback = r#"{"type":"tool_result","tool_use_id":"toolu_1","content":"","output":"zsh: status unknown"}"#;
    assert!(
        worker_output_indicates_blocked_with_receipt(
            empty_content_output_fallback,
            "",
            "The retry succeeded.",
            true,
        ),
        "empty content must fall through to output: {empty_content_output_fallback}"
    );

    let unparsed_content_result_fallback = r#"{"type":"tool_call_result","tool_use_id":"toolu_2","content":{"nested":true},"result":"bash: exit status unknown"}"#;
    assert!(
        worker_output_indicates_blocked_with_receipt(
            unparsed_content_result_fallback,
            "",
            "The retry succeeded.",
            true,
        ),
        "unparsed content must fall through to result: {unparsed_content_result_fallback}"
    );

    let empty_content_array_text_fallback =
        r#"{"type":"tool_result","content":[],"text":"STATUS: BLOCKED — tool failed"}"#;
    assert!(
        worker_output_indicates_blocked_with_receipt(
            empty_content_array_text_fallback,
            "",
            "The retry succeeded.",
            true,
        ),
        "empty content array must fall through to text: {empty_content_array_text_fallback}"
    );
}

#[test]
fn claude_result_envelope_classifies_like_assistant_prose() {
    // R6-003: Claude terminal {"type":"result","result":"..."} must not fall
    // through to Raw. Resolved historical gate prose in the result text with a
    // current receipt must not re-block; error result envelopes stay fail-closed.
    let resolved_result = r#"{"type":"result","result":"Initial gate status was unknown; reran the gate and it now PASSes the gate.","subtype":"success","is_error":false}"#;
    let stream = [
        r#"{"type":"system","subtype":"init","session_id":"s1"}"#,
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Initial gate status was unknown; reran the gate and it now PASSes the gate."}]}}"#,
        resolved_result,
    ]
    .join("\n");
    assert!(
        !worker_output_indicates_blocked_with_receipt(&stream, "", "The retry succeeded.", true,),
        "Claude system→assistant→result stream with resolved history + current receipt must not block"
    );
    assert!(
        !worker_output_indicates_blocked_with_receipt(
            resolved_result,
            "",
            "The retry succeeded.",
            true,
        ),
        "Claude result envelope alone with resolved history + current receipt must not block"
    );

    // Error result envelopes remain fail-closed (not treated as suppressible prose).
    let error_result = r#"{"type":"result","result":"Initial gate status was unknown; reran the gate and it now PASSes the gate.","is_error":true}"#;
    assert!(
        worker_output_indicates_blocked_with_receipt(
            error_result,
            "",
            "The retry succeeded.",
            true,
        ),
        "Claude error result envelope must not use assistant suppression: {error_result}"
    );

    // R7-002: subtype prefixes (error_api/error_*) and non-bool is_error stay
    // fail-closed — do not treat as suppressible agent prose.
    for error_envelope in [
        r#"{"type":"result","result":"Initial gate status was unknown; reran the gate and it now PASSes the gate.","subtype":"error_api"}"#,
        r#"{"type":"result","result":"Initial gate status was unknown; reran the gate and it now PASSes the gate.","subtype":"error_max_turns"}"#,
        r#"{"type":"result","result":"Initial gate status was unknown; reran the gate and it now PASSes the gate.","is_error":"true"}"#,
        r#"{"type":"result","result":"Initial gate status was unknown; reran the gate and it now PASSes the gate.","is_error":1}"#,
    ] {
        assert!(
            worker_output_indicates_blocked_with_receipt(
                error_envelope,
                "",
                "The retry succeeded.",
                true,
            ),
            "Claude error-like result envelope must not use assistant suppression: {error_envelope}"
        );
    }
}
