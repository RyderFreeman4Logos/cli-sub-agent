use crate::{
    ExecutionCancellation, SpawnOptions, StreamMode, spawn_tool_with_options,
    wait_and_capture_with_idle_timeout,
};
use std::{process::Command, time::Duration};

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
    let pid: i32 = std::fs::read_to_string(&pid_file)
        .expect("descendant publishes pid")
        .trim()
        .parse()
        .expect("numeric descendant pid");
    cancellation.cancel();
    tokio::time::timeout(Duration::from_secs(2), wait)
        .await
        .expect("cancellation must reap leader")
        .expect("wait result");
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
    assert!(
        stat.is_empty() || stat.split_whitespace().nth(2) == Some("Z"),
        "descendant must be terminated before return; stat={stat}"
    );
}
