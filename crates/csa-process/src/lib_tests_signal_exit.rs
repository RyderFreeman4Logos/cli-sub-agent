use super::*;

#[cfg(unix)]
#[tokio::test]
async fn wait_and_capture_preserves_signal_exit_diagnostics() {
    let mut cmd = Command::new("bash");
    cmd.args(["-c", "kill -KILL $$"]);

    let child = spawn_tool(cmd, None).await.expect("Failed to spawn");
    let result = wait_and_capture(child, StreamMode::BufferOnly)
        .await
        .expect("Failed to wait for signaled child");

    assert_eq!(result.exit_code, 137);
    assert_eq!(result.raw_process_exit_code, Some(137));
    assert_eq!(result.exit_signal, Some(9));
    assert_eq!(result.terminal_reason.as_deref(), Some("signal"));
    assert_eq!(result.model_completed, Some(false));
    assert!(result.summary.contains("signal 9"));
    assert!(result.stderr_output.contains("SIGKILL"));
}
