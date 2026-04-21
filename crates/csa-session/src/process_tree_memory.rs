use std::fs;
use std::io;
use std::path::Path;
use std::process::Command;

const BYTES_PER_MB: u64 = 1024 * 1024;

/// Measure a session daemon's process-tree RSS in MB.
///
/// Linux-only for now. Sampling prefers cgroup `memory.current` when the
/// transient `csa-<tool>-<session>.scope` still exists, and falls back to
/// summing `VmRSS` across the daemon process group.
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
        let session_dir =
            crate::get_session_dir(project_root, session_id).map_err(io::Error::other)?;

        if let Some(bytes) = read_scope_memory_current_bytes(project_root, session_id)? {
            return Ok(bytes_to_mb_ceil(bytes));
        }

        let daemon_pid = csa_process::ToolLiveness::daemon_pid_for_signal(&session_dir)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "session daemon PID unavailable")
            })?;
        let target_pgrp = read_process_group_id(daemon_pid).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("failed to read process group for daemon PID {daemon_pid}"),
            )
        })?;

        let mut total_kib = 0_u64;
        let mut matched_any = false;
        for entry in fs::read_dir("/proc")? {
            let entry = entry?;
            let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
                continue;
            };
            if read_process_group_id(pid) == Some(target_pgrp)
                && let Some(rss_kib) = read_vmrss_kib(pid)
            {
                total_kib = total_kib.saturating_add(rss_kib);
                matched_any = true;
            }
        }

        if !matched_any {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("no live processes found in process group {target_pgrp}"),
            ));
        }

        Ok(bytes_to_mb_ceil(total_kib.saturating_mul(1024)))
    }
}

#[cfg(target_os = "linux")]
fn read_scope_memory_current_bytes(
    project_root: &Path,
    session_id: &str,
) -> io::Result<Option<u64>> {
    let Some(metadata) =
        crate::load_metadata(project_root, session_id).map_err(io::Error::other)?
    else {
        return Ok(None);
    };

    let scope_name = csa_resource::scope_unit_name(&metadata.tool, session_id);
    let Some(control_group) = read_scope_control_group(&scope_name) else {
        return Ok(None);
    };

    let memory_current_path = Path::new("/sys/fs/cgroup")
        .join(control_group.trim_start_matches('/'))
        .join("memory.current");

    match fs::read_to_string(&memory_current_path) {
        Ok(raw) => raw
            .trim()
            .parse::<u64>()
            .map(Some)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

#[cfg(target_os = "linux")]
fn read_scope_control_group(scope_name: &str) -> Option<String> {
    let output = Command::new("systemctl")
        .args([
            "--user",
            "show",
            scope_name,
            "--property=ControlGroup",
            "--value",
        ])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let control_group = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if control_group.is_empty() || control_group == "/" {
        return None;
    }
    Some(control_group)
}

#[cfg(target_os = "linux")]
fn read_process_group_id(pid: u32) -> Option<i32> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let close_paren = stat.rfind(')')?;
    let after_comm = &stat[close_paren + 1..];
    let mut parts = after_comm.split_whitespace();
    parts.next()?; // state
    parts.next()?; // ppid
    parts.next()?.parse::<i32>().ok()
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
    use super::bytes_to_mb_ceil;

    #[test]
    fn bytes_to_mb_ceil_rounds_up_partial_megabytes() {
        assert_eq!(bytes_to_mb_ceil(0), 0);
        assert_eq!(bytes_to_mb_ceil(1), 1);
        assert_eq!(bytes_to_mb_ceil(1024 * 1024), 1);
        assert_eq!(bytes_to_mb_ceil(1024 * 1024 + 1), 2);
    }
}
