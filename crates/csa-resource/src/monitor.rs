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
