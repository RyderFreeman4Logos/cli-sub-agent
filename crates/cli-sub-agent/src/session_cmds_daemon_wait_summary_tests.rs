use chrono::Utc;

use super::{
    WAIT_OUTPUT_MAX_BYTES, read_wait_output_log, render_wait_output_log, render_wait_result_json,
    render_wait_result_summary,
};

#[path = "session_cmds_daemon_wait_summary_kill_tests.rs"]
mod kill_diagnostics;
#[path = "session_cmds_daemon_wait_summary_provider_quota_tests.rs"]
mod provider_quota;
#[path = "session_cmds_daemon_wait_summary_recovery_tests.rs"]
mod recovery;
#[path = "session_cmds_daemon_wait_summary_unavailable_tests.rs"]
mod unavailable_reason;

#[test]
fn read_wait_output_log_tails_large_stdout_without_loading_prefix() {
    let temp = tempfile::tempdir().expect("tempdir");
    let stdout_log = temp.path().join("stdout.log");
    let prefix = vec![b'a'; WAIT_OUTPUT_MAX_BYTES as usize];
    let suffix = b"\nfinal visible line\n";
    let mut content = prefix;
    content.extend_from_slice(suffix);
    std::fs::write(&stdout_log, content).expect("stdout log should be written");

    let log = read_wait_output_log(&stdout_log).expect("stdout log should be read");

    assert!(log.truncated);
    assert!(log.raw.len() <= WAIT_OUTPUT_MAX_BYTES as usize);
    let rendered = String::from_utf8(log.raw).expect("tail should be valid utf-8");
    assert_eq!(rendered, "final visible line\n");
}

#[test]
fn render_truncated_codex_json_tail_filters_agent_messages() {
    let raw = [
        r#"{"type":"item.completed","item":{"type":"tool_result","text":"hidden shell output"}}"#,
        r#"{"type":"item.completed","item":{"type":"agent_message","text":"visible summary"}}"#,
    ]
    .join("\n");

    let rendered = render_wait_output_log(raw.as_bytes(), true)
        .expect("truncated codex transcript should render");

    assert_eq!(rendered, "visible summary");
    assert!(!rendered.contains("hidden shell output"));
}

#[test]
fn compact_summary_includes_usage_and_review_verdict() {
    let temp = tempfile::tempdir().expect("tempdir");
    let output_dir = temp.path().join("output");
    std::fs::create_dir_all(&output_dir).expect("output dir should be created");
    std::fs::write(
        output_dir.join("review-verdict.json"),
        r#"{"schema_version":1,"session_id":"01TESTWAITSUMMARY","timestamp":"2026-04-01T00:00:00Z","decision":"pass","verdict_legacy":"CLEAN","severity_counts":{"critical":0,"high":0,"medium":0,"low":0},"prior_round_refs":[]}"#,
    )
    .expect("review verdict should be written");
    std::fs::write(
        temp.path().join("review_meta.json"),
        r#"{
  "session_id": "01TESTWAITSUMMARY",
  "head_sha": "deadbeef",
  "decision": "pass",
  "verdict": "CLEAN",
  "tool": "codex",
  "scope": "range:main...HEAD",
  "exit_code": 0,
  "fix_attempted": false,
  "fix_rounds": 0,
  "timestamp": "2026-04-01T00:00:00Z"
}"#,
    )
    .expect("review meta should be written");
    let now = Utc::now();
    let result = csa_session::SessionResult {
        post_exec_gate: None,
        status: "success".to_string(),
        exit_code: 0,
        summary: r#"{"type":"turn.completed","usage":{"input_tokens":100,"cached_input_tokens":40,"output_tokens":25}}"#.to_string(),
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now + chrono::TimeDelta::seconds(65),
        events_count: 0,
        artifacts: Vec::new(),
        ..Default::default()
    };

    let summary = render_wait_result_summary(temp.path(), "01TESTWAITSUMMARY", &result);

    assert!(summary.len() <= 2048);
    assert!(summary.contains("Session: 01TESTWAITSUMMARY"));
    assert!(summary.contains("Elapsed: 1m 5s"));
    assert!(summary.contains(
        "Tokens: input=100, output=25, total=125, cache_read=40, uncached=60, cache=40%"
    ));
    assert!(summary.contains("Review verdict: PASS"));
}

