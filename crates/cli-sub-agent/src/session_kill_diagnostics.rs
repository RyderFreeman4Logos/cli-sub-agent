//! Best-effort diagnostics for signal exits that often mean host OOM pressure
//! or a bounded in-turn child command timing out.

use std::path::Path;

use anyhow::{Context, Result};
use csa_resource::memory_monitor::{MEMORY_SOFT_LIMIT_KILL_HINT, MemorySoftLimitKillDiagnostic};
use csa_session::{
    KillDiagnosticReport, MetaSessionState, SessionResult, SignalResultMetadata, save_result,
    save_result_with_signal_metadata,
};

#[path = "session_kill_diagnostics_memory.rs"]
mod memory;

#[cfg(target_os = "linux")]
#[path = "session_kill_diagnostics_cgroup.rs"]
mod cgroup;

mod child_timeout;
#[cfg(target_os = "linux")]
use cgroup::read_session_cgroup_memory_events;
use child_timeout::{
    ChildTimeoutKind, ChildTimeoutProvenance, detect_child_timeout_provenance, redact_command_text,
    truncate_one_line,
};
use memory::read_memory_soft_limit_diagnostic;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MemInfo {
    pub(crate) total_kb: u64,
    pub(crate) available_kb: u64,
}

impl MemInfo {
    fn available_below_five_percent(&self) -> bool {
        self.available_kb.saturating_mul(20) < self.total_kb
    }

    fn available_below_ten_percent(&self) -> bool {
        self.available_kb.saturating_mul(10) < self.total_kb
    }

    fn total_mb(&self) -> u64 {
        kb_to_mb(self.total_kb)
    }

