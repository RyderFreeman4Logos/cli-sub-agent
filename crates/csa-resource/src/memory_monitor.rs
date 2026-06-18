//! Background memory monitor for cgroup scopes.
//!
//! Polls `MemoryCurrent` at a configurable interval and sends SIGTERM to the
//! process group when usage exceeds `soft_limit_percent` of `MemoryMax`.
//! After a grace period, escalates to SIGKILL.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use tracing::{debug, info, warn};

pub const MEMORY_SOFT_LIMIT_KILL_HINT: &str = "memory_soft_limit";
pub const MEMORY_SOFT_LIMIT_KILL_FILE_NAME: &str = "memory-soft-limit-kill.toml";
const MEMORY_SOFT_LIMIT_DIAGNOSTIC_DIR: &str = "memory-soft-limit";

static SOFT_LIMIT_DIAGNOSTICS: LazyLock<Mutex<HashMap<PathBuf, RecordedSoftLimitDiagnostic>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Debug, Clone)]
struct RecordedSoftLimitDiagnostic {
    diagnostic: MemorySoftLimitKillDiagnostic,
    recorded_at: SystemTime,
}

/// Concrete evidence that CSA's memory monitor initiated a signal kill.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemorySoftLimitKillDiagnostic {
    pub kill_hint: String,
    pub signal: i32,
    pub current_mb: u64,
    pub threshold_mb: u64,
    pub memory_max_mb: u64,
    pub soft_limit_percent: u8,
    pub scope_name: String,
}

impl MemorySoftLimitKillDiagnostic {
    fn from_config(config: &MemoryMonitorConfig, current_bytes: u64, threshold_bytes: u64) -> Self {
        Self {
            kill_hint: MEMORY_SOFT_LIMIT_KILL_HINT.to_string(),
            signal: libc::SIGTERM,
            current_mb: bytes_to_mb(current_bytes),
            threshold_mb: bytes_to_mb(threshold_bytes),
            memory_max_mb: bytes_to_mb(config.memory_max_bytes),
            soft_limit_percent: config.soft_limit_percent,
            scope_name: config.scope_name.clone(),
        }
    }
}

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
    /// Optional stable registry key for supervisor-side kill diagnostics.
    ///
    /// The legacy field name is path-shaped because older implementations wrote
    /// an informational TOML artifact there. Current implementations only use it
    /// as an in-process CSA-owned registry key and never create, remove, or trust
    /// a file at this location.
    pub diagnostic_path: Option<PathBuf>,
}