#[test]
fn compact_json_includes_token_cache_derived_fields() {
    let temp = tempfile::tempdir().expect("tempdir");
    let now = Utc::now();
    let result = csa_session::SessionResult {
        post_exec_gate: None,
        status: "success".to_string(),
        exit_code: 0,
        summary: r#"{"type":"turn.completed","usage":{"input_tokens":100,"cached_input_tokens":40,"output_tokens":25}}"#.to_string(),
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now,
        events_count: 0,
        artifacts: Vec::new(),
        ..Default::default()
    };

    let rendered = render_wait_result_json(temp.path(), "01TESTWAITJSON", &result)
        .expect("wait result JSON should render");
    let value: serde_json::Value =
        serde_json::from_str(&rendered).expect("wait result JSON should parse");

    assert_eq!(value["tokens"]["cache_read_input_tokens"], 40);
    assert_eq!(value["tokens"]["uncached_input_tokens"], 60);
    assert_eq!(value["tokens"]["cache_read_ratio"], serde_json::json!(0.4));
}

#[test]
fn compact_summary_includes_nested_input_cache_details() {
    let temp = tempfile::tempdir().expect("tempdir");
    let now = Utc::now();
    let result = csa_session::SessionResult {
        post_exec_gate: None,
        status: "success".to_string(),
        exit_code: 0,
        summary: r#"{"usage":{"input_tokens":100,"input_tokens_details":{"cached_tokens":40},"output_tokens":25,"output_tokens_details":{"reasoning_tokens":5}}}"#.to_string(),
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now,
        events_count: 0,
        artifacts: Vec::new(),
        ..Default::default()
    };

    let summary = render_wait_result_summary(temp.path(), "01TESTWAITNESTED", &result);

    assert!(summary.contains(
        "Tokens: input=100, output=25, reasoning_output=5, total=125, cache_read=40, uncached=60, cache=40%"
    ));
}

#[test]
fn compact_summary_prefers_post_exec_gate_failure_over_success_markdown() {
    let temp = tempfile::tempdir().expect("tempdir");
    let output_dir = temp.path().join("output");
    std::fs::create_dir_all(&output_dir).expect("output dir should be created");
    std::fs::write(
        output_dir.join("summary.md"),
        "Fixed and committed the remaining medium finding. Did not push; working tree clean.",
    )
    .expect("success-looking summary should be written");
    std::fs::write(
        temp.path().join(csa_session::GATE_FAILURE_LOG_REL_PATH),
        "FAIL [   0.005s] cli_sub_agent::gate::fails\nerror: Recipe `test` failed on line 42 with exit code 100\n",
    )
    .expect("gate failure log should be written");

    let now = Utc::now();
    let report = csa_session::PostExecGateReport::from_redacted_gate_output(
        "post-exec gate",
        1,
        "FAIL [   0.005s] cli_sub_agent::gate::fails\nerror: Recipe `test` failed on line 42 with exit code 100\n",
    );
    let result = csa_session::SessionResult {
        post_exec_gate: Some(report),
        status: "failure".to_string(),
        exit_code: 1,
        summary: "POST-EXEC GATE FAILED (exit=1, step=just test)".to_string(),
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now,
        events_count: 0,
        artifacts: vec![csa_session::SessionArtifact::new(
            csa_session::GATE_FAILURE_LOG_REL_PATH,
        )],
        ..Default::default()
    };

    let summary = render_wait_result_summary(temp.path(), "01TESTWAITGATE", &result);

    assert!(summary.contains("Post-exec gate: failed"));
    assert!(summary.contains("step=just test"));
    assert!(summary.contains(csa_session::GATE_FAILURE_LOG_REL_PATH));
    assert!(summary.contains("Summary: POST-EXEC GATE FAILED"));
    assert!(
        !summary.contains("working tree clean"),
        "wait summary must not show child success markdown as authoritative: {summary}"
    );
}

