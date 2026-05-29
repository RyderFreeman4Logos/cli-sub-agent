use std::path::Path;
use std::time::{Duration, Instant};

use crate::ToolLiveness;

const LIVENESS_POLL_INTERVAL: Duration = Duration::from_secs(10);
const FATAL_ERROR_PROGRESS_GRACE: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IdleTerminationReason {
    Idle,
    FatalError,
}

pub(crate) fn idle_timeout_note(
    received_first_output: bool,
    initial_response_timeout: Option<Duration>,
    reason: IdleTerminationReason,
    effective_idle: Duration,
    liveness_dead_timeout: Duration,
) -> (&'static str, String) {
    if matches!(reason, IdleTerminationReason::FatalError) {
        let progress_kind = if !received_first_output && initial_response_timeout.is_some() {
            "stdout progress during initial response"
        } else {
            "output progress"
        };
        return (
            "fatal backend error",
            format!(
                "fatal backend error: matched configured 4xx/5xx/provider marker and observed no {progress_kind} for 30s; process killed"
            ),
        );
    }
    if !received_first_output && initial_response_timeout.is_some() {
        return (
            "initial_response_timeout",
            format!(
                "initial_response_timeout: no stdout output for {}s; process killed immediately (no liveness polling)",
                effective_idle.as_secs(),
            ),
        );
    }
    (
        "idle timeout",
        format!(
            "idle timeout: no stdout/stderr output for {}s; liveness false for {}s; process killed",
            effective_idle.as_secs(),
            liveness_dead_timeout.as_secs(),
        ),
    )
}

/// Check the startup watchdog before the first stdout byte is observed.
///
/// Stderr-only output must not satisfy the initial response watchdog, but fatal
/// backend errors commonly arrive on stderr.  Detect those after the same short
/// fatal-error grace used by the normal idle watchdog instead of waiting for the
/// full configured initial-response timeout.
pub(crate) fn should_terminate_for_initial_response(
    last_stdout_activity: Instant,
    initial_response_timeout: Duration,
    session_dir: Option<&Path>,
    next_liveness_poll_at: &mut Option<Instant>,
) -> Option<IdleTerminationReason> {
    let stdout_idle_for = last_stdout_activity.elapsed();

    if stdout_idle_for < FATAL_ERROR_PROGRESS_GRACE {
        *next_liveness_poll_at = None;
        return (stdout_idle_for >= initial_response_timeout)
            .then_some(IdleTerminationReason::Idle);
    }

    if let Some(dir) = session_dir {
        let now = Instant::now();
        let should_poll = next_liveness_poll_at
            .as_ref()
            .is_none_or(|next_poll| now >= *next_poll);
        if should_poll {
            *next_liveness_poll_at = Some(now + LIVENESS_POLL_INTERVAL);
            if ToolLiveness::probe(dir).fatal_error {
                return Some(IdleTerminationReason::FatalError);
            }
        }
    }

    (stdout_idle_for >= initial_response_timeout).then_some(IdleTerminationReason::Idle)
}

