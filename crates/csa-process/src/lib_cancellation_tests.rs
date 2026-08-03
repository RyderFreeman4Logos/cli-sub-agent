use crate::{
    ExecutionCancellation, SpawnOptions, StreamMode, spawn_tool_with_options,
    wait_and_capture_with_idle_timeout,
};
use std::{process::Command, time::Duration};
use tokio::time::timeout;

#[tokio::test]
async fn cancellation_notifies_waiter_when_cancelled_before_wait() {
    let cancellation = ExecutionCancellation::new();
    cancellation.cancel();

    timeout(Duration::from_millis(50), cancellation.cancelled())
        .await
        .expect("cancel-before-wait must not miss notification");
}

#[tokio::test]
async fn cancellation_notifies_waiter_when_cancelled_after_wait_begins() {
    let cancellation = ExecutionCancellation::new();
    let waiter = {
        let cancellation = cancellation.clone();
        tokio::spawn(async move { cancellation.cancelled().await })
    };

    tokio::task::yield_now().await;
    cancellation.cancel();
    timeout(Duration::from_millis(50), waiter)
        .await
        .expect("cancel-after-wait must notify")
        .expect("wait task must not panic");
}

#[tokio::test]
async fn retry_backoff_cancellation_prevents_legacy_and_acp_retry_spawns() {
    let cancellation = ExecutionCancellation::new();
    let legacy_attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(1));
    let acp_attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(1));
    let canceller = cancellation.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        canceller.cancel();
    });

    for attempts in [&legacy_attempts, &acp_attempts] {
        if !crate::retry_backoff_cancelled(Some(&cancellation), Duration::from_secs(30)).await {
            attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    assert_eq!(legacy_attempts.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(acp_attempts.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[cfg(unix)]
async fn assert_dead_or_zombie(pid: i32) {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
    assert!(
        stat.is_empty() || stat.split_whitespace().nth(2) == Some("Z"),
        "descendant must be terminated before return; stat={stat}"
    );
}

#[cfg(unix)]
async fn wait_for_fixture_pid(pid_file: &std::path::Path) -> i32 {
    for _ in 0..50 {
        if let Ok(pid) = std::fs::read_to_string(pid_file) {
            return pid.trim().parse().expect("numeric descendant pid");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("descendant publishes pid");
}

#[cfg(unix)]
#[tokio::test]
async fn cancellation_reaps_term_resistant_descendant() {
    let temp = tempfile::tempdir().expect("tempdir");
    let pid_file = temp.path().join("descendant.pid");
    let cancellation = ExecutionCancellation::new();
    let mut command = Command::new("/bin/sh");
    command.arg("-c").arg(format!(
        "trap '' TERM; sh -c 'trap \"\" TERM; echo $$ > \"{}\"; while :; do :; done' & while :; do :; done",
        pid_file.display()
    ));
    let child = spawn_tool_with_options(
        command.into(),
        None,
        SpawnOptions {
            cancellation: Some(cancellation.clone()),
            ..SpawnOptions::default()
        },
    )
    .await
    .expect("spawn fixture");
    let wait = wait_and_capture_with_idle_timeout(
        child,
        StreamMode::BufferOnly,
        Duration::from_secs(60),
        Duration::from_secs(60),
        Duration::from_millis(20),
        None,
        SpawnOptions {
            cancellation: Some(cancellation.clone()),
            ..SpawnOptions::default()
        },
        None,
    );
    for _ in 0..50 {
        if pid_file.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let pid = wait_for_fixture_pid(&pid_file).await;
    cancellation.cancel();
    tokio::time::timeout(Duration::from_secs(2), wait)
        .await
        .expect("cancellation must reap leader")
        .expect("wait result");
    assert_dead_or_zombie(pid).await;
}

#[cfg(unix)]
#[tokio::test]
async fn cancellation_after_both_pipes_close_reaps_term_resistant_descendant() {
    let temp = tempfile::tempdir().expect("tempdir");
    let pid_file = temp.path().join("descendant.pid");
    let cancellation = ExecutionCancellation::new();
    let mut command = Command::new("/bin/sh");
    command.arg("-c").arg(format!(
        "trap '' TERM; sh -c 'trap \"\" TERM; echo $$ > \"{}\"; while :; do :; done' & exec 1>&- 2>&-; while :; do :; done",
        pid_file.display()
    ));
    let child = spawn_tool_with_options(
        command.into(),
        None,
        SpawnOptions {
            cancellation: Some(cancellation.clone()),
            ..SpawnOptions::default()
        },
    )
    .await
    .expect("spawn fixture");
    let wait = wait_and_capture_with_idle_timeout(
        child,
        StreamMode::BufferOnly,
        Duration::from_secs(60),
        Duration::from_secs(60),
        Duration::from_millis(20),
        None,
        SpawnOptions {
            cancellation: Some(cancellation.clone()),
            ..SpawnOptions::default()
        },
        None,
    );
    let pid = wait_for_fixture_pid(&pid_file).await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    cancellation.cancel();
    tokio::time::timeout(Duration::from_secs(2), wait)
        .await
        .expect("cancellation must remain active after pipe EOF")
        .expect("wait result");
    assert_dead_or_zombie(pid).await;
}

#[cfg(unix)]
#[tokio::test]
async fn idle_timeout_after_both_pipes_close_reaps_term_resistant_descendant() {
    let temp = tempfile::tempdir().expect("tempdir");
    let pid_file = temp.path().join("descendant.pid");
    let mut command = Command::new("/bin/sh");
    command.arg("-c").arg(format!(
        "trap '' TERM; sh -c 'trap \"\" TERM; echo $$ > \"{}\"; while :; do :; done' & exec 1>&- 2>&-; while :; do :; done",
        pid_file.display()
    ));
    let child = spawn_tool_with_options(command.into(), None, SpawnOptions::default())
        .await
        .expect("spawn fixture");
    let pid = wait_for_fixture_pid(&pid_file).await;
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        wait_and_capture_with_idle_timeout(
            child,
            StreamMode::BufferOnly,
            Duration::from_millis(40),
            Duration::from_millis(40),
            Duration::from_millis(20),
            None,
            SpawnOptions::default(),
            None,
        ),
    )
    .await
    .expect("idle timeout must remain active after pipe EOF")
    .expect("wait result");
    assert_eq!(result.exit_code, 137);
    assert_dead_or_zombie(pid).await;
}
