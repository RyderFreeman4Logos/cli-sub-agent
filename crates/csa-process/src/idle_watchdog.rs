use std::path::Path;
use std::time::{Duration, Instant};

use crate::ToolLiveness;
use crate::tool_liveness::ProviderErrorKind;

const LIVENESS_POLL_INTERVAL: Duration = Duration::from_secs(10);
const FATAL_ERROR_PROGRESS_GRACE: Duration = Duration::from_secs(30);
const TRANSIENT_PROVIDER_ERROR_RETRY_BUDGET: u8 = 1;
const TRANSIENT_PROVIDER_ERROR_BACKOFF: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IdleTerminationReason {
    Idle,
    FatalError,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProviderErrorBackoff {
    retries_used: u8,
    retry_after: Option<Instant>,
}

impl ProviderErrorBackoff {
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }

    fn transient_retry_deadline(&mut self, now: Instant) -> Option<Instant> {
        if let Some(retry_after) = self.retry_after
            && now < retry_after
        {
            return Some(retry_after);
        }

        if self.retries_used < TRANSIENT_PROVIDER_ERROR_RETRY_BUDGET {
            self.retries_used += 1;
            let retry_after = now + TRANSIENT_PROVIDER_ERROR_BACKOFF;
            self.retry_after = Some(retry_after);
            return Some(retry_after);
        }

        None
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct IdleWatchdogState {
    pub(crate) liveness_dead_since: Option<Instant>,
    pub(crate) next_liveness_poll_at: Option<Instant>,
    pub(crate) provider_error_backoff: ProviderErrorBackoff,
}

impl IdleWatchdogState {
    pub(crate) fn reset_on_activity(&mut self) {
        self.liveness_dead_since = None;
        self.next_liveness_poll_at = None;
        self.provider_error_backoff.reset();
    }
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
#[cfg(test)]
pub(crate) fn should_terminate_for_initial_response(
    last_stdout_activity: Instant,
    initial_response_timeout: Duration,
    session_dir: Option<&Path>,
    next_liveness_poll_at: &mut Option<Instant>,
    error_marker_scan_enabled: bool,
) -> Option<IdleTerminationReason> {
    let mut state = IdleWatchdogState {
        next_liveness_poll_at: *next_liveness_poll_at,
        ..IdleWatchdogState::default()
    };
    let termination = should_terminate_for_initial_response_with_state(
        last_stdout_activity,
        initial_response_timeout,
        session_dir,
        &mut state,
        error_marker_scan_enabled,
    );
    *next_liveness_poll_at = state.next_liveness_poll_at;
    termination
}

pub(crate) fn should_terminate_for_initial_response_with_state(
    last_stdout_activity: Instant,
    initial_response_timeout: Duration,
    session_dir: Option<&Path>,
    state: &mut IdleWatchdogState,
    error_marker_scan_enabled: bool,
) -> Option<IdleTerminationReason> {
    let stdout_idle_for = last_stdout_activity.elapsed();

    if stdout_idle_for < FATAL_ERROR_PROGRESS_GRACE {
        state.reset_on_activity();
        return (stdout_idle_for >= initial_response_timeout)
            .then_some(IdleTerminationReason::Idle);
    }

    // #1745 stopgap opt-out; proper fix = scope marker scan to backend-transport stream only (#182)
    if error_marker_scan_enabled && let Some(dir) = session_dir {
        let now = Instant::now();
        let should_poll = state
            .next_liveness_poll_at
            .as_ref()
            .is_none_or(|next_poll| now >= *next_poll);
        if should_poll {
            state.next_liveness_poll_at = Some(now + LIVENESS_POLL_INTERVAL);
            let signals = ToolLiveness::probe(dir);
            if let Some(reason) = provider_error_termination(
                signals.provider_error,
                now,
                &mut state.next_liveness_poll_at,
                &mut state.provider_error_backoff,
            ) {
                return Some(reason);
            }
        }
    } else {
        state.provider_error_backoff.reset();
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
#[cfg(test)]
pub(crate) fn should_terminate_for_idle(
    last_activity: &mut Instant,
    idle_timeout: Duration,
    liveness_dead_timeout: Duration,
    session_dir: Option<&Path>,
    liveness_dead_since: &mut Option<Instant>,
    next_liveness_poll_at: &mut Option<Instant>,
    error_marker_scan_enabled: bool,
) -> Option<IdleTerminationReason> {
    let mut state = IdleWatchdogState {
        liveness_dead_since: *liveness_dead_since,
        next_liveness_poll_at: *next_liveness_poll_at,
        ..IdleWatchdogState::default()
    };
    let termination = should_terminate_for_idle_with_state(
        last_activity,
        idle_timeout,
        liveness_dead_timeout,
        session_dir,
        &mut state,
        error_marker_scan_enabled,
    );
    *liveness_dead_since = state.liveness_dead_since;
    *next_liveness_poll_at = state.next_liveness_poll_at;
    termination
}

pub(crate) fn should_terminate_for_idle_with_state(
    last_activity: &mut Instant,
    idle_timeout: Duration,
    liveness_dead_timeout: Duration,
    session_dir: Option<&Path>,
    state: &mut IdleWatchdogState,
    error_marker_scan_enabled: bool,
) -> Option<IdleTerminationReason> {
    let idle_for = last_activity.elapsed();
    if idle_for < idle_timeout && idle_for < FATAL_ERROR_PROGRESS_GRACE {
        state.reset_on_activity();
        return None;
    }

    // Legacy execute_in path has no spool/session directory context.
    // Preserve original behavior: idle-timeout means immediate termination.
    let Some(dir) = session_dir else {
        state.provider_error_backoff.reset();
        return (idle_for >= idle_timeout).then_some(IdleTerminationReason::Idle);
    };

    let now = Instant::now();
    let should_poll = state
        .next_liveness_poll_at
        .as_ref()
        .is_none_or(|next_poll| now >= *next_poll);
    if !should_poll {
        return None;
    }

    let signals = ToolLiveness::probe(dir);
    if signals.has_progress_signal() {
        // Real progress observed: reset idle/death timers and give a fresh window.
        *last_activity = now;
        state.liveness_dead_since = None;
        state.next_liveness_poll_at = Some(now + LIVENESS_POLL_INTERVAL);
        state.provider_error_backoff.reset();
        return None;
    }

    // #1745 stopgap opt-out; proper fix = scope marker scan to backend-transport stream only (#182)
    if error_marker_scan_enabled && idle_for >= FATAL_ERROR_PROGRESS_GRACE {
        if let Some(reason) = provider_error_termination(
            signals.provider_error,
            now,
            &mut state.next_liveness_poll_at,
            &mut state.provider_error_backoff,
        ) {
            return Some(reason);
        }
    } else {
        state.provider_error_backoff.reset();
    }

    if idle_for < idle_timeout {
        state.liveness_dead_since = None;
        state.next_liveness_poll_at = Some(now + LIVENESS_POLL_INTERVAL);
        return None;
    }

    let dead_since = state.liveness_dead_since.get_or_insert(now);
    if dead_since.elapsed() >= liveness_dead_timeout {
        return Some(IdleTerminationReason::Idle);
    }

    let liveness_dead_deadline = *dead_since + liveness_dead_timeout;
    state.next_liveness_poll_at = Some((now + LIVENESS_POLL_INTERVAL).min(liveness_dead_deadline));
    None
}

fn provider_error_termination(
    provider_error: Option<ProviderErrorKind>,
    now: Instant,
    next_liveness_poll_at: &mut Option<Instant>,
    provider_error_backoff: &mut ProviderErrorBackoff,
) -> Option<IdleTerminationReason> {
    match provider_error {
        Some(ProviderErrorKind::Permanent) => Some(IdleTerminationReason::FatalError),
        Some(ProviderErrorKind::Transient) => {
            if let Some(retry_after) = provider_error_backoff.transient_retry_deadline(now) {
                *next_liveness_poll_at = Some(retry_after);
                None
            } else {
                Some(IdleTerminationReason::FatalError)
            }
        }
        None => {
            provider_error_backoff.reset();
            None
        }
    }
}

#[cfg(test)]
#[path = "idle_watchdog_provider_error_tests.rs"]
mod provider_error_tests;

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
            true,
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
            true,
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
            true,
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
            true,
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
            true,
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
            true,
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
            true,
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
            true,
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
            true,
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
        std::fs::write(tmp.path().join("stderr.log"), "invalid_api_key\n").expect("write stderr");

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
            true,
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
            true,
        );

        assert_eq!(terminate, None);
        assert!(dead_since.is_none());
    }

    #[test]
    fn fatal_error_marker_scan_disabled_does_not_kill_idle(/* #1745 opt-out */) {
        let tmp = tempfile::tempdir().expect("tempdir");
        // Same marker + same no-progress condition as
        // `fatal_error_marker_fast_fails_before_promoted_idle_timeout`, but with
        // the scan disabled: the marker-based fatal classification MUST NOT fire.
        std::fs::write(
            tmp.path().join("stderr.log"),
            "provider failed: HTTP 429 Too Many Requests\n",
        )
        .expect("write stderr");

        let mut dead_since = None;
        let mut next_poll = None;
        // Idle for the fatal grace but still well under the promoted idle timeout,
        // so the only thing that could terminate here is the marker fast-fail.
        let mut last_activity =
            Instant::now() - FATAL_ERROR_PROGRESS_GRACE - Duration::from_secs(1);

        let terminate = should_terminate_for_idle(
            &mut last_activity,
            Duration::from_secs(7200),
            Duration::from_secs(600),
            Some(tmp.path()),
            &mut dead_since,
            &mut next_poll,
            false,
        );

        assert_eq!(
            terminate, None,
            "scan disabled must bypass marker-based fatal kill"
        );
        assert!(dead_since.is_none());
    }

    #[test]
    fn initial_response_scan_disabled_ignores_fatal_marker(/* #1745 opt-out */) {
        let temp = tempfile::tempdir().expect("tempdir");
        // Mirrors `initial_response_fast_fails_after_fatal_error_grace` but with the
        // scan disabled: a fatal marker on stderr must NOT trigger a FatalError; the
        // configured initial-response timeout (here large) governs instead.
        std::fs::write(temp.path().join("stderr.log"), "invalid_api_key\n").expect("stderr log");
        let last_stdout_activity =
            Instant::now() - FATAL_ERROR_PROGRESS_GRACE - Duration::from_secs(1);
        let mut next_poll = None;

        let should_terminate = should_terminate_for_initial_response(
            last_stdout_activity,
            Duration::from_secs(600),
            Some(temp.path()),
            &mut next_poll,
            false,
        );

        assert_eq!(
            should_terminate, None,
            "scan disabled must not fast-fail on initial-response fatal marker"
        );
    }
}
