//! Cgroup v2 working-set sampling for the soft memory monitor.

use std::path::PathBuf;

/// Return the cgroup usage that should count toward the soft limit.
///
/// `memory.current` includes page cache charged to the scope. The cgroup v2
/// `inactive_file` counter is reclaimable file cache, so exclude it when the
/// kernel provides a coherent value. If the stat cannot be read or is malformed,
/// retain the full charge so an unavailable diagnostic source never weakens the
/// memory limit.
pub(crate) fn soft_limit_usage_bytes(memory_current_bytes: u64, memory_stat: Option<&str>) -> u64 {
    let Some(inactive_file_bytes) = memory_stat
        .and_then(parse_inactive_file_bytes)
        .filter(|inactive_file_bytes| *inactive_file_bytes <= memory_current_bytes)
    else {
        return memory_current_bytes;
    };

    memory_current_bytes - inactive_file_bytes
}

fn parse_inactive_file_bytes(memory_stat: &str) -> Option<u64> {
    memory_stat.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        match (fields.next(), fields.next(), fields.next()) {
            (Some("inactive_file"), Some(value), None) => value.parse().ok(),
            _ => None,
        }
    })
}

pub(crate) async fn query_soft_limit_usage_bytes(
    scope_name: &str,
    memory_current_bytes: u64,
) -> u64 {
    let memory_stat = query_cgroup_memory_stat(scope_name).await;
    soft_limit_usage_bytes(memory_current_bytes, memory_stat.as_deref())
}

async fn query_cgroup_memory_stat(scope_name: &str) -> Option<String> {
    let control_group = query_systemd_control_group(scope_name).await?;
    let path = cgroup_memory_stat_path(&control_group)?;
    tokio::fs::read_to_string(path).await.ok()
}

async fn query_systemd_control_group(scope_name: &str) -> Option<String> {
    let output = tokio::process::Command::new("systemctl")
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
        .await
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let control_group = String::from_utf8(output.stdout).ok()?;
    cgroup_memory_stat_path(&control_group)?;
    Some(control_group)
}

fn cgroup_memory_stat_path(control_group: &str) -> Option<PathBuf> {
    let relative = control_group.trim().trim_start_matches('/');
    if relative.is_empty() {
        return None;
    }

    let mut path = PathBuf::from("/sys/fs/cgroup");
    for segment in relative.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return None;
        }
        path.push(segment);
    }
    Some(path.join("memory.stat"))
}

#[cfg(test)]
#[path = "memory_monitor_cgroup_tests.rs"]
mod tests;