#[test]
fn compact_summary_includes_unknown_signal_evidence() {
    let temp = tempfile::tempdir().expect("tempdir");
    let now = Utc::now();
    let diagnostic = "CSA diagnostic: signal kill hint: unknown_signal (termination_reason=sigterm, MemAvailable: 12000 MB / MemTotal: 16000 MB, earlyoom not running, cgroup memory.events oom=0 oom_kill=0). No timeout or cgroup OOM evidence was found, and memory checks did not identify a concrete kill source; reason remains unknown.";
    let result = csa_session::SessionResult {
        post_exec_gate: None,
        status: "signal".to_string(),
        exit_code: 143,
        summary: diagnostic.to_string(),
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now,
        events_count: 0,
        artifacts: Vec::new(),
        peak_memory_mb: None,
        kill_hint: Some("unknown_signal".to_string()),
        last_item: None,
        fallback_chain: None,
        ..Default::default()
    };

    let summary = render_wait_result_summary(temp.path(), "01TESTWAITUNKNOWN", &result);

    assert!(summary.contains("Kill hint: unknown_signal"));
    assert!(summary.contains("termination_reason=sigterm"));
    assert!(summary.contains("cgroup memory.events oom=0 oom_kill=0"));
    assert!(summary.contains("reason remains unknown"));
}

#[test]
fn compact_summary_includes_csa_timeout_effective_timeout_details() {
    let temp = tempfile::tempdir().expect("tempdir");
    let now = Utc::now();
    let diagnostic = "CSA diagnostic: signal kill hint: csa_timeout (termination_reason=initial_response_timeout, CSA supervisor timeout metadata matched signal exit, requested_timeout_seconds=10800, effective_timeout_kind=initial_response_timeout, effective_timeout_seconds=45, effective_timeout_source=initial_response_timeout, idle_timeout_seconds=10800, initial_response_timeout_seconds=45). The recorded timeout is the concrete kill reason.";
    let result = csa_session::SessionResult {
        post_exec_gate: None,
        status: "signal".to_string(),
        exit_code: 137,
        summary: diagnostic.to_string(),
        tool: "gemini-cli".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now + chrono::TimeDelta::seconds(47),
        events_count: 0,
        artifacts: Vec::new(),
        peak_memory_mb: None,
        kill_hint: Some("csa_timeout".to_string()),
        last_item: None,
        fallback_chain: None,
        ..Default::default()
    };

    let summary = render_wait_result_summary(temp.path(), "01TESTWAITTIMEOUT", &result);

    assert!(summary.contains("Kill hint: csa_timeout"));
    assert!(summary.contains("termination_reason=initial_response_timeout"));
    assert!(summary.contains("requested_timeout_seconds=10800"));
    assert!(summary.contains("effective_timeout_seconds=45"));
    assert!(summary.contains("effective_timeout_source=initial_response_timeout"));
}

