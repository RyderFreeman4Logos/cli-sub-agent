//! Best-effort diagnostics for signal exits that often mean host OOM pressure.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use csa_session::{
    MetaSessionState, SessionResult, SignalResultMetadata, save_result,
    save_result_with_signal_metadata,
};

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
    Earlyoom,
    MemoryPressure,
    PossibleMemoryPressure,
    UnknownSignal,
}

impl KillHint {
    pub(crate) fn as_result_hint(self) -> &'static str {
        match self {
            Self::CsaTimeout => "csa_timeout",
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
}

impl KillDiagnostic {
    fn detail_parts(&self) -> Vec<String> {
        let mut details = Vec::new();
        if let Some(meminfo) = &self.observations.meminfo {
            details.push(format!(
                "MemAvailable: {} MB / MemTotal: {} MB",
                meminfo.available_mb(),
                meminfo.total_mb()
            ));
        } else {
            details.push("MemAvailable: unavailable".to_string());
        }
        if self.observations.earlyoom_running {
            details.push("earlyoom running".to_string());
        }
        if let Some(events) = self.observations.cgroup_memory_events
            && events.has_oom_event()
        {
            details.push(format!(
                "cgroup memory.events oom={} oom_kill={}",
                events.oom, events.oom_kill
            ));
        }
        details
    }

    pub(crate) fn stderr_line(&self) -> Option<String> {
        let hint = match self.hint {
            KillHint::Earlyoom => "earlyoom",
            KillHint::MemoryPressure => "memory pressure",
            KillHint::PossibleMemoryPressure => "possible memory pressure",
            KillHint::CsaTimeout | KillHint::UnknownSignal => return None,
        };

        Some(format!(
            "CSA diagnostic: signal kill hint: {hint} ({}). Re-dispatch when host memory frees.",
            self.detail_parts().join(", ")
        ))
    }