/// Return the CSA supervisor-owned diagnostic registry key for a session directory.
///
/// The key intentionally maps outside the child-writable session/output tree,
/// but no TOML file is written or trusted at this location. The authoritative
/// evidence is the in-process record inserted by CSA's memory monitor, so a
/// tool-created file at this path (or in `output/`) cannot by itself prove
/// `memory_soft_limit`.
pub fn soft_limit_diagnostic_path_for_session_dir(session_dir: &Path) -> Option<PathBuf> {
    let session_id = session_dir.file_name()?.to_str()?.trim();
    if session_id.is_empty() || session_id.contains('/') || session_id.contains('\\') {
        return None;
    }

    Some(
        supervisor_diagnostic_root()
            .join(MEMORY_SOFT_LIMIT_DIAGNOSTIC_DIR)
            .join(session_dir_key(session_dir))
            .join(session_id)
            .join(MEMORY_SOFT_LIMIT_KILL_FILE_NAME),
    )
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
    // Clear CSA-owned evidence at run start before any early-returning monitor
    // setup validation. A later run with disabled monitoring or a zero memory
    // limit must not inherit a previous run's soft-limit kill evidence.
    clear_soft_limit_diagnostic_path(config.diagnostic_path.as_deref());

    let threshold_bytes = crate::memory_policy::soft_limit_threshold_bytes(
        config.memory_max_bytes,
        config.soft_limit_percent,
    )?;

    // Defense in depth: clamp zero interval to 1 second to prevent busy-polling.
    if config.interval.is_zero() {
        config.interval = Duration::from_secs(1);
    }

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

        // Soft limit exceeded — record concrete evidence before sending SIGTERM.
        let diagnostic =
            MemorySoftLimitKillDiagnostic::from_config(&config, current, threshold_bytes);
        let current_mb = diagnostic.current_mb;
        let threshold_mb = diagnostic.threshold_mb;
        warn!(
            scope = %config.scope_name,
            current_mb,
            threshold_mb,
            "memory soft limit exceeded, sending SIGTERM to process group"
        );
        record_soft_limit_diagnostic(config.diagnostic_path.as_deref(), &diagnostic);

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

/// Record CSA-owned in-process evidence that the memory monitor initiated a
/// soft-limit SIGTERM for `path`.
///
/// This is the authoritative boundary: readers only classify `memory_soft_limit`
/// when this in-memory evidence exists. Callers must not invoke this after
/// parsing files or other child-writable artifacts.
pub fn record_soft_limit_diagnostic_evidence(
    path: &Path,
    diagnostic: &MemorySoftLimitKillDiagnostic,
) {
    let Ok(mut diagnostics) = SOFT_LIMIT_DIAGNOSTICS.lock() else {
        return;
    };
    diagnostics.insert(
        path.to_path_buf(),
        RecordedSoftLimitDiagnostic {
            diagnostic: diagnostic.clone(),
            recorded_at: SystemTime::now(),
        },
    );
}

/// Read CSA-owned memory soft-limit evidence previously recorded for `path`.
pub fn read_soft_limit_diagnostic(path: &Path) -> Option<MemorySoftLimitKillDiagnostic> {
    read_soft_limit_diagnostic_recorded_at_or_after(path, None)
}

/// Read CSA-owned memory soft-limit evidence recorded no earlier than `not_before`.
pub fn read_soft_limit_diagnostic_recorded_at_or_after(
    path: &Path,
    not_before: Option<SystemTime>,
) -> Option<MemorySoftLimitKillDiagnostic> {
    let recorded = SOFT_LIMIT_DIAGNOSTICS
        .lock()
        .ok()
        .and_then(|diagnostics| diagnostics.get(path).cloned())?;
    if let Some(not_before) = not_before
        && recorded.recorded_at < not_before
    {
        return None;
    }
    validate_soft_limit_diagnostic(recorded.diagnostic)
}

fn validate_soft_limit_diagnostic(
    diagnostic: MemorySoftLimitKillDiagnostic,
) -> Option<MemorySoftLimitKillDiagnostic> {
    if diagnostic.kill_hint == MEMORY_SOFT_LIMIT_KILL_HINT && diagnostic.signal == libc::SIGTERM {
        Some(diagnostic)
    } else {
        None
    }
}

fn supervisor_diagnostic_root() -> PathBuf {
    if let Some(runtime_dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        let runtime_dir = PathBuf::from(runtime_dir);
        if runtime_dir.is_absolute() {
            return runtime_dir
                .join("cli-sub-agent")
                .join("supervisor-diagnostics");
        }
    }

    std::env::temp_dir().join(format!(
        "cli-sub-agent-supervisor-{}",
        effective_uid_label()
    ))
}

fn session_dir_key(session_dir: &Path) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    session_dir.as_os_str().hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

#[cfg(unix)]
fn effective_uid_label() -> String {
    // SAFETY: geteuid has no preconditions and does not dereference pointers.
    unsafe { libc::geteuid() }.to_string()
}

#[cfg(not(unix))]
fn effective_uid_label() -> String {
    "unknown".to_string()
}

/// Clear CSA-owned memory soft-limit evidence for a diagnostic registry key.
///
/// Call this at the start of every new run before monitor setup can be skipped;
/// otherwise a previous run's registry entry could be mistaken for the current
/// run when the new run exits from an unrelated external signal.
pub fn clear_soft_limit_diagnostic(path: &Path) {
    clear_soft_limit_diagnostic_path(Some(path));
}

fn clear_soft_limit_diagnostic_path(path: Option<&Path>) {
    let Some(path) = path else {
        return;
    };
    if let Ok(mut diagnostics) = SOFT_LIMIT_DIAGNOSTICS.lock() {
        diagnostics.remove(path);
    }
}

fn record_soft_limit_diagnostic(path: Option<&Path>, diagnostic: &MemorySoftLimitKillDiagnostic) {
    let Some(path) = path else {
        return;
    };
    record_soft_limit_diagnostic_evidence(path, diagnostic);
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

fn bytes_to_mb(bytes: u64) -> u64 {
    bytes / 1024 / 1024
}

#[cfg(test)]
mod tests {
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
}
