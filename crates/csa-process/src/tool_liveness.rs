use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const LIVENESS_RECENT_WINDOW_SECS: u64 = 30;
const OUTPUT_LOG_FILE: &str = "output.log";
const ACP_EVENTS_LOG_FILE: &str = "output/acp-events.jsonl";
const STDERR_LOG_FILE: &str = "stderr.log";
const SNAPSHOT_FILE: &str = ".liveness.snapshot";

pub const DEFAULT_LIVENESS_DEAD_SECS: u64 = 600;

#[derive(Debug, Default, Clone, Copy)]
struct LivenessSnapshot {
    output_log_size: Option<u64>,
    acp_events_size: Option<u64>,
    stderr_log_size: Option<u64>,
    run_log_size: Option<u64>,
}

/// Filesystem-only liveness probe for a running tool session.
///
/// Signal priority:
/// 1) live PID from lock files
/// 2) output growth (`output.log` / ACP events)
/// 3) recent writes under session directory
/// 4) stderr/log growth (`stderr.log` or latest run log)
pub struct ToolLiveness;

impl ToolLiveness {
    pub fn is_alive(session_dir: &Path) -> bool {
        let now = SystemTime::now();
        let mut snapshot = load_snapshot(session_dir);

        let process_alive = has_live_pid_signal(session_dir);
        let output_growth = has_output_growth_signal(session_dir, now, &mut snapshot);
        let session_write = has_recent_session_write_signal(session_dir, now);
        let stderr_activity = has_stderr_activity_signal(session_dir, now, &mut snapshot);

        save_snapshot(session_dir, &snapshot);

        process_alive || output_growth || session_write || stderr_activity
    }
}

fn has_live_pid_signal(session_dir: &Path) -> bool {
    let locks_dir = session_dir.join("locks");
    let entries = match fs::read_dir(&locks_dir) {
        Ok(entries) => entries,
        Err(_) => return false,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "lock")
            && fs::read_to_string(&path)
                .ok()
                .and_then(|content| extract_pid(&content))
                .is_some_and(is_process_alive)
        {
            return true;
        }
    }
    false
}

fn has_output_growth_signal(
    session_dir: &Path,
    now: SystemTime,
    snapshot: &mut LivenessSnapshot,
) -> bool {
    let output_path = session_dir.join(OUTPUT_LOG_FILE);
    let (output_growth, output_size) = detect_growth(&output_path, snapshot.output_log_size);
    snapshot.output_log_size = output_size;

    let acp_path = session_dir.join(ACP_EVENTS_LOG_FILE);
    let (acp_growth, acp_size) = detect_growth(&acp_path, snapshot.acp_events_size);
    snapshot.acp_events_size = acp_size;

    output_growth
        || acp_growth
        || file_modified_recently(&output_path, now)
        || file_modified_recently(&acp_path, now)
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
            if path.file_name().is_some_and(|name| name == SNAPSHOT_FILE) {
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

fn has_stderr_activity_signal(
    session_dir: &Path,
    now: SystemTime,
    snapshot: &mut LivenessSnapshot,
) -> bool {
    let stderr_path = session_dir.join(STDERR_LOG_FILE);
    let (stderr_growth, stderr_size) = detect_growth(&stderr_path, snapshot.stderr_log_size);
    snapshot.stderr_log_size = stderr_size;

    let latest_run_log = newest_log_file(session_dir);
    let (run_growth, run_size) = latest_run_log
        .as_ref()
        .map(|path| detect_growth(path, snapshot.run_log_size))
        .unwrap_or((false, None));
    snapshot.run_log_size = run_size;

    stderr_growth
        || latest_run_log
            .as_ref()
            .is_some_and(|path| file_modified_recently(path, now))
        || run_growth
        || file_modified_recently(&stderr_path, now)
}

fn newest_log_file(session_dir: &Path) -> Option<PathBuf> {
    let logs_dir = session_dir.join("logs");
    let mut newest: Option<(SystemTime, PathBuf)> = None;
    for entry in fs::read_dir(logs_dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "log") {
            continue;
        }
        let modified = entry.metadata().ok()?.modified().ok()?;
        match &newest {
            Some((current, _)) if &modified <= current => {}
            _ => newest = Some((modified, path)),
        }
    }
    newest.map(|(_, path)| path)
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
            "output_log_size" => snapshot.output_log_size = parsed,
            "acp_events_size" => snapshot.acp_events_size = parsed,
            "stderr_log_size" => snapshot.stderr_log_size = parsed,
            "run_log_size" => snapshot.run_log_size = parsed,
            _ => {}
        }
    }
    snapshot
}

fn save_snapshot(session_dir: &Path, snapshot: &LivenessSnapshot) {
    let mut lines = Vec::with_capacity(4);
    if let Some(value) = snapshot.output_log_size {
        lines.push(format!("output_log_size={value}"));
    }
    if let Some(value) = snapshot.acp_events_size {
        lines.push(format!("acp_events_size={value}"));
    }
    if let Some(value) = snapshot.stderr_log_size {
        lines.push(format!("stderr_log_size={value}"));
    }
    if let Some(value) = snapshot.run_log_size {
        lines.push(format!("run_log_size={value}"));
    }
    if lines.is_empty() {
        return;
    }
    let _ = fs::write(snapshot_path(session_dir), lines.join("\n"));
}

fn extract_pid(lock_content: &str) -> Option<u32> {
    let pid_key_pos = lock_content.find("\"pid\"")?;
    let tail = &lock_content[pid_key_pos..];
    let colon_pos = tail.find(':')?;
    let number = tail[colon_pos + 1..]
        .chars()
        .skip_while(|ch| ch.is_ascii_whitespace())
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>();
    number.parse::<u32>().ok()
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
