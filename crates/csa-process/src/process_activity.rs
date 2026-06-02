use std::collections::{HashMap, HashSet};

/// Runtime state of a process tree sampled for quiet-session liveness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessTreeStatus {
    /// No live process remains in the monitored process tree.
    Dead,
    /// At least one process is alive, but CPU counters did not advance since the previous sample.
    AliveIdle,
    /// At least one process is alive and cumulative CPU counters advanced since the previous sample.
    AliveWithCpuProgress,
}

/// Tracks CPU progress for a spawned tool process and its descendants.
///
/// The first observation establishes the baseline. A later observation reports
/// [`ProcessTreeStatus::AliveWithCpuProgress`] only when cumulative user+system
/// CPU ticks for the process tree increased.
#[derive(Debug, Clone, Copy)]
pub struct ProcessTreeActivity {
    root_pid: u32,
    last_cpu_ticks: Option<u64>,
}

impl ProcessTreeActivity {
    pub fn new(root_pid: u32) -> Self {
        Self {
            root_pid,
            last_cpu_ticks: None,
        }
    }

    pub fn observe(&mut self) -> ProcessTreeStatus {
        let Some(cpu_ticks) = process_tree_cpu_ticks(self.root_pid) else {
            self.last_cpu_ticks = None;
            return ProcessTreeStatus::Dead;
        };

        let status = match self.last_cpu_ticks {
            Some(previous) if cpu_ticks > previous => ProcessTreeStatus::AliveWithCpuProgress,
            _ => ProcessTreeStatus::AliveIdle,
        };
        self.last_cpu_ticks = Some(cpu_ticks);
        status
    }
}

/// Return cumulative user+system CPU ticks for the live process tree rooted at `root_pid`.
pub fn process_tree_cpu_ticks(root_pid: u32) -> Option<u64> {
    platform_process_tree_cpu_ticks(root_pid)
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy)]
struct ProcStat {
    pid: u32,
    ppid: u32,
    pgrp: i32,
    state: char,
    cpu_ticks: u64,
}

#[cfg(target_os = "linux")]
fn platform_process_tree_cpu_ticks(root_pid: u32) -> Option<u64> {
    let stats = read_all_proc_stats();
    let root = stats.iter().find(|stat| stat.pid == root_pid).copied();
    let root_pgrp = root.and_then(|stat| (stat.pgrp == root_pid as i32).then_some(stat.pgrp));
    let parents: HashMap<u32, u32> = stats.iter().map(|stat| (stat.pid, stat.ppid)).collect();

    let mut saw_live_process = false;
    let total = stats
        .iter()
        .filter(|stat| process_belongs_to_tree(**stat, root_pid, root_pgrp, &parents))
        .filter(|stat| !matches!(stat.state, 'Z' | 'X'))
        .inspect(|_| saw_live_process = true)
        .map(|stat| stat.cpu_ticks)
        .sum::<u64>();

    saw_live_process.then_some(total)
}

#[cfg(target_os = "linux")]
fn process_belongs_to_tree(
    stat: ProcStat,
    root_pid: u32,
    root_pgrp: Option<i32>,
    parents: &HashMap<u32, u32>,
) -> bool {
    stat.pid == root_pid
        || root_pgrp.is_some_and(|pgrp| stat.pgrp == pgrp)
        || is_descendant_of(stat.pid, root_pid, parents)
}

#[cfg(target_os = "linux")]
fn is_descendant_of(pid: u32, root_pid: u32, parents: &HashMap<u32, u32>) -> bool {
    let mut current = pid;
    let mut seen = HashSet::new();
    while let Some(parent) = parents.get(&current).copied() {
        if parent == root_pid {
            return true;
        }
        if parent == 0 || !seen.insert(current) {
            return false;
        }
        current = parent;
    }
    false
}

#[cfg(target_os = "linux")]
fn read_all_proc_stats() -> Vec<ProcStat> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };

    entries
        .flatten()
        .filter_map(|entry| {
            let pid = entry.file_name().to_str()?.parse::<u32>().ok()?;
            read_proc_stat(pid)
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn read_proc_stat(pid: u32) -> Option<ProcStat> {
    let content = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let close_paren = content.rfind(')')?;
    let after_comm = &content[close_paren + 1..];
    let mut parts = after_comm.split_whitespace();
    let state = parts.next()?.chars().next()?;
    let ppid = parts.next()?.parse::<u32>().ok()?;
    let pgrp = parts.next()?.parse::<i32>().ok()?;
    for _ in 0..8 {
        parts.next()?;
    }
    let utime = parts.next()?.parse::<u64>().ok()?;
    let stime = parts.next()?.parse::<u64>().ok()?;

    Some(ProcStat {
        pid,
        ppid,
        pgrp,
        state,
        cpu_ticks: utime.saturating_add(stime),
    })
}

#[cfg(not(target_os = "linux"))]
fn platform_process_tree_cpu_ticks(root_pid: u32) -> Option<u64> {
    process_is_alive(root_pid).then_some(0)
}

#[cfg(not(target_os = "linux"))]
fn process_is_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // SAFETY: `kill(pid, 0)` probes existence/permission without sending a signal.
        let ret = unsafe { libc::kill(pid as libc::pid_t, 0) };
        if ret == 0 {
            return true;
        }
        let errno = std::io::Error::last_os_error().raw_os_error();
        errno == Some(libc::EPERM)
    }

    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

#[cfg(test)]
#[path = "process_activity_tests.rs"]
mod tests;
