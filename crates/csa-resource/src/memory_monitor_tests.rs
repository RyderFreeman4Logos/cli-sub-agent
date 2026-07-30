use super::*;

fn test_soft_limit_diagnostic() -> MemorySoftLimitKillDiagnostic {
    MemorySoftLimitKillDiagnostic {
        kill_hint: MEMORY_SOFT_LIMIT_KILL_HINT.to_string(),
        signal: libc::SIGTERM,
        current_mb: 900,
        threshold_mb: 700,
        memory_max_mb: 1000,
        soft_limit_percent: 70,
        scope_name: "csa-codex-01J.scope".to_string(),
    }
}

#[test]
fn test_start_returns_none_for_zero_max() {
    let config = MemoryMonitorConfig {
        scope_name: "test.scope".to_string(),
        pgid: 1234,
        memory_max_bytes: 0,
        soft_limit_percent: 80,
        interval: Duration::from_secs(5),
        grace_period: Duration::from_secs(5),
        diagnostic_path: None,
    };
    assert!(start(config).is_none());
}

#[test]
fn test_start_returns_none_for_zero_percent() {
    let config = MemoryMonitorConfig {
        scope_name: "test.scope".to_string(),
        pgid: 1234,
        memory_max_bytes: 1024 * 1024 * 1024,
        soft_limit_percent: 0,
        interval: Duration::from_secs(5),
        grace_period: Duration::from_secs(5),
        diagnostic_path: None,
    };
    assert!(start(config).is_none());
}

#[test]
fn test_start_returns_none_for_over_100_percent() {
    let config = MemoryMonitorConfig {
        scope_name: "test.scope".to_string(),
        pgid: 1234,
        memory_max_bytes: 1024 * 1024 * 1024,
        soft_limit_percent: 101,
        interval: Duration::from_secs(5),
        grace_period: Duration::from_secs(5),
        diagnostic_path: None,
    };
    assert!(start(config).is_none());
}

#[test]
fn soft_limit_diagnostic_from_config_records_actionable_fields() {
    let config = MemoryMonitorConfig {
        scope_name: "csa-codex-01J.scope".to_string(),
        pgid: 1234,
        memory_max_bytes: 10 * 1024 * 1024,
        soft_limit_percent: 70,
        interval: Duration::from_secs(5),
        grace_period: Duration::from_secs(5),
        diagnostic_path: None,
    };

    let diagnostic =
        MemorySoftLimitKillDiagnostic::from_config(&config, 8 * 1024 * 1024, 7 * 1024 * 1024);

    assert_eq!(diagnostic.kill_hint, MEMORY_SOFT_LIMIT_KILL_HINT);
    assert_eq!(diagnostic.signal, libc::SIGTERM);
    assert_eq!(diagnostic.current_mb, 8);
    assert_eq!(diagnostic.threshold_mb, 7);
    assert_eq!(diagnostic.memory_max_mb, 10);
    assert_eq!(diagnostic.soft_limit_percent, 70);
    assert_eq!(diagnostic.scope_name, "csa-codex-01J.scope");
}

#[test]
fn records_soft_limit_diagnostic_without_writing_artifact() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join(MEMORY_SOFT_LIMIT_KILL_FILE_NAME);
    let diagnostic = MemorySoftLimitKillDiagnostic {
        kill_hint: MEMORY_SOFT_LIMIT_KILL_HINT.to_string(),
        signal: libc::SIGTERM,
        current_mb: 900,
        threshold_mb: 700,
        memory_max_mb: 1000,
        soft_limit_percent: 70,
        scope_name: "csa-codex-01J.scope".to_string(),
    };

    record_soft_limit_diagnostic(Some(&path), &diagnostic);

    let loaded = read_soft_limit_diagnostic(&path).expect("diagnostic should parse");
    assert_eq!(loaded, diagnostic);
    assert!(
        !path.exists(),
        "memory soft-limit registry evidence must not create a disk artifact"
    );
}

#[cfg(unix)]
#[test]
fn records_soft_limit_diagnostic_without_following_existing_symlink() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join(MEMORY_SOFT_LIMIT_KILL_FILE_NAME);
    let symlink_target = temp.path().join("would-be-clobbered.txt");
    let sentinel = "do not overwrite me";
    std::fs::write(&symlink_target, sentinel).expect("write symlink target");
    std::os::unix::fs::symlink(&symlink_target, &path).expect("create symlink");
    let diagnostic = test_soft_limit_diagnostic();

    record_soft_limit_diagnostic(Some(&path), &diagnostic);

    assert_eq!(
        read_soft_limit_diagnostic(&path),
        Some(diagnostic),
        "registry evidence should still be available for the current run"
    );
    assert_eq!(
        std::fs::read_to_string(&symlink_target).expect("read symlink target"),
        sentinel,
        "memory soft-limit recording must not follow or clobber a symlink path"
    );
}

