use super::*;

pub(super) fn process_tree_made_cpu_progress(
    process_activity: Option<&mut ProcessTreeActivity>,
) -> bool {
    process_activity.is_some_and(|activity| {
        matches!(activity.observe(), ProcessTreeStatus::AliveWithCpuProgress)
    })
}

pub(super) fn resolve_heartbeat_interval() -> Option<Duration> {
    let raw = std::env::var(HEARTBEAT_INTERVAL_ENV).ok();
    let secs = match raw {
        Some(value) => match value.trim().parse::<u64>() {
            Ok(0) => return None,
            Ok(parsed) => parsed,
            Err(_) => DEFAULT_HEARTBEAT_SECS,
        },
        None => DEFAULT_HEARTBEAT_SECS,
    };
    Some(Duration::from_secs(secs))
}

/// Indicates which timeout phase the heartbeat is reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TimeoutPhase {
    /// Waiting for the first response from the backend tool.
    InitialResponse,
    /// Normal idle timeout (after first output received, or no initial-response-timeout configured).
    Idle,
}

pub(super) fn maybe_emit_heartbeat(
    heartbeat_interval: Option<Duration>,
    execution_start: Instant,
    last_activity: Instant,
    last_heartbeat: &mut Instant,
    effective_timeout: Duration,
    phase: TimeoutPhase,
) {
    let Some(interval) = heartbeat_interval else {
        return;
    };

    let now = Instant::now();
    let idle_for = now.saturating_duration_since(last_activity);
    if idle_for < interval {
        return;
    }
    if now.saturating_duration_since(*last_heartbeat) < interval {
        return;
    }

    let elapsed = now.saturating_duration_since(execution_start);
    let phase_label = match phase {
        TimeoutPhase::InitialResponse => "initial-response-timeout",
        TimeoutPhase::Idle => "idle-timeout",
    };
    eprintln!(
        "[csa-heartbeat] ACP prompt still running: elapsed={}s idle={}s {phase_label}={}s",
        elapsed.as_secs(),
        idle_for.as_secs(),
        effective_timeout.as_secs()
    );
    *last_heartbeat = now;
}

pub(super) fn stop_reason_to_string(reason: StopReason) -> String {
    match reason {
        StopReason::EndTurn => "end_turn".to_string(),
        StopReason::MaxTokens => "max_tokens".to_string(),
        StopReason::MaxTurnRequests => "max_turn_requests".to_string(),
        StopReason::Refusal => "refusal".to_string(),
        StopReason::Cancelled => "cancelled".to_string(),
        _ => "unknown".to_string(),
    }
}