    fn available_mb(&self) -> u64 {
        kb_to_mb(self.available_kb)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CgroupMemoryEvents {
    pub(crate) oom: u64,
    pub(crate) oom_kill: u64,
}

impl CgroupMemoryEvents {
    fn has_oom_event(self) -> bool {
        self.oom > 0 || self.oom_kill > 0
    }

    fn has_oom_kill_event(self) -> bool {
        self.oom_kill > 0
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct KillSignalObservations {
    pub(crate) meminfo: Option<MemInfo>,
    pub(crate) earlyoom_running: bool,
    pub(crate) cgroup_memory_events: Option<CgroupMemoryEvents>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KillHint {
    CsaTimeout,
    ChildTimeout,
    HookCommitTimeout,
    MemorySoftLimit,
    Earlyoom,
    MemoryPressure,
    PossibleMemoryPressure,
    UnknownSignal,
}

impl KillHint {
    pub(crate) fn as_result_hint(self) -> &'static str {
        match self {
            Self::CsaTimeout => "csa_timeout",
            Self::ChildTimeout => "child_timeout",
            Self::HookCommitTimeout => "hook_commit_timeout",
            Self::MemorySoftLimit => MEMORY_SOFT_LIMIT_KILL_HINT,
            Self::Earlyoom => "earlyoom",
            Self::MemoryPressure => "memory_pressure",
            Self::PossibleMemoryPressure => "possible_memory_pressure",
            Self::UnknownSignal => "unknown_signal",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KillDiagnostic {
    pub(crate) hint: KillHint,
    pub(crate) observations: KillSignalObservations,
    pub(crate) terminal_reason: Option<String>,
    pub(crate) child_timeout: Option<ChildTimeoutProvenance>,
    pub(crate) memory_soft_limit: Option<MemorySoftLimitKillDiagnostic>,
}

impl KillDiagnostic {
    fn detail_parts(&self) -> Vec<String> {
        let mut details = Vec::new();
        details.push(match self.terminal_reason.as_deref() {
            Some(reason) if !reason.trim().is_empty() => {
                format!("termination_reason={}", reason.trim())
            }
            _ => "termination_reason: missing".to_string(),
        });
        if self.hint == KillHint::CsaTimeout {
            details.push("CSA supervisor timeout metadata matched signal exit".to_string());
            return details;
        }
        if let Some(event) = &self.memory_soft_limit {
            details.push("CSA memory monitor soft-limit event matched signal exit".to_string());
            details.push(format!("current_mb={}", event.current_mb));
            details.push(format!("threshold_mb={}", event.threshold_mb));
            details.push(format!("memory_max_mb={}", event.memory_max_mb));
            details.push(format!("soft_limit_percent={}", event.soft_limit_percent));
            details.push(format!("scope_name={}", event.scope_name));
            return details;
        }
        if let Some(child) = &self.child_timeout {
            details.push("transcript matched bounded child command".to_string());
            if let Some(seconds) = child.timeout_seconds {
                details.push(format!("child_timeout_seconds={seconds}"));
            }
            if let Some(status) = child
                .command_status
                .as_deref()
                .map(str::trim)
                .filter(|status| !status.is_empty())
            {
                details.push(format!("child_command_status={status}"));
            }
            if child.transcript_exit_143 {
                details.push("transcript reported child exit 143".to_string());
            }
            details.push(format!(
                "child_command={}",
                redacted_command_one_line(&child.command, 180)
            ));
            return details;
        }
        if let Some(meminfo) = &self.observations.meminfo {
            details.push(format!(
                "MemAvailable: {} MB / MemTotal: {} MB",
                meminfo.available_mb(),
                meminfo.total_mb()
            ));
        } else {
            details.push("MemAvailable: unavailable".to_string());
        }
        details.push(if self.observations.earlyoom_running {
            "earlyoom running".to_string()
        } else {
            "earlyoom not running".to_string()
        });
        match self.observations.cgroup_memory_events {
            Some(events) => {
                details.push(format!(
                    "cgroup memory.events oom={} oom_kill={}",
                    events.oom, events.oom_kill
                ));
            }
            None => {
                details.push(
                    "cgroup memory.events: unavailable at expected session scope".to_string(),
                );
            }
        }
        details
    }

    pub(crate) fn stderr_line(&self) -> Option<String> {
        let hint = match self.hint {
            KillHint::Earlyoom => "earlyoom",
            KillHint::MemoryPressure => "memory pressure",
            KillHint::PossibleMemoryPressure => "possible memory pressure",
            KillHint::CsaTimeout => "csa_timeout",
            KillHint::ChildTimeout => "child_timeout",
            KillHint::HookCommitTimeout => "hook_commit_timeout",
            KillHint::MemorySoftLimit => "memory soft limit",
            KillHint::UnknownSignal => "unknown_signal",
        };

        let details = self.detail_parts().join(", ");
        let suffix = match self.hint {
            KillHint::Earlyoom | KillHint::MemoryPressure | KillHint::PossibleMemoryPressure => {
                "Re-dispatch when host memory frees."
            }
            KillHint::CsaTimeout => "The recorded timeout is the concrete kill reason.",
            KillHint::MemorySoftLimit => {
                "CSA's memory monitor sent SIGTERM at the configured soft limit; increase resources.memory_max_mb or tools.<tool>.memory_max_mb, raise resources.soft_limit_percent only if safe, or reduce compile/test parallelism."
            }
            KillHint::ChildTimeout => {
                "The transcript shows the last tool command was wrapped in a bounded timeout; inspect that child timeout before treating this as an external session kill."
            }
            KillHint::HookCommitTimeout => {
                "The transcript shows a bounded hook-enabled git commit was active; inspect hook duration/toolchain setup or increase the child command timeout before redispatching."
            }
            KillHint::UnknownSignal => {
                "No timeout or cgroup OOM evidence was found, and memory checks did not identify a concrete kill source; reason remains unknown."
            }
        };
        Some(format!(
            "CSA diagnostic: signal kill hint: {hint} ({details}). {suffix}"
        ))
    }

    pub(crate) fn last_item(&self) -> Option<String> {
        self.child_timeout
            .as_ref()
            .map(|child| redacted_command_one_line(&child.command, 300))
            .filter(|command| !command.is_empty())
    }

    pub(crate) fn ephemeral_line(&self) -> String {
        format!(
            "CSA diagnostic: ephemeral run ended by signal (kill_hint={}, {}). No persistent session metadata was created.",
            self.hint.as_result_hint(),
            self.detail_parts().join(", ")
        )
    }

    pub(crate) fn result_report(&self) -> Option<KillDiagnosticReport> {
        self.memory_soft_limit
            .as_ref()
            .map(|event| KillDiagnosticReport {
                source: event.kill_hint.clone(),
                signal: Some(event.signal),
                current_mb: Some(event.current_mb),
                threshold_mb: Some(event.threshold_mb),
                memory_max_mb: Some(event.memory_max_mb),
                soft_limit_percent: Some(event.soft_limit_percent),
                scope_name: Some(event.scope_name.clone()),
            })
    }
}

#[cfg(test)]
pub(crate) fn diagnose_signal_kill(
    exit_code: i32,
    terminal_reason: Option<&str>,
    tool_name: &str,
    session_id: &str,
    session_dir: Option<&Path>,
) -> Option<KillDiagnostic> {
    diagnose_signal_kill_with_artifact_window(
        exit_code,
        terminal_reason,
        tool_name,
        session_id,
        session_dir,
        None,
    )
}

fn diagnose_signal_kill_with_artifact_window(
    exit_code: i32,
    terminal_reason: Option<&str>,
    tool_name: &str,
    session_id: &str,
    session_dir: Option<&Path>,
    artifact_not_before: Option<&chrono::DateTime<chrono::Utc>>,
) -> Option<KillDiagnostic> {
    let child_timeout = session_dir.and_then(|dir| detect_child_timeout_provenance(dir, exit_code));
    let memory_soft_limit =
        session_dir.and_then(|dir| read_memory_soft_limit_diagnostic(dir, artifact_not_before));
    diagnose_signal_kill_with_events(
        exit_code,
        terminal_reason,
        memory_soft_limit,
        child_timeout,
        || collect_signal_observations(tool_name, session_id),
    )
}

pub(crate) fn diagnose_ephemeral_signal_kill(
    exit_code: i32,
    terminal_reason: Option<&str>,
) -> Option<KillDiagnostic> {
    diagnose_signal_kill_with(
        exit_code,
        terminal_reason,
        collect_ephemeral_signal_observations,
    )
}

pub(crate) fn diagnose_signal_kill_with(
    exit_code: i32,
    terminal_reason: Option<&str>,
    collect: impl FnOnce() -> KillSignalObservations,
) -> Option<KillDiagnostic> {
    diagnose_signal_kill_with_child_timeout(exit_code, terminal_reason, None, collect)
}

fn diagnose_signal_kill_with_child_timeout(
    exit_code: i32,
    terminal_reason: Option<&str>,
    child_timeout: Option<ChildTimeoutProvenance>,
    collect: impl FnOnce() -> KillSignalObservations,
) -> Option<KillDiagnostic> {
    diagnose_signal_kill_with_events(exit_code, terminal_reason, None, child_timeout, collect)
}

fn diagnose_signal_kill_with_events(
    exit_code: i32,
    terminal_reason: Option<&str>,
    memory_soft_limit: Option<MemorySoftLimitKillDiagnostic>,
    child_timeout: Option<ChildTimeoutProvenance>,
    collect: impl FnOnce() -> KillSignalObservations,
) -> Option<KillDiagnostic> {
    if !matches!(exit_code, 137 | 143) {
        return None;
    }
    if csa_timeout_reason(terminal_reason) {
        return Some(KillDiagnostic {
            hint: KillHint::CsaTimeout,
            observations: KillSignalObservations::default(),
            terminal_reason: normalize_terminal_reason(terminal_reason),
            child_timeout: None,
            memory_soft_limit: None,
        });
    }
    if let Some(memory_soft_limit) = memory_soft_limit {
        return Some(KillDiagnostic {
            hint: KillHint::MemorySoftLimit,
            observations: KillSignalObservations::default(),
            terminal_reason: normalize_terminal_reason(terminal_reason),
            child_timeout: None,
            memory_soft_limit: Some(memory_soft_limit),
        });
    }
    Some(classify_signal_kill(
        exit_code,
        collect(),
        normalize_terminal_reason(terminal_reason),
        child_timeout,
    ))
}

pub(crate) fn last_known_work_item<'a>(
    session: &'a csa_session::MetaSessionState,
    tool_name: &str,
) -> Option<&'a str> {
    session
        .tools
        .get(tool_name)
        .map(|tool_state| tool_state.last_action_summary.trim())
        .filter(|summary| !summary.is_empty())
}

fn redacted_command_one_line(command: &str, max_chars: usize) -> String {
    truncate_one_line(&redact_command_text(command), max_chars)
}

pub(crate) fn save_result_with_signal_diagnostic(
    project_root: &Path,
    session: &MetaSessionState,
    tool_name: &str,
    result: &mut SessionResult,
    terminal_reason: Option<&str>,
    stderr_output: Option<&mut String>,
) -> Result<Option<String>> {
    let session_dir = csa_session::get_session_dir(project_root, &session.meta_session_id).ok();
    let diagnostic = diagnose_signal_kill_with_artifact_window(
        result.exit_code,
        terminal_reason,
        tool_name,
        &session.meta_session_id,
        session_dir.as_deref(),
        Some(&result.started_at),
    );
    let diagnostic_line = diagnostic.as_ref().and_then(KillDiagnostic::stderr_line);
    if let Some(line) = &diagnostic_line {
        append_stderr_line(stderr_output, line);
        result.summary = line.clone();
    }

    if let Some(diagnostic) = diagnostic {
        let diagnostic_last_item = diagnostic.last_item();
        let last_item = diagnostic_last_item
            .as_deref()
            .or_else(|| last_known_work_item(session, tool_name))
            .map(redact_command_text);
        result.kill_diagnostics = diagnostic.result_report();
        save_result_with_signal_metadata(
            project_root,
            &session.meta_session_id,
            result,
            SignalResultMetadata {
                kill_hint: diagnostic.hint.as_result_hint(),
                last_item: last_item.as_deref(),
            },
        )?;
    } else {
        save_result(project_root, &session.meta_session_id, result)?;
    }

    Ok(diagnostic_line)
}

pub(crate) fn render_result_toml_with_signal_diagnostic(
    result: &SessionResult,
    diagnostic: Option<&KillDiagnostic>,
    last_item: Option<&str>,
) -> Result<String> {
    let mut rendered_result = result.clone();
    if let Some(diagnostic) = diagnostic {
        rendered_result.kill_hint = Some(diagnostic.hint.as_result_hint().to_string());
        rendered_result.kill_diagnostics = diagnostic.result_report();
        if let Some(line) = diagnostic.stderr_line() {
            rendered_result.summary = line;
        }
        rendered_result.last_item = last_item
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(redact_command_text);
    }
    toml::to_string_pretty(&rendered_result).context("Failed to render session result")
}

pub(crate) fn signal_toml(
    result: &SessionResult,
    session: &MetaSessionState,
    session_id: &str,
    session_dir: &Path,
    exit_code: i32,
) -> Result<String> {
    let diagnostic = diagnose_signal_kill_with_artifact_window(
        exit_code,
        session.termination_reason.as_deref(),
        &result.tool,
        session_id,
        Some(session_dir),
        Some(&result.started_at),
    );
    let diagnostic_last_item = diagnostic.as_ref().and_then(KillDiagnostic::last_item);
    let last_item = diagnostic_last_item
        .as_deref()
        .or_else(|| last_known_work_item(session, &result.tool));
    render_result_toml_with_signal_diagnostic(result, diagnostic.as_ref(), last_item)
}

fn append_stderr_line(stderr_output: Option<&mut String>, line: &str) {
    let Some(stderr_output) = stderr_output else {
        return;
    };
    if !stderr_output.is_empty() && !stderr_output.ends_with('\n') {
        stderr_output.push('\n');
    }
    stderr_output.push_str(line);
    stderr_output.push('\n');
}

fn csa_timeout_reason(reason: Option<&str>) -> bool {
    matches!(
        reason.map(str::trim),
        Some("timeout" | "idle_timeout" | "initial_response_timeout")
    )
}

fn normalize_terminal_reason(reason: Option<&str>) -> Option<String> {
    reason
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn classify_signal_kill(
    exit_code: i32,
    observations: KillSignalObservations,
    terminal_reason: Option<String>,
    child_timeout: Option<ChildTimeoutProvenance>,
) -> KillDiagnostic {
    let very_low_memory = observations
        .meminfo
        .as_ref()
        .is_some_and(MemInfo::available_below_five_percent);
    let possible_low_memory = observations
        .meminfo
        .as_ref()
        .is_some_and(MemInfo::available_below_ten_percent);
    let cgroup_oom_kill = observations
        .cgroup_memory_events
        .is_some_and(CgroupMemoryEvents::has_oom_kill_event);
    let strong_memory_pressure = very_low_memory || cgroup_oom_kill;
    let hint = if observations.earlyoom_running && strong_memory_pressure {
        KillHint::Earlyoom
    } else if strong_memory_pressure {
        KillHint::MemoryPressure
    } else if possible_low_memory {
        KillHint::PossibleMemoryPressure
    } else if exit_code == 143
        && let Some(child_timeout) = child_timeout
    {
        return KillDiagnostic {
            hint: child_timeout_hint(child_timeout.kind),
            observations,
            terminal_reason,
            child_timeout: Some(child_timeout),
            memory_soft_limit: None,
        };
    } else {
        KillHint::UnknownSignal
    };

    KillDiagnostic {
        hint,
        observations,
        terminal_reason,
        child_timeout: None,
        memory_soft_limit: None,
    }
}

fn child_timeout_hint(kind: ChildTimeoutKind) -> KillHint {
    match kind {
        ChildTimeoutKind::BoundedCommand => KillHint::ChildTimeout,
        ChildTimeoutKind::HookEnabledGitCommit => KillHint::HookCommitTimeout,
    }
}

#[cfg(target_os = "linux")]
fn collect_signal_observations(tool_name: &str, session_id: &str) -> KillSignalObservations {
    KillSignalObservations {
        meminfo: read_meminfo_from_path(Path::new("/proc/meminfo")),
        earlyoom_running: earlyoom_running(),
        cgroup_memory_events: read_session_cgroup_memory_events(tool_name, session_id),
    }
}

#[cfg(not(target_os = "linux"))]
fn collect_signal_observations(_tool_name: &str, _session_id: &str) -> KillSignalObservations {
    KillSignalObservations::default()
}

#[cfg(target_os = "linux")]
fn collect_ephemeral_signal_observations() -> KillSignalObservations {
    KillSignalObservations {
        meminfo: read_meminfo_from_path(Path::new("/proc/meminfo")),
        earlyoom_running: earlyoom_running(),
        cgroup_memory_events: None,
    }
}

#[cfg(not(target_os = "linux"))]
fn collect_ephemeral_signal_observations() -> KillSignalObservations {
    KillSignalObservations::default()
}

pub(crate) fn parse_meminfo(content: &str) -> Option<MemInfo> {
    let mut total_kb = None;
    let mut available_kb = None;

    for line in content.lines() {
        let mut fields = line.split_whitespace();
        match fields.next() {
            Some("MemTotal:") => total_kb = fields.next().and_then(|value| value.parse().ok()),
            Some("MemAvailable:") => {
                available_kb = fields.next().and_then(|value| value.parse().ok());
            }
            _ => {}
        }
    }

    Some(MemInfo {
        total_kb: total_kb?,
        available_kb: available_kb?,
    })
}

#[cfg(target_os = "linux")]
fn read_meminfo_from_path(path: &Path) -> Option<MemInfo> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|content| parse_meminfo(&content))
}

#[cfg(target_os = "linux")]
fn earlyoom_running() -> bool {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return false;
    };
    entries.filter_map(Result::ok).any(|entry| {
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            return false;
        };
        if !name.as_bytes().iter().all(u8::is_ascii_digit) {
            return false;
        }
        std::fs::read_to_string(entry.path().join("comm"))
            .is_ok_and(|comm| comm.trim() == "earlyoom")
    })
}

fn kb_to_mb(kb: u64) -> u64 {
    kb / 1024
}

#[cfg(test)]
#[path = "session_kill_diagnostics_tests.rs"]
mod tests;
