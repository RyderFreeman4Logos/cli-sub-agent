use std::path::Path;

use csa_resource::memory_monitor::MemorySoftLimitKillDiagnostic;

pub(super) fn read_memory_soft_limit_diagnostic(
    session_dir: &Path,
    artifact_not_before: Option<&chrono::DateTime<chrono::Utc>>,
) -> Option<MemorySoftLimitKillDiagnostic> {
    let path =
        csa_resource::memory_monitor::soft_limit_diagnostic_path_for_session_dir(session_dir)?;
    let not_before = match artifact_not_before {
        Some(not_before) => Some(utc_datetime_to_system_time(not_before)?),
        None => None,
    };
    csa_resource::memory_monitor::read_soft_limit_diagnostic_recorded_at_or_after(&path, not_before)
}

fn utc_datetime_to_system_time(
    value: &chrono::DateTime<chrono::Utc>,
) -> Option<std::time::SystemTime> {
    let seconds = value.timestamp();
    if seconds < 0 {
        return None;
    }
    std::time::UNIX_EPOCH.checked_add(std::time::Duration::new(
        seconds as u64,
        value.timestamp_subsec_nanos(),
    ))
}

#[cfg(test)]
mod tests {
    use super::super::{KillHint, KillSignalObservations, diagnose_signal_kill_with_events};
    use super::*;
    use std::path::{Path, PathBuf};

    fn memory_soft_limit_event() -> MemorySoftLimitKillDiagnostic {
        MemorySoftLimitKillDiagnostic {
            kill_hint: csa_resource::memory_monitor::MEMORY_SOFT_LIMIT_KILL_HINT.to_string(),
            signal: libc::SIGTERM,
            current_mb: 9216,
            threshold_mb: 8601,
            memory_max_mb: 12_288,
            soft_limit_percent: 70,
            scope_name: "csa-codex-01KTEST.scope".to_string(),
        }
    }

    fn test_session_dir(temp: &tempfile::TempDir, session_id: &str) -> PathBuf {
        let session_dir = temp.path().join("sessions").join(session_id);
        std::fs::create_dir_all(&session_dir).expect("create session dir");
        session_dir
    }

    fn register_memory_soft_limit_evidence(session_dir: &Path) {
        let registry_key =
            csa_resource::memory_monitor::soft_limit_diagnostic_path_for_session_dir(session_dir)
                .expect("CSA-owned memory diagnostic path");
        csa_resource::memory_monitor::record_soft_limit_diagnostic_evidence(
            &registry_key,
            &memory_soft_limit_event(),
        );
    }

    #[test]
    fn stale_registry_evidence_within_one_second_stays_unknown() {
        let temp = tempfile::tempdir().expect("tempdir");
        let session_dir = test_session_dir(&temp, "01KTESTMEMSTALE1S");
        register_memory_soft_limit_evidence(&session_dir);

        let later_run_start = chrono::Utc::now() + chrono::Duration::milliseconds(500);
        let memory_soft_limit =
            read_memory_soft_limit_diagnostic(&session_dir, Some(&later_run_start));
        assert!(
            memory_soft_limit.is_none(),
            "previous-run registry evidence must not be accepted inside a one-second grace window"
        );

        let diagnostic = diagnose_signal_kill_with_events(
            143,
            Some("sigterm"),
            memory_soft_limit,
            None,
            None,
            KillSignalObservations::default,
        )
        .expect("signal exit should produce diagnostic");

        assert_eq!(diagnostic.hint, KillHint::UnknownSignal);
        assert!(diagnostic.result_report().is_none());
    }

    #[test]
    fn current_run_registry_evidence_still_classifies() {
        let temp = tempfile::tempdir().expect("tempdir");
        let session_dir = test_session_dir(&temp, "01KTESTMEMCURRENT");
        let current_run_start = chrono::Utc::now();
        register_memory_soft_limit_evidence(&session_dir);

        let memory_soft_limit =
            read_memory_soft_limit_diagnostic(&session_dir, Some(&current_run_start));
        let diagnostic = diagnose_signal_kill_with_events(
            143,
            Some("signal"),
            memory_soft_limit,
            None,
            None,
            KillSignalObservations::default,
        )
        .expect("signal exit should produce diagnostic");

        assert_eq!(diagnostic.hint, KillHint::MemorySoftLimit);
        assert_eq!(
            diagnostic
                .result_report()
                .as_ref()
                .map(|report| report.source.as_str()),
            Some("memory_soft_limit")
        );
    }
}