#[test]
fn compact_summary_labels_fix_loop_noop_from_review_meta() {
    let temp = tempfile::tempdir().expect("tempdir");
    let output_dir = temp.path().join("output");
    std::fs::create_dir_all(&output_dir).expect("output dir should be created");
    std::fs::write(
        output_dir.join("review-verdict.json"),
        r#"{"schema_version":1,"session_id":"01TESTWAITNOOP","timestamp":"2026-04-01T00:00:00Z","decision":"fail","verdict_legacy":"HAS_ISSUES","severity_counts":{"critical":0,"high":0,"medium":1,"low":0},"failure_reason":"fix_loop_noop:head_unchanged_worktree_clean","prior_round_refs":[]}"#,
    )
    .expect("review verdict should be written");
    std::fs::write(
        temp.path().join("review_meta.json"),
        r#"{
  "session_id": "01TESTWAITNOOP",
  "head_sha": "deadbeef",
  "decision": "fail",
  "verdict": "HAS_ISSUES",
  "failure_reason": "fix_loop_noop:head_unchanged_worktree_clean",
  "tool": "codex",
  "scope": "range:main...HEAD",
  "exit_code": 1,
  "fix_attempted": true,
  "fix_rounds": 1,
  "timestamp": "2026-04-01T00:00:00Z",
  "fix_convergence": {
    "quality_gate_passed": true,
    "fix_output_was_substantive": true,
    "post_consistency_decision": "fail",
    "reached_genuine_clean_convergence": false,
    "terminal_reason": "fix_loop_noop:head_unchanged_worktree_clean"
  }
}"#,
    )
    .expect("review meta should be written");
    let now = Utc::now();
    let result = csa_session::SessionResult {
        post_exec_gate: None,
        status: "failure".to_string(),
        exit_code: 1,
        summary: "fix loop did not engage: head_unchanged_worktree_clean".to_string(),
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now + chrono::TimeDelta::seconds(65),
        events_count: 0,
        artifacts: Vec::new(),
        peak_memory_mb: None,
        kill_hint: None,
        kill_diagnostics: None,
        last_item: None,
        fallback_chain: None,
        gate_timeout: false,
        warnings: vec!["fix loop did not engage: head_unchanged_worktree_clean".to_string()],
        raw_process_exit_code: None,
        uncommitted_changes: None,
        large_diff_warning: None,
        require_commit_recovery: None,
        memory_soft_limit_recovery: None,
        manager_fields: Default::default(),
    };

    let summary = render_wait_result_summary(temp.path(), "01TESTWAITNOOP", &result);

    assert!(summary.contains("Review verdict: FIX-LOOP-NO-OP (head_unchanged_worktree_clean)"));
    assert!(summary.contains("Warning: fix loop did not engage: head_unchanged_worktree_clean"));
    assert!(summary.contains("Summary: fix loop did not engage: head_unchanged_worktree_clean"));
}

#[test]
fn compact_summary_uses_primary_failure_and_opaque_total_exhaustion_failover() {
    let temp = tempfile::tempdir().expect("tempdir");
    let output_dir = temp.path().join("output");
    std::fs::create_dir_all(&output_dir).expect("output dir should be created");
    std::fs::write(
        output_dir.join("review-verdict.json"),
        r#"{"schema_version":1,"session_id":"01TESTWAITQUOTA","timestamp":"2026-04-01T00:00:00Z","decision":"unavailable","verdict_legacy":"UNAVAILABLE","severity_counts":{"critical":0,"high":0,"medium":0,"low":0},"primary_failure":"auth_unavailable; HTTP 429","failure_reason":"all tier-4-critical models failed: gemini-cli/google/gemini-3.1-pro-preview/xhigh=auth_unavailable, codex/openai/gpt-5.5/xhigh=HTTP 429; earliest_reset=13h 58m","prior_round_refs":[]}"#,
    )
    .expect("review verdict should be written");
    let now = Utc::now();
    let result = csa_session::SessionResult {
        post_exec_gate: None,
        status: "failed".to_string(),
        exit_code: 1,
        summary: "review unavailable".to_string(),
        tool: "codex".to_string(),
        original_tool: Some("gemini-cli".to_string()),
        fallback_tool: Some("codex".to_string()),
        fallback_reason: Some("429_quota_exhausted".to_string()),
        started_at: now,
        completed_at: now + chrono::TimeDelta::seconds(65),
        events_count: 0,
        artifacts: Vec::new(),
        peak_memory_mb: None,
        kill_hint: None,
        last_item: None,
        fallback_chain: Some(vec![
            csa_core::types::FallbackAttempt {
                tool: "gemini-cli".to_string(),
                model_spec: Some("gemini-cli/google/gemini-3.1-pro-preview/xhigh".to_string()),
                skip_reason: "attempted-and-errored".to_string(),
                quota_exhausted: false,
                timestamp: now,
            },
            csa_core::types::FallbackAttempt {
                tool: "codex".to_string(),
                model_spec: Some("codex/openai/gpt-5.5/xhigh".to_string()),
                skip_reason: "rate-limit-429".to_string(),
                quota_exhausted: false,
                timestamp: now,
            },
        ]),
        ..Default::default()
    };

    let summary = render_wait_result_summary(temp.path(), "01TESTWAITQUOTA", &result);

    assert!(summary.contains("Review verdict: UNAVAILABLE (auth_unavailable; HTTP 429)"));
    assert!(summary.contains(
        "Failover: review unavailable: all tier-4-critical backends rate-limited; earliest reset ~13h 58m"
    ));
    assert!(!summary.contains("attempted-and-errored"));
    assert!(!summary.contains("API Key"));
}

