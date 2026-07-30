#[cfg(target_os = "linux")]
use std::collections::{HashSet, VecDeque};
#[cfg(target_os = "linux")]
use std::fs;
use std::io;
use std::path::Path;
#[cfg(target_os = "linux")]
use std::path::PathBuf;

#[cfg(any(target_os = "linux", test))]
const BYTES_PER_MB: u64 = 1024 * 1024;

/// Cached Linux process-tree sampler for `csa session wait --memory-warn`.
///
/// The cgroup path and daemon PID are resolved once up front so per-tick
/// sampling only needs a direct file read or a bounded descendant walk.
pub struct SessionTreeMemorySampler {
    #[cfg(target_os = "linux")]
    daemon_pid: u32,
    #[cfg(target_os = "linux")]
    memory_current_path: Option<PathBuf>,
}

impl SessionTreeMemorySampler {
    pub fn new(project_root: &Path, session_id: &str) -> io::Result<Self> {
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (project_root, session_id);
            return Err(unsupported_process_tree_memory());
        }

        #[cfg(target_os = "linux")]
        {
            let session_dir =
                crate::get_session_dir(project_root, session_id).map_err(io::Error::other)?;
            let daemon_pid = csa_process::ToolLiveness::daemon_pid_for_signal(&session_dir)
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::NotFound, "session daemon PID unavailable")
                })?;

            let expected_daemon_scope = csa_resource::cgroup::scope_unit_name("daemon", session_id);
            Ok(Self {
                daemon_pid,
                memory_current_path: read_process_control_group(daemon_pid).and_then(
                    |control_group| {
                        csa_scope_memory_current_path(&control_group, &expected_daemon_scope)
                    },
                ),
            })
        }
    }

    pub fn sample_rss_mb(&self) -> io::Result<u64> {
        #[cfg(not(target_os = "linux"))]
        {
            return Err(unsupported_process_tree_memory());
        }

        #[cfg(target_os = "linux")]
        {
            if let Some(memory_current_path) = &self.memory_current_path
                && let Ok(bytes) = read_memory_current_bytes(memory_current_path)
            {
                return Ok(bytes_to_mb_ceil(bytes));
            }

            sample_process_tree_rss_mb(self.daemon_pid)
        }
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
        return Err(unsupported_process_tree_memory());
    }

    #[cfg(target_os = "linux")]
    {
        SessionTreeMemorySampler::new(project_root, session_id)?.sample_rss_mb()
    }
}

#[cfg(not(target_os = "linux"))]
fn unsupported_process_tree_memory() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "session process-tree memory sampling is only available on Linux",
    )
}

#[cfg(target_os = "linux")]
fn read_memory_current_bytes(memory_current_path: &Path) -> io::Result<u64> {
    let raw = fs::read_to_string(memory_current_path)?;
    raw.trim()
        .parse::<u64>()
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

#[cfg(target_os = "linux")]
fn csa_scope_memory_current_path(
    control_group: &str,
    expected_daemon_scope: &str,
) -> Option<PathBuf> {
    // A parent/login scope includes unrelated processes and would be counted once per
    // active session by admission. A direct detached daemon can also inherit another
    // session's CSA scope, so only this daemon's exact transient scope can safely
    // replace process-tree sampling.
    let relative = control_group.trim().trim_start_matches('/');
    let scope_name = relative.rsplit('/').next()?;
    if scope_name != expected_daemon_scope {
        return None;
    }

    Some(
        Path::new("/sys/fs/cgroup")
            .join(relative)
            .join("memory.current"),
    )
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

#[cfg(any(target_os = "linux", test))]
fn bytes_to_mb_ceil(bytes: u64) -> u64 {
    bytes.saturating_add(BYTES_PER_MB - 1) / BYTES_PER_MB
}

#[cfg(all(test, not(target_os = "linux")))]
mod non_linux_tests {
    use super::*;

    #[test]
    fn sampler_reports_memory_sampling_unavailable() {
        let err = match SessionTreeMemorySampler::new(Path::new("."), "session") {
            Ok(_) => panic!("non-Linux sampler should be unavailable"),
            Err(err) => err,
        };

        assert_eq!(err.kind(), io::ErrorKind::Unsupported);
    }

    #[test]
    fn one_shot_sampler_reports_memory_sampling_unavailable() {
        let err = session_tree_rss_mb(Path::new("."), "session")
            .expect_err("non-Linux one-shot sampler should be unavailable");

        assert_eq!(err.kind(), io::ErrorKind::Unsupported);
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::bytes_to_mb_ceil;
    #[cfg(target_os = "linux")]
    use super::{csa_scope_memory_current_path, parse_process_control_group};

    #[test]
    fn bytes_to_mb_ceil_rounds_up_partial_megabytes() {
        assert_eq!(bytes_to_mb_ceil(0), 0);
        assert_eq!(bytes_to_mb_ceil(1), 1);
        assert_eq!(bytes_to_mb_ceil(1024 * 1024), 1);
        assert_eq!(bytes_to_mb_ceil(1024 * 1024 + 1), 2);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn parse_process_control_group_prefers_unified_v2_entry() {
        let raw = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/csa.scope\n";
        assert_eq!(
            parse_process_control_group(raw).as_deref(),
            Some("/user.slice/user-1000.slice/user@1000.service/app.slice/csa.scope")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn parse_process_control_group_accepts_memory_controller_entry() {
        let raw = "7:memory:/user.slice/user-1000.slice/session-2.scope\n";
        assert_eq!(
            parse_process_control_group(raw).as_deref(),
            Some("/user.slice/user-1000.slice/session-2.scope")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn parse_process_control_group_ignores_root_path() {
        let raw = "0::/\n";
        assert_eq!(parse_process_control_group(raw), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn shared_login_scope_is_not_used_as_a_session_memory_sample() {
        assert_eq!(
            csa_scope_memory_current_path(
                "/user.slice/user-1001.slice/session-5.scope",
                "csa-daemon-01JTEST.scope",
            ),
            None,
            "a shared login scope would charge every active session with the same host memory"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn csa_scope_is_used_as_a_session_memory_sample() {
        assert_eq!(
            csa_scope_memory_current_path(
                "/user.slice/user-1001.slice/user@1001.service/app.slice/csa-daemon-01JTEST.scope",
                "csa-daemon-01JTEST.scope",
            )
            .as_deref(),
            Some(
                Path::new("/sys/fs/cgroup")
                    .join("user.slice/user-1001.slice/user@1001.service/app.slice/csa-daemon-01JTEST.scope/memory.current")
                    .as_path()
            )
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn inherited_parent_csa_scope_is_not_used_for_a_different_daemon_session() {
        assert_eq!(
            csa_scope_memory_current_path(
                "/user.slice/user-1001.slice/user@1001.service/app.slice/csa-codex-01JPARENT.scope",
                "csa-daemon-01JCHILD.scope",
            ),
            None,
            "a direct detached daemon can inherit its caller's CSA scope"
        );
    }
}