#[test]
fn ignores_unregistered_soft_limit_diagnostic_artifact_file() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join(MEMORY_SOFT_LIMIT_KILL_FILE_NAME);
    let diagnostic = MemorySoftLimitKillDiagnostic {
        kill_hint: MEMORY_SOFT_LIMIT_KILL_HINT.to_string(),
        signal: libc::SIGTERM,
        current_mb: 900,
        threshold_mb: 700,
        memory_max_mb: 1000,
        soft_limit_percent: 70,
        scope_name: "csa-codex-01J.scope".to_string(),
    };
    std::fs::write(
        &path,
        toml::to_string_pretty(&diagnostic).expect("serialize"),
    )
    .expect("write forged artifact");

    assert!(
        read_soft_limit_diagnostic(&path).is_none(),
        "a TOML file alone is not authoritative CSA monitor evidence"
    );
}

#[test]
fn ignores_soft_limit_diagnostic_with_unexpected_hint() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join(MEMORY_SOFT_LIMIT_KILL_FILE_NAME);
    let diagnostic = MemorySoftLimitKillDiagnostic {
        kill_hint: "unknown_signal".to_string(),
        signal: libc::SIGTERM,
        current_mb: 900,
        threshold_mb: 700,
        memory_max_mb: 1000,
        soft_limit_percent: 70,
        scope_name: "csa-codex-01J.scope".to_string(),
    };
    record_soft_limit_diagnostic_evidence(&path, &diagnostic);

    assert!(read_soft_limit_diagnostic(&path).is_none());
}

#[test]
fn rejects_soft_limit_diagnostic_recorded_before_not_before_without_grace_window() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join(MEMORY_SOFT_LIMIT_KILL_FILE_NAME);
    let run_start = SystemTime::now();
    let diagnostic = test_soft_limit_diagnostic();

    record_soft_limit_diagnostic_evidence(&path, &diagnostic);

    assert_eq!(
        read_soft_limit_diagnostic_recorded_at_or_after(&path, Some(run_start)),
        Some(diagnostic.clone()),
        "evidence recorded after this run's start should remain authoritative"
    );
    let later_run_start = SystemTime::now()
        .checked_add(Duration::from_millis(500))
        .expect("later run start");
    assert!(
        read_soft_limit_diagnostic_recorded_at_or_after(&path, Some(later_run_start)).is_none(),
        "evidence recorded before a later run start must not be accepted by a grace window"
    );
}

#[test]
fn start_clears_stale_soft_limit_registry_when_monitor_disabled_without_touching_artifact() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join(MEMORY_SOFT_LIMIT_KILL_FILE_NAME);
    let diagnostic = test_soft_limit_diagnostic();
    record_soft_limit_diagnostic_evidence(&path, &diagnostic);
    let stale_contents = "kill_hint = \"memory_soft_limit\"\n";
    std::fs::write(&path, stale_contents).expect("write stale artifact");

    assert!(
        start(MemoryMonitorConfig {
            scope_name: "test.scope".to_string(),
            pgid: 1234,
            memory_max_bytes: 0,
            soft_limit_percent: 70,
            interval: Duration::from_secs(5),
            grace_period: Duration::from_secs(5),
            diagnostic_path: Some(path.clone()),
        })
        .is_none(),
        "zero memory limit should skip monitor setup"
    );

    assert_eq!(
        std::fs::read_to_string(&path).expect("stale artifact should be untouched"),
        stale_contents,
        "disabled monitor setup must not mutate disk artifacts"
    );
    assert!(
        read_soft_limit_diagnostic(&path).is_none(),
        "disabled monitor setup should still clear stale in-process evidence"
    );
}

#[tokio::test]
async fn start_clears_stale_soft_limit_registry_without_touching_artifact() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join(MEMORY_SOFT_LIMIT_KILL_FILE_NAME);
    let diagnostic = MemorySoftLimitKillDiagnostic {
        kill_hint: MEMORY_SOFT_LIMIT_KILL_HINT.to_string(),
        signal: libc::SIGTERM,
        current_mb: 900,
        threshold_mb: 700,
        memory_max_mb: 1000,
        soft_limit_percent: 70,
        scope_name: "csa-codex-01J.scope".to_string(),
    };
    record_soft_limit_diagnostic_evidence(&path, &diagnostic);
    let stale_contents = "kill_hint = \"memory_soft_limit\"\n";
    std::fs::write(&path, stale_contents).expect("write stale artifact");

    let handle = start(MemoryMonitorConfig {
        scope_name: "test.scope".to_string(),
        pgid: 1234,
        memory_max_bytes: 1024 * 1024 * 1024,
        soft_limit_percent: 70,
        interval: Duration::from_secs(3600),
        grace_period: Duration::from_secs(5),
        diagnostic_path: Some(path.clone()),
    })
    .expect("monitor should start");

    assert_eq!(
        std::fs::read_to_string(&path).expect("stale artifact should be untouched"),
        stale_contents,
        "start should clear stale registry evidence without mutating disk artifacts"
    );
    assert!(
        read_soft_limit_diagnostic(&path).is_none(),
        "start should clear stale in-process diagnostic evidence"
    );
    handle.stop().await;
}
