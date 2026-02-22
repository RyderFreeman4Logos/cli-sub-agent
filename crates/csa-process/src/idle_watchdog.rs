use std::path::Path;
use std::time::{Duration, Instant};

use crate::ToolLiveness;

const LIVENESS_POLL_INTERVAL: Duration = Duration::from_secs(10);

/// Check whether an idle tool process should be terminated.
///
/// When the tool has been silent (no stdout/stderr) for `idle_timeout`, this
/// function queries [`ToolLiveness::is_alive`] before killing.  If the tool is
/// still alive (filesystem activity, live PID, etc.) the idle timer is **reset**
/// via `last_activity`, giving the tool another full `idle_timeout` window.
/// Termination only happens when liveness returns false continuously for
/// `liveness_dead_timeout`.
pub(crate) fn should_terminate_for_idle(
    last_activity: &mut Instant,
    idle_timeout: Duration,
    liveness_dead_timeout: Duration,
    session_dir: Option<&Path>,
    liveness_dead_since: &mut Option<Instant>,
    next_liveness_poll_at: &mut Option<Instant>,
) -> bool {
    if last_activity.elapsed() < idle_timeout {
        *liveness_dead_since = None;
        *next_liveness_poll_at = None;
        return false;
    }

    // Legacy execute_in path has no spool/session directory context.
    // Preserve original behavior: idle-timeout means immediate termination.
    if session_dir.is_none() {
        return true;
    }

    let now = Instant::now();
    let should_poll = next_liveness_poll_at
        .as_ref()
        .is_none_or(|next_poll| now >= *next_poll);
    if !should_poll {
        return false;
    }

    if session_dir.is_some_and(|dir| ToolLiveness::is_alive(dir) || ToolLiveness::is_working(dir)) {
        // Tool is alive: reset the idle timer so it gets another full window.
        *last_activity = now;
        *liveness_dead_since = None;
        *next_liveness_poll_at = Some(now + LIVENESS_POLL_INTERVAL);
        return false;
    }

    let dead_since = liveness_dead_since.get_or_insert(now);
    if dead_since.elapsed() >= liveness_dead_timeout {
        return true;
    }

    let liveness_dead_deadline = *dead_since + liveness_dead_timeout;
    *next_liveness_poll_at = Some((now + LIVENESS_POLL_INTERVAL).min(liveness_dead_deadline));
    false
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

        assert!(should_terminate);
        assert!(dead_since.is_none());
        assert!(next_poll.is_none());
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

        assert!(!should_terminate);
        assert!(dead_since.is_some());
        assert!(next_poll.is_some());
    }

    #[test]
    fn test_idle_timer_resets_when_liveness_alive() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // Create a fresh lock file with our own PID so is_alive() returns true
        let locks_dir = tmp.path().join("locks");
        std::fs::create_dir_all(&locks_dir).expect("create locks dir");
        std::fs::write(
            locks_dir.join("codex.lock"),
            format!("{{\"pid\": {}}}", std::process::id()),
        )
        .expect("write lock");

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

        assert!(!terminate, "should not terminate when tool is alive");
        assert!(
            dead_since.is_none(),
            "liveness=true should reset death timer"
        );
        assert!(
            last_activity > before,
            "liveness=true should reset idle timer"
        );
    }
}