    pub(crate) fn ephemeral_line(&self) -> String {
        format!(
            "CSA diagnostic: ephemeral run ended by signal (kill_hint={}, {}). No persistent session metadata was created.",
            self.hint.as_result_hint(),
            self.detail_parts().join(", ")
        )
    }
}

pub(crate) fn diagnose_signal_kill(
    exit_code: i32,
    terminal_reason: Option<&str>,
    tool_name: &str,
    session_id: &str,
) -> Option<KillDiagnostic> {
    diagnose_signal_kill_with(exit_code, terminal_reason, || {
        collect_signal_observations(tool_name, session_id)
    })
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
    if !matches!(exit_code, 137 | 143) {
        return None;
    }
    if csa_timeout_reason(terminal_reason) {
        return Some(KillDiagnostic {
            hint: KillHint::CsaTimeout,
            observations: KillSignalObservations::default(),
        });
    }
    Some(classify_signal_kill(collect()))
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

pub(crate) fn save_result_with_signal_diagnostic(
    project_root: &Path,
    session: &MetaSessionState,
    tool_name: &str,
    result: &mut SessionResult,
    terminal_reason: Option<&str>,
    stderr_output: Option<&mut String>,
) -> Result<Option<String>> {
    let diagnostic = diagnose_signal_kill(
        result.exit_code,
        terminal_reason,
        tool_name,
        &session.meta_session_id,
    );
    let diagnostic_line = diagnostic.as_ref().and_then(KillDiagnostic::stderr_line);
    if let Some(line) = &diagnostic_line {
        append_stderr_line(stderr_output, line);
        result.summary = line.clone();
    }

    if let Some(diagnostic) = diagnostic {
        save_result_with_signal_metadata(
            project_root,
            &session.meta_session_id,
            result,
            SignalResultMetadata {
                kill_hint: diagnostic.hint.as_result_hint(),
                last_item: last_known_work_item(session, tool_name),
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
        if let Some(line) = diagnostic.stderr_line() {
            rendered_result.summary = line;
        }
        rendered_result.last_item = last_item
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
    }
    toml::to_string_pretty(&rendered_result).context("Failed to render session result")
}

pub(crate) fn signal_toml(
    result: &SessionResult,
    session: &MetaSessionState,
    session_id: &str,
    exit_code: i32,
) -> Result<String> {
    let diagnostic = diagnose_signal_kill(
        exit_code,
        session.termination_reason.as_deref(),
        &result.tool,
        session_id,
    );
    render_result_toml_with_signal_diagnostic(
        result,
        diagnostic.as_ref(),
        last_known_work_item(session, &result.tool),
    )
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
    matches!(reason, Some("idle_timeout" | "initial_response_timeout"))
}

fn classify_signal_kill(observations: KillSignalObservations) -> KillDiagnostic {
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
    } else {
        KillHint::UnknownSignal
    };

    KillDiagnostic { hint, observations }
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

#[cfg(target_os = "linux")]
fn read_session_cgroup_memory_events(
    tool_name: &str,
    session_id: &str,
) -> Option<CgroupMemoryEvents> {
    let scope = csa_resource::cgroup::scope_unit_name(tool_name, session_id);
    cgroup_memory_event_candidates(&scope)
        .into_iter()
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .map(|content| parse_memory_events(&content))
        .find(|events| events.has_oom_event())
}

#[cfg(target_os = "linux")]
fn cgroup_memory_event_candidates(scope: &str) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(uid) = effective_uid_from_proc_status() {
        candidates.push(PathBuf::from(format!(
            "/sys/fs/cgroup/user.slice/user-{uid}.slice/user@{uid}.service/app.slice/{scope}/memory.events"
        )));
        candidates.push(PathBuf::from(format!(
            "/sys/fs/cgroup/user.slice/user-{uid}.slice/user@{uid}.service/{scope}/memory.events"
        )));
    }
    candidates.push(PathBuf::from(format!(
        "/sys/fs/cgroup/system.slice/{scope}/memory.events"
    )));
    candidates
}

#[cfg(target_os = "linux")]
fn effective_uid_from_proc_status() -> Option<u32> {
    let content = std::fs::read_to_string("/proc/self/status").ok()?;
    content.lines().find_map(|line| {
        let rest = line.strip_prefix("Uid:")?;
        rest.split_whitespace()
            .next()
            .and_then(|value| value.parse().ok())
    })
}

fn kb_to_mb(kb: u64) -> u64 {
    kb / 1024
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn parses_meminfo_available_and_total() {
        let meminfo = parse_meminfo(
            "\
MemTotal:       16384000 kB
MemFree:         1000000 kB
MemAvailable:    900000 kB
",
        )
        .expect("meminfo should parse");

        assert_eq!(meminfo.total_kb, 16_384_000);
        assert_eq!(meminfo.available_kb, 900_000);
        assert!(meminfo.available_below_ten_percent());
        assert!(!meminfo.available_below_five_percent());
    }

    #[test]
    fn classifies_signal_exit_under_possible_memory_pressure() {
        let diagnostic = diagnose_signal_kill_with(143, None, || KillSignalObservations {
            meminfo: Some(MemInfo {
                total_kb: 10_000,
                available_kb: 999,
            }),
            earlyoom_running: false,
            cgroup_memory_events: None,
        })
        .expect("signal exit should produce diagnostic");

        assert_eq!(diagnostic.hint, KillHint::PossibleMemoryPressure);
        assert!(
            diagnostic
                .stderr_line()
                .expect("memory pressure should render")
                .contains("MemAvailable: 0 MB / MemTotal: 9 MB")
        );
    }

    #[test]
    fn classifies_signal_exit_under_strong_memory_pressure() {
        let diagnostic = diagnose_signal_kill_with(143, None, || KillSignalObservations {
            meminfo: Some(MemInfo {
                total_kb: 10_000,
                available_kb: 499,
            }),
            earlyoom_running: false,
            cgroup_memory_events: None,
        })
        .expect("signal exit should produce diagnostic");

        assert_eq!(diagnostic.hint, KillHint::MemoryPressure);
        assert!(
            diagnostic
                .stderr_line()
                .expect("memory pressure should render")
                .contains("memory pressure")
        );
    }

    #[test]
    fn classifies_unknown_when_earlyoom_runs_without_memory_pressure() {
        let diagnostic = diagnose_signal_kill_with(137, None, || KillSignalObservations {
            meminfo: Some(MemInfo {
                total_kb: 10_000,
                available_kb: 5_000,
            }),
            earlyoom_running: true,
            cgroup_memory_events: None,
        })
        .expect("signal exit should produce diagnostic");

        assert_eq!(diagnostic.hint, KillHint::UnknownSignal);
        assert!(diagnostic.stderr_line().is_none());
    }

    #[test]
    fn classifies_earlyoom_when_daemon_runs_with_strong_memory_pressure() {
        let diagnostic = diagnose_signal_kill_with(137, None, || KillSignalObservations {
            meminfo: Some(MemInfo {
                total_kb: 10_000,
                available_kb: 499,
            }),
            earlyoom_running: true,
            cgroup_memory_events: None,
        })
        .expect("signal exit should produce diagnostic");

        assert_eq!(diagnostic.hint, KillHint::Earlyoom);
        assert!(
            diagnostic
                .stderr_line()
                .expect("earlyoom should render")
                .contains("earlyoom running")
        );
    }

    #[test]
    fn classifies_earlyoom_when_daemon_runs_with_cgroup_oom_kill() {
        let diagnostic = diagnose_signal_kill_with(137, None, || KillSignalObservations {
            meminfo: Some(MemInfo {
                total_kb: 10_000,
                available_kb: 5_000,
            }),
            earlyoom_running: true,
            cgroup_memory_events: Some(CgroupMemoryEvents {
                oom: 0,
                oom_kill: 1,
            }),
        })
        .expect("signal exit should produce diagnostic");

        assert_eq!(diagnostic.hint, KillHint::Earlyoom);
    }

    #[test]
    fn does_not_classify_earlyoom_from_oom_without_oom_kill() {
        let diagnostic = diagnose_signal_kill_with(137, None, || KillSignalObservations {
            meminfo: Some(MemInfo {
                total_kb: 10_000,
                available_kb: 5_000,
            }),
            earlyoom_running: true,
            cgroup_memory_events: Some(CgroupMemoryEvents {
                oom: 1,
                oom_kill: 0,
            }),
        })
        .expect("signal exit should produce diagnostic");

        assert_eq!(diagnostic.hint, KillHint::UnknownSignal);
        assert!(diagnostic.stderr_line().is_none());
    }

    #[test]
    fn csa_timeout_terminal_reasons_take_precedence_over_earlyoom() {
        for terminal_reason in ["idle_timeout", "initial_response_timeout"] {
            let called = Cell::new(false);
            let diagnostic = diagnose_signal_kill_with(137, Some(terminal_reason), || {
                called.set(true);
                KillSignalObservations {
                    meminfo: Some(MemInfo {
                        total_kb: 10_000,
                        available_kb: 999,
                    }),
                    earlyoom_running: true,
                    cgroup_memory_events: Some(CgroupMemoryEvents {
                        oom: 1,
                        oom_kill: 1,
                    }),
                }
            })
            .expect("signal exit should produce diagnostic");

            assert_eq!(diagnostic.hint, KillHint::CsaTimeout);
            assert!(!called.get(), "{terminal_reason} should skip memory checks");
            assert!(diagnostic.stderr_line().is_none());
        }
    }

    #[test]
    fn classifies_unknown_signal_without_memory_evidence() {
        let diagnostic = diagnose_signal_kill_with(143, None, KillSignalObservations::default)
            .expect("signal exit should produce diagnostic");

        assert_eq!(diagnostic.hint, KillHint::UnknownSignal);
        assert!(diagnostic.stderr_line().is_none());
    }

    #[test]
    fn non_signal_exits_do_not_collect_observations() {
        for exit_code in [0, 1, 2] {
            let called = Cell::new(false);
            let diagnostic = diagnose_signal_kill_with(exit_code, None, || {
                called.set(true);
                KillSignalObservations::default()
            });
            assert!(diagnostic.is_none());
            assert!(!called.get(), "exit {exit_code} should not trigger checks");
        }
    }

    #[test]
    fn parses_cgroup_memory_events() {
        let events = parse_memory_events("low 2\nhigh 3\nmax 4\noom 1\noom_kill 1\n");

        assert_eq!(events.oom, 1);
        assert_eq!(events.oom_kill, 1);
        assert!(events.has_oom_event());
        assert!(events.has_oom_kill_event());
    }
}
