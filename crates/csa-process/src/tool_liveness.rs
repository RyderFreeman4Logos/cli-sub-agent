use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::process_tree_cpu_ticks;

#[path = "tool_liveness_fatal_error.rs"]
mod fatal_error;
#[cfg(test)]
use fatal_error::build_fatal_error_regex;
use fatal_error::provider_error_signal;
const LIVENESS_RECENT_WINDOW_SECS: u64 = 30;
const LOCK_FILE_STALE_SECS: u64 = 60;
const DAEMON_PID_FILE: &str = "daemon.pid";
const ACP_EVENTS_LOG_FILE: &str = "output/acp-events.jsonl";
const STDERR_LOG_FILE: &str = "stderr.log";
// Only referenced by tests now that the fatal-marker scan is scoped to the stderr
// transport stream (#1830); model/assistant `output.log` is no longer scanned.
#[cfg(test)]
const OUTPUT_LOG_FILE: &str = "output.log";
const SNAPSHOT_FILE: &str = ".liveness.snapshot";
const FATAL_ERROR_MARKERS_FILE: &str = ".fatal-error-markers";
pub const DEFAULT_LIVENESS_DEAD_SECS: u64 = 600;
#[derive(Debug, Clone, Copy)]
struct DaemonPidRecord {
    pid: u32,
    start_time_ticks: Option<u64>,
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy)]
struct ProcessMetadata {
    state: char,
    pgrp: i32,
    start_time_ticks: u64,
}

/// Fine-grained liveness signals used by idle-timeout watchdog logic.
///
/// `pid_alive`/`session_write` indicate coarse liveness, while
/// `output_growth`/`stderr_activity` indicate concrete progress.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LivenessSignals {
    pub pid_alive: bool,
    pub cpu_progress: bool,
    pub output_growth: bool,
    pub session_write: bool,
    pub stderr_activity: bool,
    pub provider_error: Option<ProviderErrorKind>,
    pub fatal_error: bool,
}

impl LivenessSignals {
    pub(crate) fn has_progress_signal(self) -> bool {
        // Treat only stream/log growth as concrete progress. Generic
        // "recent file write" is retained as a coarse liveness signal but is
        // too noisy for idle-timeout extension (lock files, snapshots, etc.).
        self.cpu_progress || self.output_growth || self.stderr_activity
    }

