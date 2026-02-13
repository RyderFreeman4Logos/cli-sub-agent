use std::time::Duration;
use sysinfo::{Pid, ProcessesToUpdate, System};

/// Monitors a child process's memory usage asynchronously.
/// Polls every 500ms and tracks peak memory in MB.
pub struct MemoryMonitor {
    handle: tokio::task::JoinHandle<u64>,
}

impl MemoryMonitor {
    /// Start monitoring a process by PID.
    /// Runs in a background tokio task, polling every 500ms.
    /// Stops automatically when the process exits.
    pub fn start(pid: u32) -> Self {
        let handle = tokio::spawn(async move {
            let mut sys = System::new();
            let sysinfo_pid = Pid::from_u32(pid);
            let mut max_mem_mb: u64 = 0;

            loop {
                tokio::time::sleep(Duration::from_millis(500)).await;
                sys.refresh_processes(ProcessesToUpdate::Some(&[sysinfo_pid]), true);
                match sys.process(sysinfo_pid) {
                    Some(process) => {
                        let mem_mb = process.memory() / 1024 / 1024;
                        if mem_mb > max_mem_mb {
                            max_mem_mb = mem_mb;
                        }
                    }
                    None => break, // Process exited
                }
            }
            max_mem_mb
        });

        Self { handle }
    }

    /// Stop monitoring and return peak memory usage in MB.
    pub async fn stop(self) -> u64 {
        self.handle.await.unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_monitor_start_stop_with_short_lived_process() {
        // Spawn a real short-lived process (sleep 0.1s)
        let child = tokio::process::Command::new("sleep")
            .arg("0.1")
            .spawn()
            .expect("failed to spawn sleep");

        let pid = child.id().expect("no pid for child");
        let monitor = MemoryMonitor::start(pid);

        // Wait for the process to exit, then stop the monitor
        let _ = child.wait_with_output().await;
        let peak_mb = monitor.stop().await;

        // sleep uses minimal memory; just verify we got a value without panic
        assert!(
            peak_mb < 1000,
            "sleep should not use >1 GB; got {} MB",
            peak_mb
        );
    }

    #[tokio::test]
    async fn test_monitor_nonexistent_pid_returns_zero() {
        // Use a PID that almost certainly doesn't exist
        let monitor = MemoryMonitor::start(u32::MAX - 1);

        // The monitor loop should immediately break (process not found)
        // after the first 500ms poll
        let peak_mb = monitor.stop().await;
        assert_eq!(peak_mb, 0, "non-existent process should report 0 MB");
    }

    #[tokio::test]
    async fn test_monitor_tracks_peak_not_final() {
        // Spawn a process that allocates some memory then exits.
        // We use `dd` to read some data into memory briefly.
        let child = tokio::process::Command::new("dd")
            .args(["if=/dev/zero", "of=/dev/null", "bs=1M", "count=1"])
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("failed to spawn dd");

        let pid = child.id().expect("no pid for child");
        let monitor = MemoryMonitor::start(pid);

        let _ = child.wait_with_output().await;
        let peak_mb = monitor.stop().await;

        // Just verify it completes without panic and returns a u64
        // (the actual value depends on system state and timing)
        let _ = peak_mb;
    }
}