#[test]
fn compact_summary_prints_pass_from_canonical_artifact_when_result_succeeded() {
    let temp = tempfile::tempdir().expect("tempdir");
    let output_dir = temp.path().join("output");
    std::fs::create_dir_all(&output_dir).expect("output dir should be created");
    std::fs::write(
        output_dir.join("review-verdict.json"),
        r#"{"schema_version":1,"session_id":"01TESTWAITARTPASS","timestamp":"2026-04-01T00:00:00Z","decision":"pass","verdict_legacy":"CLEAN","severity_counts":{"critical":0,"high":0,"medium":0,"low":0},"prior_round_refs":[]}"#,
    )
    .expect("review verdict should be written");
    let now = Utc::now();
    let result = csa_session::SessionResult {
        post_exec_gate: None,
        status: "success".to_string(),
        exit_code: 0,
        summary: "review complete".to_string(),
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now + chrono::TimeDelta::seconds(65),
        events_count: 0,
        artifacts: Vec::new(),
        ..Default::default()
    };

    let summary = render_wait_result_summary(temp.path(), "01TESTWAITARTPASS", &result);

    assert!(summary.contains("Review verdict: PASS"));
}

#[test]
fn compact_summary_includes_writer_uncommitted_warning() {
    let temp = tempfile::tempdir().expect("tempdir");
    let now = Utc::now();
    let result = csa_session::SessionResult {
        post_exec_gate: None,
        status: "success".to_string(),
        exit_code: 0,
        summary: "done".to_string(),
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now + chrono::TimeDelta::seconds(65),
        events_count: 0,
        artifacts: Vec::new(),
        peak_memory_mb: None,
        kill_hint: None,
        kill_diagnostics: None,
        last_item: None,
        fallback_chain: None,
        gate_timeout: false,
        warnings: Vec::new(),
        raw_process_exit_code: None,
        uncommitted_changes: Some(csa_session::UncommittedChanges {
            file_count: 7,
            insertions: 240,
            deletions: 12,
            approx_diff_tokens: 1_024,
            files: vec!["src/lib.rs".to_string()],
            truncated: 6,
        }),
        large_diff_warning: None,
        require_commit_recovery: None,
        memory_soft_limit_recovery: None,
        manager_fields: Default::default(),
    };

    let summary = render_wait_result_summary(temp.path(), "01TESTWAITDIRTY", &result);

    assert!(summary.contains(
        "⚠ writer session ended with 7 uncommitted files (+240/-12) — work NOT committed"
    ));
    assert!(
        !summary.contains("CSA:LARGE_DIFF_WARNING"),
        "small/unspecified large-diff warning must stay absent"
    );
}

