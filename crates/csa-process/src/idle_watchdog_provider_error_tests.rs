use super::*;

#[test]
fn transient_fatal_error_marker_retries_before_kill() {
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        tmp.path().join("stderr.log"),
        "provider failed: HTTP 429 Too Many Requests\n",
    )
    .expect("write stderr");

    let mut state = IdleWatchdogState::default();
    let mut last_activity = Instant::now() - FATAL_ERROR_PROGRESS_GRACE - Duration::from_secs(1);

    let first = should_terminate_for_idle_with_state(
        &mut last_activity,
        Duration::from_secs(7200),
        Duration::from_secs(600),
        Some(tmp.path()),
        &mut state,
        true,
    );

    assert_eq!(first, None);
    assert!(state.liveness_dead_since.is_none());
    let retry_after = state
        .next_liveness_poll_at
        .expect("transient marker schedules retry");
    assert!(retry_after > Instant::now());

    state.provider_error_backoff.retry_after = Some(Instant::now() - Duration::from_secs(1));
    state.next_liveness_poll_at = Some(Instant::now() - Duration::from_secs(1));

    let exhausted = should_terminate_for_idle_with_state(
        &mut last_activity,
        Duration::from_secs(7200),
        Duration::from_secs(600),
        Some(tmp.path()),
        &mut state,
        true,
    );

    assert_eq!(exhausted, Some(IdleTerminationReason::FatalError));
}

#[test]
fn progress_signal_clears_transient_provider_backoff() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let locks_dir = tmp.path().join("locks");
    std::fs::create_dir_all(&locks_dir).expect("create locks dir");
    std::fs::write(
        locks_dir.join("codex.lock"),
        format!("{{\"pid\": {}}}", std::process::id()),
    )
    .expect("write lock");
    std::fs::write(tmp.path().join("output.log"), "progress").expect("write output");
    std::fs::write(
        tmp.path().join(".liveness.snapshot"),
        "spool_bytes_written=8\nobserved_spool_bytes_written=0",
    )
    .expect("seed snapshot");

    let mut state = IdleWatchdogState {
        liveness_dead_since: Some(Instant::now() - Duration::from_secs(5)),
        next_liveness_poll_at: Some(Instant::now() - Duration::from_secs(1)),
        provider_error_backoff: ProviderErrorBackoff {
            retries_used: TRANSIENT_PROVIDER_ERROR_RETRY_BUDGET,
            retry_after: Some(Instant::now() + TRANSIENT_PROVIDER_ERROR_BACKOFF),
        },
    };
    let mut last_activity = Instant::now() - Duration::from_secs(10);

    let terminate = should_terminate_for_idle_with_state(
        &mut last_activity,
        Duration::from_secs(1),
        Duration::from_secs(1),
        Some(tmp.path()),
        &mut state,
        true,
    );

    assert_eq!(terminate, None);
    assert_eq!(
        state.provider_error_backoff,
        ProviderErrorBackoff::default()
    );
}
