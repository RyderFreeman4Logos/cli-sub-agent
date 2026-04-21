use std::collections::{HashSet, VecDeque};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const BYTES_PER_MB: u64 = 1024 * 1024;

/// Cached Linux process-tree sampler for `csa session wait --memory-warn`.
///
/// The cgroup path and daemon PID are resolved once up front so per-tick
/// sampling only needs a direct file read or a bounded descendant walk.
pub struct SessionTreeMemorySampler {
    daemon_pid: u32,
    memory_current_path: Option<PathBuf>,
}

impl SessionTreeMemorySampler {
    pub fn new(project_root: &Path, session_id: &str) -> io::Result<Self> {
        let session_dir =
            crate::get_session_dir(project_root, session_id).map_err(io::Error::other)?;
        let daemon_pid = csa_process::ToolLiveness::daemon_pid_for_signal(&session_dir)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "session daemon PID unavailable")
            })?;

        Ok(Self {
            daemon_pid,
            memory_current_path: read_process_control_group(daemon_pid).map(|control_group| {
                Path::new("/sys/fs/cgroup")
                    .join(control_group.trim_start_matches('/'))
                    .join("memory.current")
            }),
        })
    }

    pub fn sample_rss_mb(&self) -> io::Result<u64> {
        if let Some(memory_current_path) = &self.memory_current_path
            && let Ok(bytes) = read_memory_current_bytes(memory_current_path)
        {
            return Ok(bytes_to_mb_ceil(bytes));
        }

        sample_process_tree_rss_mb(self.daemon_pid)
    }
}

/// Measure a session daemon's process-tree RSS in MB.
///
/// Linux-only for now. Sampling prefers cgroup `memory.current` when the
/// transient scope still exists, and falls back to summing `VmRSS` across the
/// daemon's live descendants.
pub fn session_tree_rss_mb(project_root: &Path, session_id: &str) -> io::Result<u64> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (project_root, session_id);
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "session wait memory warnings are Linux-only",
        ));
    }

    #[cfg(target_os = "linux")]
    {
        SessionTreeMemorySampler::new(project_root, session_id)?.sample_rss_mb()
    }
}

#[cfg(target_os = "linux")]
fn read_memory_current_bytes(memory_current_path: &Path) -> io::Result<u64> {
    let raw = fs::read_to_string(memory_current_path)?;
    raw.trim()
        .parse::<u64>()
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

#[cfg(target_os = "linux")]
fn sample_process_tree_rss_mb(root_pid: u32) -> io::Result<u64> {
    let mut total_kib = 0_u64;
    let mut matched_any = false;
    let mut pending = VecDeque::from([root_pid]);
    let mut visited = HashSet::new();

    while let Some(pid) = pending.pop_front() {
        if !visited.insert(pid) {
            continue;
        }

        if let Some(rss_kib) = read_vmrss_kib(pid) {
            total_kib = total_kib.saturating_add(rss_kib);
            matched_any = true;
        }

        pending.extend(read_child_pids(pid));
    }

    if !matched_any {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("no live processes found in process tree rooted at daemon PID {root_pid}"),
        ));
    }

    Ok(bytes_to_mb_ceil(total_kib.saturating_mul(1024)))
}

#[cfg(target_os = "linux")]
fn read_process_control_group(pid: u32) -> Option<String> {
    let raw = fs::read_to_string(format!("/proc/{pid}/cgroup")).ok()?;
    parse_process_control_group(&raw)
}

#[cfg(target_os = "linux")]
fn parse_process_control_group(raw: &str) -> Option<String> {
    raw.lines().find_map(|line| {
        let mut parts = line.splitn(3, ':');
        let _hierarchy = parts.next()?;
        let controllers = parts.next()?;
        let path = parts.next()?.trim();

        if path.is_empty() || path == "/" {
            return None;
        }

        if controllers.is_empty()
            || controllers
                .split(',')
                .any(|controller| controller == "memory")
        {
            Some(path.to_string())
        } else {
            None
        }
    })
}

#[cfg(target_os = "linux")]
fn read_child_pids(pid: u32) -> Vec<u32> {
    let children_path = format!("/proc/{pid}/task/{pid}/children");
    let Ok(raw) = fs::read_to_string(children_path) else {
        return Vec::new();
    };

    raw.split_whitespace()
        .filter_map(|value| value.parse::<u32>().ok())
        .collect()
}

#[cfg(target_os = "linux")]
fn read_vmrss_kib(pid: u32) -> Option<u64> {
    let status = fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    status.lines().find_map(|line| {
        let value = line.strip_prefix("VmRSS:")?.split_whitespace().next()?;
        value.parse::<u64>().ok()
    })
}

fn bytes_to_mb_ceil(bytes: u64) -> u64 {
    bytes.saturating_add(BYTES_PER_MB - 1) / BYTES_PER_MB
}

#[cfg(test)]
mod tests {
    use super::{bytes_to_mb_ceil, parse_process_control_group};

    #[test]
    fn bytes_to_mb_ceil_rounds_up_partial_megabytes() {
        assert_eq!(bytes_to_mb_ceil(0), 0);
        assert_eq!(bytes_to_mb_ceil(1), 1);
        assert_eq!(bytes_to_mb_ceil(1024 * 1024), 1);
        assert_eq!(bytes_to_mb_ceil(1024 * 1024 + 1), 2);
    }

    #[test]
    fn parse_process_control_group_prefers_unified_v2_entry() {
        let raw = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/csa.scope\n";
        assert_eq!(
            parse_process_control_group(raw).as_deref(),
            Some("/user.slice/user-1000.slice/user@1000.service/app.slice/csa.scope")
        );
    }

    #[test]
    fn parse_process_control_group_accepts_memory_controller_entry() {
        let raw = "7:memory:/user.slice/user-1000.slice/session-2.scope\n";
        assert_eq!(
            parse_process_control_group(raw).as_deref(),
            Some("/user.slice/user-1000.slice/session-2.scope")
        );
    }

    #[test]
    fn parse_process_control_group_ignores_root_path() {
        let raw = "0::/\n";
        assert_eq!(parse_process_control_group(raw), None);
    }
}