/// Check whether an idle tool process should be terminated.
///
/// When the tool has been silent (no stdout/stderr) for `idle_timeout`, this
/// function queries [`ToolLiveness::probe`] before killing. Only **progress
/// signals** (output/log growth) reset the idle timer.
/// Pure "process exists" signals (live PID only) no longer
/// grant unlimited extensions; in that case, termination happens once
/// `liveness_dead_timeout` elapses.
pub(crate) fn should_terminate_for_idle(
    last_activity: &mut Instant,
    idle_timeout: Duration,
    liveness_dead_timeout: Duration,
    session_dir: Option<&Path>,
    liveness_dead_since: &mut Option<Instant>,
    next_liveness_poll_at: &mut Option<Instant>,
) -> Option<IdleTerminationReason> {
    let idle_for = last_activity.elapsed();
    if idle_for < idle_timeout && idle_for < FATAL_ERROR_PROGRESS_GRACE {
        *liveness_dead_since = None;
        *next_liveness_poll_at = None;
        return None;
    }

    // Legacy execute_in path has no spool/session directory context.
    // Preserve original behavior: idle-timeout means immediate termination.
    let Some(dir) = session_dir else {
        return (idle_for >= idle_timeout).then_some(IdleTerminationReason::Idle);
    };

    let now = Instant::now();
    let should_poll = next_liveness_poll_at
        .as_ref()
        .is_none_or(|next_poll| now >= *next_poll);
    if !should_poll {
        return None;
    }

    let signals = ToolLiveness::probe(dir);
    if signals.has_progress_signal() {
        // Real progress observed: reset idle/death timers and give a fresh window.
        *last_activity = now;
        *liveness_dead_since = None;
        *next_liveness_poll_at = Some(now + LIVENESS_POLL_INTERVAL);
        return None;
    }

    if signals.fatal_error && idle_for >= FATAL_ERROR_PROGRESS_GRACE {
        return Some(IdleTerminationReason::FatalError);
    }

    if idle_for < idle_timeout {
        *liveness_dead_since = None;
        *next_liveness_poll_at = Some(now + LIVENESS_POLL_INTERVAL);
        return None;
    }

    let dead_since = liveness_dead_since.get_or_insert(now);
    if dead_since.elapsed() >= liveness_dead_timeout {
        return Some(IdleTerminationReason::Idle);
    }

    let liveness_dead_deadline = *dead_since + liveness_dead_timeout;
    *next_liveness_poll_at = Some((now + LIVENESS_POLL_INTERVAL).min(liveness_dead_deadline));
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_session_dir_kills_immediately_after_idle_timeout() {
        let mut dead_since = None;
        let mut next_poll = None;
        let mut last_activity = Instant::now() - Duration::from_secs(2);
        let should_terminate = should_terminate_for_idle(
            &mut last_activity,
            Duration::from_secs(1),
            Duration::from_secs(600),
            None,
            &mut dead_since,
            &mut next_poll,
        );

        assert_eq!(should_terminate, Some(IdleTerminationReason::Idle));
        assert!(dead_since.is_none());
        assert!(next_poll.is_none());
    }

    #[test]
    fn initial_response_fast_fails_after_fatal_error_grace() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(temp.path().join("stderr.log"), "invalid_api_key\n").expect("stderr log");
        let last_stdout_activity =
            Instant::now() - FATAL_ERROR_PROGRESS_GRACE - Duration::from_secs(1);
        let mut next_poll = None;

        let should_terminate = should_terminate_for_initial_response(
            last_stdout_activity,
            Duration::from_secs(600),
            Some(temp.path()),
            &mut next_poll,
        );

        assert_eq!(should_terminate, Some(IdleTerminationReason::FatalError));
    }

    #[test]
    fn initial_response_ignores_nonfatal_stderr_until_configured_timeout() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(temp.path().join("stderr.log"), "startup heartbeat\n").expect("stderr log");
        let last_stdout_activity =
            Instant::now() - FATAL_ERROR_PROGRESS_GRACE - Duration::from_secs(1);
        let mut next_poll = None;

        let should_terminate = should_terminate_for_initial_response(
            last_stdout_activity,
            Duration::from_secs(600),
            Some(temp.path()),
            &mut next_poll,
        );

        assert_eq!(should_terminate, None);
        assert!(next_poll.is_some());
    }

    #[test]
    fn initial_response_liveness_probe_waits_for_next_poll() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(temp.path().join("stderr.log"), "invalid_api_key\n").expect("stderr log");
        let last_stdout_activity =
            Instant::now() - FATAL_ERROR_PROGRESS_GRACE - Duration::from_secs(1);
        let scheduled_next_poll = Instant::now() + LIVENESS_POLL_INTERVAL;
        let mut next_poll = Some(scheduled_next_poll);

        let should_terminate = should_terminate_for_initial_response(
            last_stdout_activity,
            Duration::from_secs(600),
            Some(temp.path()),
            &mut next_poll,
        );

        assert_eq!(should_terminate, None);
        assert_eq!(next_poll, Some(scheduled_next_poll));
    }

    #[test]
    fn initial_response_timeout_still_fires_without_session_dir() {
        let last_stdout_activity =
            Instant::now() - FATAL_ERROR_PROGRESS_GRACE - Duration::from_secs(1);
        let mut next_poll = None;

        let should_terminate = should_terminate_for_initial_response(
            last_stdout_activity,
            Duration::from_secs(2),
            None,
            &mut next_poll,
        );

        assert_eq!(should_terminate, Some(IdleTerminationReason::Idle));
    }

    #[test]
    fn initial_response_timeout_still_fires_while_liveness_poll_is_throttled() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(temp.path().join("stderr.log"), "invalid_api_key\n").expect("stderr log");
        let last_stdout_activity =
            Instant::now() - FATAL_ERROR_PROGRESS_GRACE - Duration::from_secs(1);
        let mut next_poll = Some(Instant::now() + LIVENESS_POLL_INTERVAL);

        let should_terminate = should_terminate_for_initial_response(
            last_stdout_activity,
            Duration::from_secs(2),
            Some(temp.path()),
            &mut next_poll,
        );

        assert_eq!(should_terminate, Some(IdleTerminationReason::Idle));
    }

    #[test]
    fn fatal_error_note_takes_priority_over_initial_response_note() {
        let (kind, note) = idle_timeout_note(
            false,
            Some(Duration::from_secs(600)),
            IdleTerminationReason::FatalError,
            Duration::from_secs(600),
            Duration::from_secs(60),
        );

        assert_eq!(kind, "fatal backend error");
        assert!(note.contains("fatal backend error"));
    }

    #[test]
    fn with_session_dir_enters_liveness_grace_period_first() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut dead_since = None;
        let mut next_poll = None;
        let mut last_activity = Instant::now() - Duration::from_secs(2);
        let should_terminate = should_terminate_for_idle(
            &mut last_activity,
            Duration::from_secs(1),
            Duration::from_secs(600),
            Some(temp.path()),
            &mut dead_since,
            &mut next_poll,
        );

        assert_eq!(should_terminate, None);
        assert!(dead_since.is_some());
        assert!(next_poll.is_some());
    }

    #[test]
    fn test_idle_timer_resets_when_progress_signal_present() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // Create a lock file for live PID and a fresh output log to simulate
        // concrete progress signal.
        let locks_dir = tmp.path().join("locks");
        std::fs::create_dir_all(&locks_dir).expect("create locks dir");
        std::fs::write(
            locks_dir.join("codex.lock"),
            format!("{{\"pid\": {}}}", std::process::id()),
        )
        .expect("write lock");
        std::fs::write(tmp.path().join("output.log"), "progress").expect("write output");
        std::fs::write(
            tmp.path().join(".liveness.snapshot"),
            "spool_bytes_written=8\nobserved_spool_bytes_written=0",
        )
        .expect("seed snapshot");

        let mut dead_since = Some(Instant::now() - Duration::from_secs(5));
        let mut next_poll = Some(Instant::now() - Duration::from_secs(1));
        let mut last_activity = Instant::now() - Duration::from_secs(10);
        let before = last_activity;

        let terminate = should_terminate_for_idle(
            &mut last_activity,
            Duration::from_secs(1),
            Duration::from_secs(1),
            Some(tmp.path()),
            &mut dead_since,
            &mut next_poll,
        );

        assert_eq!(terminate, None, "should not terminate when tool is alive");
        assert!(
            dead_since.is_none(),
            "progress signal should reset death timer"
        );
        assert!(
            last_activity > before,
            "progress signal should reset idle timer"
        );
    }

    #[test]
    fn test_idle_timer_survives_spool_rotation_with_monotonic_counter() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let locks_dir = tmp.path().join("locks");
        std::fs::create_dir_all(&locks_dir).expect("create locks dir");
        std::fs::write(
            locks_dir.join("codex.lock"),
            format!("{{\"pid\": {}}}", std::process::id()),
        )
        .expect("write lock");
        // After rotation the live output.log may be tiny, but the monotonic
        // counter still proves fresh progress happened.
        std::fs::write(
            tmp.path().join("output.log"),
            "[CSA:TRUNCATED bytes_written=33554500 rotated_at=2026-03-13T00:00:00Z]\n",
        )
        .expect("write rotated output");
        std::fs::write(
            tmp.path().join(".liveness.snapshot"),
            "spool_bytes_written=33554500\nobserved_spool_bytes_written=33554400",
        )
        .expect("seed snapshot");

        let mut dead_since = Some(Instant::now() - Duration::from_secs(5));
        let mut next_poll = Some(Instant::now() - Duration::from_secs(1));
        let mut last_activity = Instant::now() - Duration::from_secs(10);
        let before = last_activity;

        let terminate = should_terminate_for_idle(
            &mut last_activity,
            Duration::from_secs(1),
            Duration::from_secs(1),
            Some(tmp.path()),
            &mut dead_since,
            &mut next_poll,
        );

        assert!(
            terminate.is_none(),
            "rotated spool progress should prevent termination"
        );
        assert!(dead_since.is_none(), "progress should clear death timer");
        assert!(
            last_activity > before,
            "monotonic spool growth should reset idle timer"
        );
    }

    #[test]
    fn fatal_error_marker_fast_fails_before_promoted_idle_timeout() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            tmp.path().join("stderr.log"),
            "provider failed: HTTP 429 Too Many Requests\n",
        )
        .expect("write stderr");

        let mut dead_since = None;
        let mut next_poll = None;
        let mut last_activity =
            Instant::now() - FATAL_ERROR_PROGRESS_GRACE - Duration::from_secs(1);

        let terminate = should_terminate_for_idle(
            &mut last_activity,
            Duration::from_secs(7200),
            Duration::from_secs(600),
            Some(tmp.path()),
            &mut dead_since,
            &mut next_poll,
        );

        assert_eq!(terminate, Some(IdleTerminationReason::FatalError));
        assert!(dead_since.is_none());
    }

    #[test]
    fn fatal_error_marker_waits_for_no_progress_grace() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            tmp.path().join("stderr.log"),
            "provider failed: HTTP 500 Internal Server Error\n",
        )
        .expect("write stderr");

        let mut dead_since = None;
        let mut next_poll = None;
        let mut last_activity =
            Instant::now() - FATAL_ERROR_PROGRESS_GRACE + Duration::from_secs(1);

        let terminate = should_terminate_for_idle(
            &mut last_activity,
            Duration::from_secs(7200),
            Duration::from_secs(600),
            Some(tmp.path()),
            &mut dead_since,
            &mut next_poll,
        );

        assert_eq!(terminate, None);
        assert!(dead_since.is_none());
    }
}