    pub(crate) fn has_any_signal(self) -> bool {
        self.pid_alive || self.session_write || self.has_progress_signal()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderErrorKind {
    Transient,
    Permanent,
}

#[derive(Debug, Default, Clone, Copy)]
struct LivenessSnapshot {
    spool_bytes_written: Option<u64>,
    observed_spool_bytes_written: Option<u64>,
    acp_events_size: Option<u64>,
    stderr_log_size: Option<u64>,
    process_cpu_ticks: Option<u64>,
}

/// Filesystem-only liveness probe for a running tool session.
///
/// Signal priority:
/// 1) live PID from lock files
/// 2) output growth (`output.log` / ACP events)
/// 3) recent writes under session directory
/// 4) stderr growth (`stderr.log`)
pub struct ToolLiveness;

impl ToolLiveness {
    pub(crate) fn probe(session_dir: &Path) -> LivenessSignals {
        let now = SystemTime::now();
        let mut snapshot = load_snapshot(session_dir);
        let daemon_pid_alive = Self::daemon_pid_is_alive(session_dir);

        let provider_error = provider_error_signal(session_dir);
        let signals = LivenessSignals {
            pid_alive: has_live_pid_signal(session_dir) || daemon_pid_alive,
            cpu_progress: has_process_cpu_progress_signal(session_dir, &mut snapshot),
            output_growth: has_output_growth_signal(session_dir, &mut snapshot),
            session_write: has_recent_session_write_signal(session_dir, now),
            stderr_activity: has_stderr_activity_signal(session_dir, &mut snapshot),
            provider_error,
            fatal_error: provider_error.is_some(),
        };

        save_snapshot(session_dir, &snapshot);
        signals
    }

    pub fn is_alive(session_dir: &Path) -> bool {
        Self::probe(session_dir).has_any_signal()
    }

    /// Return a live process PID that still matches this session's context.
    ///
    /// This intentionally ignores coarse filesystem activity so callers can
    /// distinguish "session files were touched recently" from "the daemon
    /// process that should produce result.toml is still alive".
    pub fn live_process_pid(session_dir: &Path) -> Option<u32> {
        find_session_pid(session_dir)
    }

    /// Whether a session still has a live process associated with it.
    pub fn has_live_process(session_dir: &Path) -> bool {
        Self::live_process_pid(session_dir).is_some()
    }

    /// Whether the recorded daemon PID still blocks session finalization.
    ///
    /// For modern `daemon.pid` records this verifies the same process instance
    /// via PID + start-time; for legacy single-field records it only falls back
    /// to signals we can still tie to this session (context-matched daemon PID
    /// or a zombie daemon leader whose process group still has live members).
    pub fn daemon_pid_is_alive(session_dir: &Path) -> bool {
        Self::daemon_pid_for_signal(session_dir).is_some()
    }

    /// Return the recorded daemon PID only when it still matches a session-
    /// relevant live process or process group.
    pub fn daemon_pid_for_signal(session_dir: &Path) -> Option<u32> {
        let record = read_daemon_pid_record(session_dir)?;

        #[cfg(unix)]
        {
            match (record.start_time_ticks, read_process_metadata(record.pid)) {
                (Some(expected), Some(metadata)) if metadata.start_time_ticks != expected => None,
                (Some(_), Some(ProcessMetadata { state: 'X', .. })) => None,
                (Some(_), Some(ProcessMetadata { state: 'Z', .. }))
                    if has_live_process_group_member(record.pid) =>
                {
                    Some(record.pid)
                }
                (Some(_), Some(ProcessMetadata { state: 'Z', .. })) => None,
                (Some(_), Some(_)) => Some(record.pid),
                (Some(_), None) if is_process_alive(record.pid) => Some(record.pid),
                (None, Some(ProcessMetadata { state: 'Z', .. }))
                    if has_live_process_group_member(record.pid) =>
                {
                    Some(record.pid)
                }
                (None, Some(ProcessMetadata { state: 'Z', .. })) => None,
                (None, _) if find_session_pid(session_dir) == Some(record.pid) => Some(record.pid),
                _ => None,
            }
        }

        #[cfg(not(unix))]
        {
            Some(record.pid)
        }
    }

    /// Zero-cost observation of whether the tool process is actively working.
    ///
    /// Reads `/proc/{pid}/stat` for the PID found in session lock files and
    /// checks the process state field:
    /// - **R** (running), **S** (sleeping), **D** (disk sleep) → working
    /// - **Z** (zombie), **T** (stopped), **X** (dead) → not working
    ///
    /// Falls back to `kill(pid, 0)` when `/proc` is unavailable.
    /// Does NOT consume tokens or context window.
    pub fn is_working(session_dir: &Path) -> bool {
        let Some(pid) = find_session_pid(session_dir) else {
            return false;
        };
        is_pid_working(pid)
    }
}

pub fn write_fatal_error_markers(session_dir: &Path, markers: &[String]) -> std::io::Result<()> {
    let mut file = File::create(session_dir.join(FATAL_ERROR_MARKERS_FILE))?;
    for marker in markers {
        writeln!(file, "{marker}")?;
    }
    Ok(())
}

pub(crate) fn record_spool_bytes_written(session_dir: &Path, bytes_written: u64) {
    let mut snapshot = load_snapshot(session_dir);
    snapshot.spool_bytes_written = Some(bytes_written);
    save_snapshot(session_dir, &snapshot);
}

/// Extract the first live PID from session lock files.
fn find_session_pid(session_dir: &Path) -> Option<u32> {
    if let Some(pid) = read_daemon_pid(session_dir)
        && pid_matches_session_context(pid, None, session_dir, None)
    {
        return Some(pid);
    }

    let locks_dir = session_dir.join("locks");
    let entries = fs::read_dir(&locks_dir).ok()?;

    for entry in entries.flatten() {
        let path = entry.path();
        if is_reconciler_artifact(&path) || path.extension().is_none_or(|ext| ext != "lock") {
            continue;
        }
        let Some(content) = fs::read_to_string(&path).ok() else {
            continue;
        };
        let Some(pid) = extract_pid(&content) else {
            continue;
        };
        let tool_name = path.file_stem().and_then(|stem| stem.to_str());
        let recent = lock_file_is_recent(&path, SystemTime::now());
        if pid_matches_session_context(pid, tool_name, session_dir, Some(recent)) {
            return Some(pid);
        }
    }
    None
}

fn is_reconciler_artifact(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|n| n.to_str()),
        Some(".reconcile.lock") | Some(".reconcile")
    )
}

