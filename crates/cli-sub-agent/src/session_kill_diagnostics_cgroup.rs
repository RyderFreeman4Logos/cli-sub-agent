use std::path::PathBuf;

use super::CgroupMemoryEvents;

pub(super) fn read_session_cgroup_memory_events(
    tool_name: &str,
    session_id: &str,
) -> Option<CgroupMemoryEvents> {
    let scope = csa_resource::cgroup::scope_unit_name(tool_name, session_id);
    let mut first_readable = None;
    for path in cgroup_memory_event_candidates(&scope) {
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        let events = parse_memory_events(&content);
        if events.has_oom_event() {
            return Some(events);
        }
        first_readable.get_or_insert(events);
    }
    first_readable
}

fn parse_memory_events(content: &str) -> CgroupMemoryEvents {
    let mut events = CgroupMemoryEvents {
        oom: 0,
        oom_kill: 0,
    };
    for line in content.lines() {
        let mut fields = line.split_whitespace();
        let key = fields.next();
        let value = fields.next().and_then(|raw| raw.parse::<u64>().ok());
        match (key, value) {
            (Some("oom"), Some(value)) => events.oom = value,
            (Some("oom_kill"), Some(value)) => events.oom_kill = value,
            _ => {}
        }
    }
    events
}

fn cgroup_memory_event_candidates(scope: &str) -> Vec<PathBuf> {
    let control_group = query_systemd_control_group(scope);
    cgroup_memory_event_candidates_from_parts(
        scope,
        effective_uid_from_proc_status(),
        control_group.as_deref(),
    )
}

fn cgroup_memory_event_candidates_from_parts(
    scope: &str,
    uid: Option<u32>,
    control_group: Option<&str>,
) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = control_group.and_then(systemd_control_group_memory_events_path) {
        push_unique_path(&mut candidates, path);
    }
    if let Some(uid) = uid {
        push_unique_path(
            &mut candidates,
            PathBuf::from(format!(
                "/sys/fs/cgroup/user.slice/user-{uid}.slice/user@{uid}.service/app.slice/{scope}/memory.events"
            )),
        );
        push_unique_path(
            &mut candidates,
            PathBuf::from(format!(
                "/sys/fs/cgroup/user.slice/user-{uid}.slice/user@{uid}.service/{scope}/memory.events"
            )),
        );
    }
    push_unique_path(
        &mut candidates,
        PathBuf::from(format!("/sys/fs/cgroup/system.slice/{scope}/memory.events")),
    );
    candidates
}

fn query_systemd_control_group(scope: &str) -> Option<String> {
    let output = std::process::Command::new("systemctl")
        .args([
            "--user",
            "show",
            scope,
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
    let stdout = String::from_utf8_lossy(&output.stdout);
    systemd_control_group_value(&stdout).map(ToOwned::to_owned)
}

fn systemd_control_group_memory_events_path(raw: &str) -> Option<PathBuf> {
    let control_group = systemd_control_group_value(raw)?;
    let relative = control_group.strip_prefix('/').unwrap_or(control_group);
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
    path.push("memory.events");
    Some(path)
}

fn systemd_control_group_value(raw: &str) -> Option<&str> {
    raw.lines().find_map(|line| {
        let trimmed = line.trim();
        let value = trimmed
            .strip_prefix("ControlGroup=")
            .unwrap_or(trimmed)
            .trim();
        if value.is_empty() || value == "/" {
            None
        } else {
            Some(value)
        }
    })
}

fn push_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.contains(&path) {
        paths.push(path);
    }
}

fn effective_uid_from_proc_status() -> Option<u32> {
    let content = std::fs::read_to_string("/proc/self/status").ok()?;
    content.lines().find_map(|line| {
        let rest = line.strip_prefix("Uid:")?;
        rest.split_whitespace()
            .next()
            .and_then(|value| value.parse().ok())
    })
}

#[cfg(test)]
#[path = "session_kill_diagnostics_cgroup_tests.rs"]
mod tests;
