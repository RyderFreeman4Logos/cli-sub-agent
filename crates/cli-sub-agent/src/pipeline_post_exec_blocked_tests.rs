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
}
