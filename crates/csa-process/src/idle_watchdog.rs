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
