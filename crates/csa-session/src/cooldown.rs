//! Inter-session cooldown enforcement.
//!
//! Prevents compounding memory pressure from rapid sequential session launches
//! on the same project. When the most recent session for a project exited within
//! `cooldown_seconds` ago, the new session launch is delayed by the remaining
//! cooldown time.

use chrono::{DateTime, Utc};
use std::time::Duration;
use tracing::info;

/// Default cooldown period between consecutive sessions (seconds).
pub const DEFAULT_COOLDOWN_SECONDS: u64 = 10;

/// Outcome of a cooldown evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CooldownAction {
    /// No cooldown needed — either no recent session or enough time has passed.
    Proceed,
    /// Must wait for the given duration before launching.
    Wait(Duration),
}

/// Evaluate whether a cooldown delay is needed before launching a new session.
///
/// Pure function: takes the current time and the last session's exit time,
/// returns the action to take.
///
/// - `last_session_ended_at`: when the most recent session for this project
///   last updated its `last_accessed` timestamp.
/// - `now`: current wall-clock time.
/// - `cooldown_seconds`: configured cooldown period.
pub fn evaluate_cooldown(
    last_session_ended_at: DateTime<Utc>,
    now: DateTime<Utc>,
    cooldown_seconds: u64,
) -> CooldownAction {
    if cooldown_seconds == 0 {
        return CooldownAction::Proceed;
    }

    let cooldown = chrono::Duration::seconds(cooldown_seconds as i64);
    let elapsed = now.signed_duration_since(last_session_ended_at);

    if elapsed < cooldown {
        let remaining = cooldown - elapsed;
        // Clamp to non-negative (defensive against clock skew)
        let remaining_std = remaining
            .to_std()
            .unwrap_or(Duration::from_secs(cooldown_seconds));
        CooldownAction::Wait(remaining_std)
    } else {
        CooldownAction::Proceed
    }
}

/// Sleep for the cooldown period if needed, logging a message.
///
/// Returns `Ok(true)` if a cooldown wait was performed, `Ok(false)` if not.
pub fn enforce_cooldown_sync(last_session_ended_at: DateTime<Utc>, cooldown_seconds: u64) -> bool {
    let now = Utc::now();
    match evaluate_cooldown(last_session_ended_at, now, cooldown_seconds) {
        CooldownAction::Proceed => false,
        CooldownAction::Wait(duration) => {
            info!(
                wait_secs = duration.as_secs(),
                "Cooldown: waiting {}s for memory recovery after previous session",
                duration.as_secs()
            );
            std::thread::sleep(duration);
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn utc(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(secs, 0).unwrap()
    }

    #[test]
    fn test_no_cooldown_when_zero_configured() {
        let action = evaluate_cooldown(utc(100), utc(101), 0);
        assert_eq!(action, CooldownAction::Proceed);
    }

    #[test]
    fn test_no_cooldown_when_enough_time_passed() {
        // Session ended at t=100, now is t=120, cooldown=10s → 20s elapsed > 10s
        let action = evaluate_cooldown(utc(100), utc(120), 10);
        assert_eq!(action, CooldownAction::Proceed);
    }

    #[test]
    fn test_cooldown_wait_when_too_soon() {
        // Session ended at t=100, now is t=103, cooldown=10s → 3s elapsed, need 7s more
        let action = evaluate_cooldown(utc(100), utc(103), 10);
        assert_eq!(action, CooldownAction::Wait(Duration::from_secs(7)));
    }

    #[test]
    fn test_cooldown_exact_boundary_proceeds() {
        // Session ended at t=100, now is t=110, cooldown=10s → exactly 10s elapsed
        let action = evaluate_cooldown(utc(100), utc(110), 10);
        assert_eq!(action, CooldownAction::Proceed);
    }

    #[test]
    fn test_cooldown_one_second_before_boundary() {
        // Session ended at t=100, now is t=109, cooldown=10s → 9s elapsed, need 1s more
        let action = evaluate_cooldown(utc(100), utc(109), 10);
        assert_eq!(action, CooldownAction::Wait(Duration::from_secs(1)));
    }

    #[test]
    fn test_cooldown_with_clock_skew_backward() {
        // Clock went backward: "now" is before session ended.
        // elapsed = -10s, remaining = cooldown - elapsed = 10 - (-10) = 20s.
        // This is conservative: we wait longer rather than proceeding unsafely.
        let action = evaluate_cooldown(utc(110), utc(100), 10);
        assert_eq!(action, CooldownAction::Wait(Duration::from_secs(20)));
    }

    #[test]
    fn test_cooldown_large_configured_value() {
        // Session ended at t=100, now is t=105, cooldown=60s → need 55s more
        let action = evaluate_cooldown(utc(100), utc(105), 60);
        assert_eq!(action, CooldownAction::Wait(Duration::from_secs(55)));
    }
}
