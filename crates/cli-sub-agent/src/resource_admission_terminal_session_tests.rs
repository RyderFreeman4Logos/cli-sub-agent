use super::*;

fn active_session(id: &str, last_accessed: DateTime<Utc>, memory_max_mb: u64) -> MetaSessionState {
    MetaSessionState {
        meta_session_id: id.to_string(),
        phase: SessionPhase::Active,
        last_accessed,
        sandbox_info: Some(SandboxInfo {
            mode: "cgroup".to_string(),
            memory_max_mb: Some(memory_max_mb),
            filesystem_mode: None,
            readonly_project_root: None,
            resource_resolution: None,
        }),
        ..Default::default()
    }
}

#[test]
fn terminal_sessions_do_not_create_a_false_active_session_upper() {
    let now = Utc::now();
    let sessions = vec![
        active_session("failed-one", now, 10_000),
        active_session("failed-two", now, 10_000),
    ];

    let memory = aggregate_active_session_memory(&sessions, "current", now, |_| {
        SessionMemorySample::Terminal
    });

    assert_eq!(memory.active_count, 0);
    assert_eq!(memory.sampled_count, 0);
    assert_eq!(memory.sampled_rss_mb, 0);
    assert_eq!(
        memory.projected_mb, 0,
        "terminal no-provider sessions must not create a false active-session upper=0MB"
    );
}
