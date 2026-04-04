//! Background memory monitor for cgroup scopes.
//!
//! Polls `MemoryCurrent` at a configurable interval and sends SIGTERM to the
//! process group when usage exceeds `soft_limit_percent` of `MemoryMax`.
//! After a grace period, escalates to SIGKILL.

use std::time::Duration;

use tokio::sync::watch;
use tracing::{debug, info, warn};

/// Configuration for the memory soft-limit monitor.
#[derive(Debug, Clone)]
pub struct MemoryMonitorConfig {
    /// Scope name to monitor (e.g. `csa-claude-code-01J....scope`).
    pub scope_name: String,
    /// Process group ID (negative PID) to signal.
    pub pgid: i32,
    /// Memory limit in bytes (MemoryMax).
    pub memory_max_bytes: u64,
    /// Percentage of memory_max_bytes that triggers SIGTERM.
    pub soft_limit_percent: u8,
    /// Polling interval.
    pub interval: Duration,
    /// Grace period between SIGTERM and SIGKILL.
    pub grace_period: Duration,
}

/// Handle to a running memory monitor.  Drop or call [`stop`] to cancel.
pub struct MemoryMonitorHandle {
    cancel_tx: watch::Sender<bool>,
    join: Option<tokio::task::JoinHandle<()>>,
}

impl MemoryMonitorHandle {
    /// Stop the monitor and wait for the background task to finish.
    pub async fn stop(mut self) {
        let _ = self.cancel_tx.send(true);
        if let Some(join) = self.join.take() {
            let _ = join.await;
        }
    }
}

impl Drop for MemoryMonitorHandle {
    fn drop(&mut self) {
        // Send the cancel signal so the monitor loop exits even if the caller
        // forgot to call stop() (e.g. early `?` return).  The send may fail
        // if stop() was already called — that is fine.
        let _ = self.cancel_tx.send(true);
    }
}

/// Start the background memory monitor.
///
/// Returns `None` if `memory_max_bytes` is 0 or `soft_limit_percent` is 0/100+
/// (i.e. the configuration is effectively "no monitoring").
pub fn start(mut config: MemoryMonitorConfig) -> Option<MemoryMonitorHandle> {
    if config.memory_max_bytes == 0
        || config.soft_limit_percent == 0
        || config.soft_limit_percent > 100
    {
        return None;
    }

    // Defense in depth: clamp zero interval to 1 second to prevent busy-polling.
    if config.interval.is_zero() {
        config.interval = Duration::from_secs(1);
    }

    let threshold_bytes = config.memory_max_bytes * u64::from(config.soft_limit_percent) / 100;

    info!(
        scope = %config.scope_name,
        threshold_mb = threshold_bytes / 1024 / 1024,
        limit_mb = config.memory_max_bytes / 1024 / 1024,
        percent = config.soft_limit_percent,
        interval_s = config.interval.as_secs(),
        "memory monitor started"
    );

    let (cancel_tx, cancel_rx) = watch::channel(false);
    let join = tokio::spawn(monitor_loop(config, threshold_bytes, cancel_rx));
    Some(MemoryMonitorHandle {
        cancel_tx,
        join: Some(join),
    })
}

async fn monitor_loop(
    config: MemoryMonitorConfig,
    threshold_bytes: u64,
    mut cancel_rx: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            _ = tokio::time::sleep(config.interval) => {}
            result = cancel_rx.changed() => {
                // Both Ok (value sent) and Err (channel closed, e.g. sender
                // dropped without explicit stop()) mean we should exit.
                if result.is_err() || *cancel_rx.borrow() {
                    debug!(scope = %config.scope_name, "memory monitor cancelled");
                    return;
                }
            }
        }

        let current = match query_memory_current(&config.scope_name).await {
            Some(bytes) => bytes,
            None => {
                // Scope might be gone — stop monitoring.
                debug!(
                    scope = %config.scope_name,
                    "MemoryCurrent query failed, scope likely gone"
                );
                return;
            }
        };

        if current < threshold_bytes {
            continue;
        }

        // Soft limit exceeded — send SIGTERM then wait grace period.
        let current_mb = current / 1024 / 1024;
        let threshold_mb = threshold_bytes / 1024 / 1024;
        warn!(
            scope = %config.scope_name,
            current_mb,
            threshold_mb,
            "memory soft limit exceeded, sending SIGTERM to process group"
        );

        send_signal(config.pgid, libc::SIGTERM);

        // Wait grace period, then check again.
        tokio::select! {
            _ = tokio::time::sleep(config.grace_period) => {}
            result = cancel_rx.changed() => {
                if result.is_err() || *cancel_rx.borrow() {
                    return;
                }
            }
        }

        // Re-check: if still over threshold, escalate to SIGKILL.
        if let Some(still_current) = query_memory_current(&config.scope_name).await
            && still_current >= threshold_bytes
        {
            warn!(
                scope = %config.scope_name,
                current_mb = still_current / 1024 / 1024,
                "still over soft limit after grace period, sending SIGKILL"
            );
            send_signal(config.pgid, libc::SIGKILL);
        }
        return;
    }
}

/// Query `MemoryCurrent` (in bytes) for the given systemd scope.
async fn query_memory_current(scope_name: &str) -> Option<u64> {
    let output = tokio::process::Command::new("systemctl")
        .args([
            "--user",
            "show",
            scope_name,
            "--property=MemoryCurrent",
            "--value",
        ])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let value = String::from_utf8_lossy(&output.stdout);
    let trimmed = value.trim();
    if trimmed == "infinity" || trimmed.is_empty() {
        return None;
    }
    trimmed.parse::<u64>().ok()
}

/// Send a signal to a process group.
fn send_signal(pgid: i32, signal: i32) {
    // SAFETY: libc::kill with negative pid sends to the process group.
    // pgid is validated to be positive (representing a valid process group);
    // we negate it for the kill call.
    let result = unsafe { libc::kill(-pgid.abs(), signal) };
    if result != 0 {
        let errno = std::io::Error::last_os_error();
        debug!(pgid, signal, %errno, "kill() returned error (process may already be gone)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_start_returns_none_for_zero_max() {
        let config = MemoryMonitorConfig {
            scope_name: "test.scope".to_string(),
            pgid: 1234,
            memory_max_bytes: 0,
            soft_limit_percent: 80,
            interval: Duration::from_secs(5),
            grace_period: Duration::from_secs(5),
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
        };
        assert!(start(config).is_none());
    }
}
