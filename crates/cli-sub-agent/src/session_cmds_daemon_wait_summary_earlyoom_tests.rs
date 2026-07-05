use chrono::Utc;

use super::render_wait_result_summary;

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
fn compact_summary_includes_earlyoom_prefer_list_hint() {
    let temp = tempfile::tempdir().expect("tempdir");
    let now = Utc::now();
    let diagnostic = "CSA diagnostic: signal kill hint: earlyoom (termination_reason=daemon_sigterm, MemAvailable: 14242 MB / MemTotal: 31792 MB, earlyoom running, cgroup memory.events: unavailable at expected session scope). Re-dispatch when host memory frees. If earlyoom has a --prefer list that includes csa/cargo, consider removing it or raising the threshold (-m) to avoid premature kills.";
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
        kill_hint: Some("earlyoom".to_string()),
        last_item: None,
        fallback_chain: None,
        ..Default::default()
    };

    let summary = render_wait_result_summary(temp.path(), "01TESTEARLYOOM", &result);

    assert!(summary.contains("Kill hint: earlyoom"));
    assert!(summary.contains("termination_reason=daemon_sigterm"));
    assert!(summary.contains("earlyoom running"));
    assert!(summary.contains("--prefer list"));
}