#[test]
fn compact_summary_includes_large_diff_warning_block() {
    let temp = tempfile::tempdir().expect("tempdir");
    let now = Utc::now();
    let warning = csa_session::LargeDiffWarningReport {
        changed_files: 9,
        changed_lines: 1_420,
        approx_diff_tokens: 18_000,
    };
    let result = csa_session::SessionResult {
        post_exec_gate: None,
        status: "success".to_string(),
        exit_code: 0,
        summary: "done".to_string(),
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now + chrono::TimeDelta::seconds(65),
        events_count: 0,
        artifacts: Vec::new(),
        large_diff_warning: Some(warning.clone()),
        ..Default::default()
    };

    let summary = render_wait_result_summary(temp.path(), "01TESTWAITLARGEDIFF", &result);

    assert!(summary.contains(&crate::run_cmd::format_large_diff_warning_block(&warning)));
}

#[test]
fn compact_summary_does_not_print_pass_when_result_failed() {
    let temp = tempfile::tempdir().expect("tempdir");
    let output_dir = temp.path().join("output");
    std::fs::create_dir_all(&output_dir).expect("output dir should be created");
    std::fs::write(
        output_dir.join("review-verdict.json"),
        r#"{"schema_version":1,"session_id":"01TESTWAITFAILPASS","timestamp":"2026-04-01T00:00:00Z","decision":"pass","verdict_legacy":"CLEAN","severity_counts":{"critical":0,"high":0,"medium":0,"low":0},"prior_round_refs":[]}"#,
    )
    .expect("review verdict should be written");
    let now = Utc::now();
    let result = csa_session::SessionResult {
        post_exec_gate: None,
        status: "failed".to_string(),
        exit_code: 137,
        summary: "fatal backend error: process killed".to_string(),
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now + chrono::TimeDelta::seconds(65),
        events_count: 0,
        artifacts: Vec::new(),
        ..Default::default()
    };

    let summary = render_wait_result_summary(temp.path(), "01TESTWAITFAILPASS", &result);

    assert!(!summary.contains("Review verdict: PASS"));
    assert!(summary.contains("Review verdict: UNAVAILABLE"));
    assert!(summary.contains("Summary: fatal backend error: process killed"));
}

#[test]
fn compact_summary_does_not_print_pass_for_failed_fix_convergence() {
    let temp = tempfile::tempdir().expect("tempdir");
    let output_dir = temp.path().join("output");
    std::fs::create_dir_all(&output_dir).expect("output dir should be created");
    std::fs::write(
        output_dir.join("review-verdict.json"),
        r#"{"schema_version":1,"session_id":"01TESTWAITFAILED","timestamp":"2026-04-01T00:00:00Z","decision":"pass","verdict_legacy":"CLEAN","severity_counts":{"critical":0,"high":0,"medium":0,"low":0},"prior_round_refs":[]}"#,
    )
    .expect("review verdict should be written");
    std::fs::write(
        temp.path().join("review_meta.json"),
        r#"{
  "session_id": "01TESTWAITFAILED",
  "head_sha": "deadbeef",
  "decision": "pass",
  "verdict": "CLEAN",
  "failure_reason": "fix_non_convergence:quality_gate_failed",
  "tool": "codex",
  "scope": "range:main...HEAD",
  "exit_code": 1,
  "fix_attempted": true,
  "fix_rounds": 3,
  "fix_convergence": {
    "quality_gate_passed": false,
    "fix_output_was_substantive": true,
    "post_consistency_decision": "fail",
    "reached_genuine_clean_convergence": false,
    "terminal_reason": "quality_gate_failed"
  },
  "timestamp": "2026-04-01T00:00:00Z"
}"#,
    )
    .expect("review meta should be written");
    let now = Utc::now();
    let result = csa_session::SessionResult {
        post_exec_gate: None,
        status: "failed".to_string(),
        exit_code: 1,
        summary: "fix did not converge".to_string(),
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now + chrono::TimeDelta::seconds(65),
        events_count: 0,
        artifacts: Vec::new(),
        ..Default::default()
    };

    let summary = render_wait_result_summary(temp.path(), "01TESTWAITFAILED", &result);

    assert!(!summary.contains("Review verdict: PASS"));
    assert!(summary.contains("Review verdict: UNAVAILABLE"));
}
