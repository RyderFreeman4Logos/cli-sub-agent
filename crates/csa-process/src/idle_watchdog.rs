use std::path::Path;
use std::time::{Duration, Instant};

use crate::ToolLiveness;

const LIVENESS_POLL_INTERVAL: Duration = Duration::from_secs(10);

pub(crate) fn should_terminate_for_idle(
    last_activity: Instant,
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

    if session_dir.is_some_and(ToolLiveness::is_alive) {
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
        let should_terminate = should_terminate_for_idle(
            Instant::now() - Duration::from_secs(2),
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
        let should_terminate = should_terminate_for_idle(
            Instant::now() - Duration::from_secs(2),
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
}
