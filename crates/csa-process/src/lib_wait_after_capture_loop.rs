use anyhow::{Context, Result};
use std::{
    path::Path,
    time::{Duration, Instant},
};
use tokio::{process::Child, time::MissedTickBehavior};

use crate::{
    ExecutionCancellation, IDLE_POLL_INTERVAL, IdleWatchdogState, SpawnOptions, idle_timeout_note,
    maybe_emit_heartbeat, should_terminate_for_idle_with_state,
    should_terminate_for_initial_response_with_state, terminate_child_process_group,
};

#[macro_export]
macro_rules! output_eof_wait {
    ($child:expr, $received:expr, $activity:expr, $stdout_activity:expr, $start:expr, $heartbeat:expr, $idle:expr, $initial:expr, $liveness:expr, $session_dir:expr, $interval:expr, $watchdog:expr, $options:expr, $grace:expr $(,)?) => {
        $crate::wait_after_capture::OutputEofWait {
            child: $child,
            received_first_output: $received,
            last_activity: $activity,
            last_stdout_activity: $stdout_activity,
            execution_start: $start,
            last_heartbeat: $heartbeat,
            idle_timeout: $idle,
            initial_response_timeout: $initial,
            liveness_dead_timeout: $liveness,
            session_dir: $session_dir,
            heartbeat_interval: $interval,
            idle_watchdog_state: $watchdog,
            spawn_options: $options,
            termination_grace_period: $grace,
        }
    };
}

pub(crate) struct OutputEofWait<'a> {
    pub(crate) child: &'a mut Child,
    pub(crate) received_first_output: bool,
    pub(crate) last_activity: &'a mut Instant,
    pub(crate) last_stdout_activity: Instant,
    pub(crate) execution_start: Instant,
    pub(crate) last_heartbeat: &'a mut Instant,
    pub(crate) idle_timeout: Duration,
    pub(crate) initial_response_timeout: Option<Duration>,
    pub(crate) liveness_dead_timeout: Duration,
    pub(crate) session_dir: Option<&'a Path>,
    pub(crate) heartbeat_interval: Option<Duration>,
    pub(crate) idle_watchdog_state: &'a mut IdleWatchdogState,
    pub(crate) spawn_options: &'a SpawnOptions,
    pub(crate) termination_grace_period: Duration,
}

pub(crate) async fn wait_after_output_eof(
    input: OutputEofWait<'_>,
) -> Result<(std::process::ExitStatus, Option<String>)> {
    let mut tick = tokio::time::interval(IDLE_POLL_INTERVAL);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        if let Some(status) = input
            .child
            .try_wait()
            .context("Failed to poll command after output EOF")?
        {
            return Ok((status, None));
        }
        if input
            .spawn_options
            .cancellation
            .as_ref()
            .is_some_and(ExecutionCancellation::is_cancelled)
        {
            let status = terminate_child_process_group(input.child, input.termination_grace_period)
                .await
                .context("Failed to reap cancelled command")?;
            return Ok((status, None));
        }
        let effective_idle = if !input.received_first_output {
            input.initial_response_timeout.unwrap_or(input.idle_timeout)
        } else {
            input.idle_timeout
        };
        maybe_emit_heartbeat(
            input.heartbeat_interval,
            input.execution_start,
            *input.last_activity,
            input.last_heartbeat,
            effective_idle,
        );
        let reason = if !input.received_first_output && input.initial_response_timeout.is_some() {
            should_terminate_for_initial_response_with_state(
                input.last_stdout_activity,
                effective_idle,
                input.session_dir,
                input.idle_watchdog_state,
                input.spawn_options.error_marker_scan_enabled,
            )
        } else {
            should_terminate_for_idle_with_state(
                input.last_activity,
                effective_idle,
                input.liveness_dead_timeout,
                input.session_dir,
                input.idle_watchdog_state,
                input.spawn_options.error_marker_scan_enabled,
            )
        };
        if let Some(reason) = reason {
            let (_, note) = idle_timeout_note(
                input.received_first_output,
                input.initial_response_timeout,
                reason,
                effective_idle,
                input.liveness_dead_timeout,
            );
            let status = terminate_child_process_group(input.child, input.termination_grace_period)
                .await
                .context("Failed to reap timed-out command")?;
            return Ok((status, Some(note)));
        }
        tick.tick().await;
    }
}