/// Check if a process is actively working by reading `/proc/{pid}/stat`.
///
/// Process state field (3rd field in `/proc/{pid}/stat`):
/// - R = running on CPU
/// - S = sleeping (interruptible, e.g. waiting for I/O or network)
/// - D = disk sleep (uninterruptible, e.g. waiting for disk I/O)
/// - Z = zombie (terminated but not reaped)
/// - T = stopped (e.g. by SIGSTOP)
/// - X = dead
fn is_pid_working(pid: u32) -> bool {
    #[cfg(unix)]
    {
        let Some(ProcessMetadata { state, .. }) = read_process_metadata(pid) else {
            return is_process_alive(pid);
        };
        matches!(state, 'R' | 'S' | 'D')
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

#[cfg(unix)]
fn read_process_metadata(pid: u32) -> Option<ProcessMetadata> {
    let stat_path = format!("/proc/{pid}/stat");
    let content = fs::read_to_string(stat_path).ok()?;
    let close_paren = content.rfind(')')?;
    let after_comm = &content[close_paren + 1..];
    let mut parts = after_comm.split_whitespace();
    let state = parts.next()?.chars().next()?;
    let _ppid = parts.next()?;
    let pgrp = parts.next()?.parse::<i32>().ok()?;
    for _ in 0..16 {
        parts.next()?;
    }
    let start_time_ticks = parts.next()?.parse::<u64>().ok()?;
    Some(ProcessMetadata {
        state,
        pgrp,
        start_time_ticks,
    })
}

#[cfg(unix)]
fn has_live_process_group_member(leader_pid: u32) -> bool {
    let Ok(entries) = fs::read_dir("/proc") else {
        return false;
    };
    let target_pgrp = leader_pid as i32;

    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Ok(pid) = name.parse::<u32>() else {
            continue;
        };
        let Some(ProcessMetadata { state, pgrp, .. }) = read_process_metadata(pid) else {
            continue;
        };
        if pgrp == target_pgrp && !matches!(state, 'Z' | 'X') {
            return true;
        }
    }

    false
}

fn has_live_pid_signal(session_dir: &Path) -> bool {
    find_session_pid(session_dir).is_some()
}

fn lock_file_is_recent(lock_path: &Path, now: SystemTime) -> bool {
    let modified = match fs::metadata(lock_path).and_then(|meta| meta.modified()) {
        Ok(modified) => modified,
        Err(_) => return false,
    };
    let elapsed = now.duration_since(modified).unwrap_or(Duration::ZERO);
    elapsed <= Duration::from_secs(LOCK_FILE_STALE_SECS)
}

fn process_matches_session_context(pid: u32, tool_name: Option<&str>, session_dir: &Path) -> bool {
    let Some(cmdline) = read_process_command_line(pid) else {
        return false;
    };
    let session_id = session_dir.file_name().and_then(|name| name.to_str());
    let session_path = session_dir.to_string_lossy();

    tool_name.is_some_and(|tool| cmdline.contains(tool))
        || session_id.is_some_and(|id| cmdline.contains(id))
        || cmdline.contains(session_path.as_ref())
}

#[cfg(target_os = "linux")]
fn read_process_command_line(pid: u32) -> Option<String> {
    let cmdline_path = PathBuf::from(format!("/proc/{pid}/cmdline"));
    let raw_cmdline = fs::read(cmdline_path).ok()?;
    Some(String::from_utf8_lossy(&raw_cmdline).replace('\0', " "))
}

#[cfg(target_os = "macos")]
fn read_process_command_line(pid: u32) -> Option<String> {
    let output = std::process::Command::new("/bin/ps")
        .args(["-ww", "-o", "command=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let command = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if command.is_empty() {
        return None;
    }
    Some(command)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn read_process_command_line(_pid: u32) -> Option<String> {
    None
}

fn pid_matches_session_context(
    pid: u32,
    tool_name: Option<&str>,
    session_dir: &Path,
    recent_file: Option<bool>,
) -> bool {
    if !is_process_alive(pid) {
        return false;
    }

    process_matches_session_context(pid, tool_name, session_dir) || recent_file.unwrap_or(false)
}

fn read_daemon_pid(session_dir: &Path) -> Option<u32> {
    read_daemon_pid_record(session_dir).map(|record| record.pid)
}

fn read_daemon_pid_record(session_dir: &Path) -> Option<DaemonPidRecord> {
    let pid_path = session_dir.join(DAEMON_PID_FILE);
    if let Ok(content) = fs::read_to_string(&pid_path)
        && let Some(record) = parse_daemon_pid_record(&content)
    {
        return Some(record);
    }

    let pid = read_legacy_daemon_pid_from_stderr(session_dir)?;
    Some(DaemonPidRecord {
        pid,
        start_time_ticks: None,
    })
}

fn parse_daemon_pid_record(content: &str) -> Option<DaemonPidRecord> {
    let mut parts = content.split_whitespace();
    let pid = parts.next()?.parse::<u32>().ok()?;
    let start_time_ticks = parts.next().and_then(|value| value.parse::<u64>().ok());
    Some(DaemonPidRecord {
        pid,
        start_time_ticks,
    })
}

fn read_legacy_daemon_pid_from_stderr(session_dir: &Path) -> Option<u32> {
    let stderr_path = session_dir.join(STDERR_LOG_FILE);
    let content = fs::read_to_string(stderr_path).ok()?;
    let pid_start = content.find("pid=")?;
    let rest = &content[pid_start + 4..];
    let pid_str: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    pid_str.parse::<u32>().ok()
}

fn has_output_growth_signal(session_dir: &Path, snapshot: &mut LivenessSnapshot) -> bool {
    let output_growth = matches!(
        (
            snapshot.spool_bytes_written,
            snapshot.observed_spool_bytes_written
        ),
        (Some(current), Some(previous)) if current != previous
    );
    snapshot.observed_spool_bytes_written = snapshot.spool_bytes_written;

    let acp_path = session_dir.join(ACP_EVENTS_LOG_FILE);
    let (acp_growth, acp_size) = detect_growth(&acp_path, snapshot.acp_events_size);
    snapshot.acp_events_size = acp_size;

    output_growth || acp_growth
}

fn has_recent_session_write_signal(session_dir: &Path, now: SystemTime) -> bool {
    let mut stack = vec![session_dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if is_reconciler_artifact(&path)
                || path.file_name().is_some_and(|name| name == SNAPSHOT_FILE)
            {
                continue;
            }

            let file_type = match entry.file_type() {
                Ok(ft) => ft,
                Err(_) => continue,
            };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            if file_modified_recently(&path, now) {
                return true;
            }
        }
    }
    false
}

fn has_stderr_activity_signal(session_dir: &Path, snapshot: &mut LivenessSnapshot) -> bool {
    let stderr_path = session_dir.join(STDERR_LOG_FILE);
    let (stderr_growth, stderr_size) = detect_growth(&stderr_path, snapshot.stderr_log_size);
    snapshot.stderr_log_size = stderr_size;
    stderr_growth
}

fn has_process_cpu_progress_signal(session_dir: &Path, snapshot: &mut LivenessSnapshot) -> bool {
    let Some(pid) = find_session_pid(session_dir) else {
        snapshot.process_cpu_ticks = None;
        return false;
    };
    let Some(current_ticks) = process_tree_cpu_ticks(pid) else {
        snapshot.process_cpu_ticks = None;
        return false;
    };

    let progressed =
        matches!(snapshot.process_cpu_ticks, Some(previous) if current_ticks > previous);
    snapshot.process_cpu_ticks = Some(current_ticks);
    progressed
}

fn detect_growth(path: &Path, previous_size: Option<u64>) -> (bool, Option<u64>) {
    let current_size = fs::metadata(path).ok().map(|meta| meta.len());
    let growth = match (previous_size, current_size) {
        (Some(prev), Some(current)) => current != prev,
        _ => false,
    };
    (growth, current_size)
}

fn file_modified_recently(path: &Path, now: SystemTime) -> bool {
    let modified = match fs::metadata(path).and_then(|meta| meta.modified()) {
        Ok(modified) => modified,
        Err(_) => return false,
    };
    let elapsed = now.duration_since(modified).unwrap_or(Duration::ZERO);
    elapsed <= Duration::from_secs(LIVENESS_RECENT_WINDOW_SECS)
}

fn snapshot_path(session_dir: &Path) -> PathBuf {
    session_dir.join(SNAPSHOT_FILE)
}

fn load_snapshot(session_dir: &Path) -> LivenessSnapshot {
    let path = snapshot_path(session_dir);
    let Ok(content) = fs::read_to_string(path) else {
        return LivenessSnapshot::default();
    };
    let mut snapshot = LivenessSnapshot::default();
    for line in content.lines() {
        let mut parts = line.splitn(2, '=');
        let key = parts.next().unwrap_or_default().trim();
        let value = parts.next().unwrap_or_default().trim();
        let parsed = value.parse::<u64>().ok();
        match key {
            "spool_bytes_written" => snapshot.spool_bytes_written = parsed,
            "observed_spool_bytes_written" => snapshot.observed_spool_bytes_written = parsed,
            "acp_events_size" => snapshot.acp_events_size = parsed,
            "stderr_log_size" => snapshot.stderr_log_size = parsed,
            "process_cpu_ticks" => snapshot.process_cpu_ticks = parsed,
            _ => {}
        }
    }
    snapshot
}

fn save_snapshot(session_dir: &Path, snapshot: &LivenessSnapshot) {
    let mut lines = Vec::with_capacity(4);
    if let Some(value) = snapshot.spool_bytes_written {
        lines.push(format!("spool_bytes_written={value}"));
    }
    if let Some(value) = snapshot.observed_spool_bytes_written {
        lines.push(format!("observed_spool_bytes_written={value}"));
    }
    if let Some(value) = snapshot.acp_events_size {
        lines.push(format!("acp_events_size={value}"));
    }
    if let Some(value) = snapshot.stderr_log_size {
        lines.push(format!("stderr_log_size={value}"));
    }
    if let Some(value) = snapshot.process_cpu_ticks {
        lines.push(format!("process_cpu_ticks={value}"));
    }
    if lines.is_empty() {
        return;
    }
    let _ = fs::write(snapshot_path(session_dir), lines.join("\n"));
}

fn extract_pid(lock_content: &str) -> Option<u32> {
    #[derive(serde::Deserialize)]
    struct LockFileContent {
        pid: u32,
    }
    serde_json::from_str::<LockFileContent>(lock_content)
        .ok()
        .map(|data| data.pid)
}

fn is_process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // SAFETY: `kill(pid, 0)` performs existence/permission probe only.
        let ret = unsafe { libc::kill(pid as libc::pid_t, 0) };
        if ret == 0 {
            return true;
        }
        let errno = std::io::Error::last_os_error().raw_os_error();
        errno == Some(libc::EPERM)
    }

    #[cfg(not(unix))]
    {
        std::path::Path::new(&format!("/proc/{pid}/stat")).exists()
    }
}

#[cfg(test)]
#[path = "tool_liveness_tests.rs"]
mod tests;
