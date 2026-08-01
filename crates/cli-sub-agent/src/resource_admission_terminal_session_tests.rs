use super::*;
#[cfg(target_os = "linux")]
use crate::test_session_sandbox::ScopedSessionSandbox;
#[cfg(target_os = "linux")]
use tempfile::tempdir;

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

#[test]
fn live_unsampleable_session_uses_fallback_and_counts_for_balloon() {
    let now = Utc::now();
    let sessions = vec![MetaSessionState {
        meta_session_id: "live".to_string(),
        phase: SessionPhase::Active,
        last_accessed: now - TimeDelta::hours(2),
        ..Default::default()
    }];

    let memory = aggregate_active_session_memory(&sessions, "current", now, |_| {
        SessionMemorySample::UnavailableLiveProcess
    });
    let count = count_observable_active_sessions(&sessions, "current", now, |_| {
        SessionMemorySample::UnavailableLiveProcess
    });

    assert_eq!(memory.active_count, 1);
    assert_eq!(memory.projected_mb, FALLBACK_SPAWN_PROJECTION_MB);
    assert_eq!(count, 1);
}

#[cfg(target_os = "linux")]
fn daemon_pid_record(pid: u32) -> String {
    let content = std::fs::read_to_string(format!("/proc/{pid}/stat")).expect("read /proc stat");
    let close_paren = content.rfind(')').expect("stat comm terminator");
    let mut fields = content[close_paren + 1..].split_whitespace();
    fields.next().expect("state");
    fields.next().expect("ppid");
    fields.next().expect("pgrp");
    for _ in 0..16 {
        fields.next().expect("intermediate stat field");
    }
    let start_time = fields.next().expect("start time");
    format!("{pid} {start_time}\n")
}

#[cfg(target_os = "linux")]
#[test]
fn result_with_live_daemon_remains_charged_for_memory_admission() {
    let td = tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new_blocking(&td);
    let project = td.path();
    let mut session = csa_session::create_session(project, Some("live-result"), None, None)
        .expect("create session");
    session.phase = SessionPhase::Active;
    session.sandbox_info = Some(SandboxInfo {
        mode: "cgroup".to_string(),
        memory_max_mb: Some(10_000),
        filesystem_mode: None,
        readonly_project_root: None,
        resource_resolution: None,
    });
    let session_dir =
        csa_session::get_session_dir(project, &session.meta_session_id).expect("session dir");
    let now = Utc::now();
    std::fs::write(
        session_dir.join("result.toml"),
        format!(
            "status = \"failure\"\nexit_code = 1\nsummary = \"intermediate result\"\ntool = \"codex\"\nstarted_at = \"{}\"\ncompleted_at = \"{}\"\n",
            now.to_rfc3339(),
            now.to_rfc3339(),
        ),
    )
    .expect("write result");
    assert!(
        csa_session::load_result(project, &session.meta_session_id)
            .expect("load result")
            .is_some(),
        "test setup requires a persisted terminal result"
    );

    let mut child = std::process::Command::new("sleep")
        .arg("60")
        .spawn()
        .expect("spawn daemon stand-in");
    std::fs::write(
        session_dir.join("daemon.pid"),
        daemon_pid_record(child.id()),
    )
    .expect("write daemon pid");
    assert!(
        csa_process::ToolLiveness::daemon_pid_is_alive(&session_dir),
        "test setup requires a live daemon signal"
    );

    let memory =
        aggregate_active_session_memory(&[session.clone()], "current", now, sample_session_memory);

    child.kill().ok();
    child.wait().ok();

    assert_eq!(memory.active_count, 1);
    assert_eq!(memory.projected_mb, 10_000);
}
