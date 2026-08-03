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

#[allow(clippy::too_many_arguments)]
pub(crate) async fn wait_after_output_eof(
    child: &mut Child,
    received_first_output: bool,
    last_activity: &mut Instant,
    last_stdout_activity: Instant,
    execution_start: Instant,
    last_heartbeat: &mut Instant,
    idle_timeout: Duration,
    initial_response_timeout: Option<Duration>,
    liveness_dead_timeout: Duration,
    session_dir: Option<&Path>,
    heartbeat_interval: Option<Duration>,
    idle_watchdog_state: &mut IdleWatchdogState,
    spawn_options: &SpawnOptions,
    termination_grace_period: Duration,
) -> Result<(std::process::ExitStatus, Option<String>)> {
    let mut tick = tokio::time::interval(IDLE_POLL_INTERVAL);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        if let Some(status) = child
            .try_wait()
            .context("Failed to poll command after output EOF")?
        {
            return Ok((status, None));
        }
        if spawn_options
            .cancellation
            .as_ref()
            .is_some_and(ExecutionCancellation::is_cancelled)
        {
            let status = terminate_child_process_group(child, termination_grace_period)
                .await
                .context("Failed to reap cancelled command")?;
            return Ok((status, None));
        }
        let effective_idle = if !received_first_output {
            initial_response_timeout.unwrap_or(idle_timeout)
        } else {
            idle_timeout
        };
        maybe_emit_heartbeat(
            heartbeat_interval,
            execution_start,
            *last_activity,
            last_heartbeat,
            effective_idle,
        );
        let reason = if !received_first_output && initial_response_timeout.is_some() {
            should_terminate_for_initial_response_with_state(
                last_stdout_activity,
                effective_idle,
                session_dir,
                idle_watchdog_state,
                spawn_options.error_marker_scan_enabled,
            )
        } else {
            should_terminate_for_idle_with_state(
                last_activity,
                effective_idle,
                liveness_dead_timeout,
                session_dir,
                idle_watchdog_state,
                spawn_options.error_marker_scan_enabled,
            )
        };
        if let Some(reason) = reason {
            let (_, note) = idle_timeout_note(
                received_first_output,
                initial_response_timeout,
                reason,
                effective_idle,
                liveness_dead_timeout,
            );
            let status = terminate_child_process_group(child, termination_grace_period)
                .await
                .context("Failed to reap timed-out command")?;
            return Ok((status, Some(note)));
        }
        tick.tick().await;
    }
}
