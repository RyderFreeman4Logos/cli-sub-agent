use super::*;

/// Wait for a spawned child process, capturing output and enforcing idle-timeout.
///
/// The process is killed only when there is no stdout/stderr output for the full
/// `idle_timeout` duration.
///
/// When `output_spool` is `Some`, each stdout chunk is also written to the given
/// file path with an explicit flush after each write.  This ensures partial output
/// survives OOM kills or other ungraceful terminations — the caller can recover
/// output from the spool file even if this function never returns.
#[expect(
    clippy::too_many_arguments,
    reason = "timeout params are flat for caller convenience"
)]
pub async fn wait_and_capture_with_idle_timeout(
    mut child: tokio::process::Child,
    stream_mode: StreamMode,
    idle_timeout: Duration,
    liveness_dead_timeout: Duration,
    termination_grace_period: Duration,
    output_spool: Option<&Path>,
    spawn_options: SpawnOptions,
    initial_response_timeout: Option<Duration>,
) -> Result<ExecutionResult> {
    let child_pid = child.id();
    let stdout = child.stdout.take().context("Failed to capture stdout")?;
    let stderr = child.stderr.take();

    let mut spool_file = None;
    if let Some(path) = output_spool {
        match SpoolRotator::open(
            path,
            spawn_options.spool_max_bytes,
            spawn_options.keep_rotated_spool,
        ) {
            Ok(rotator) => {
                spool_file = Some(rotator);
            }
            Err(e) => {
                warn!(path = %path.display(), error = %e, "Failed to open output spool file");
            }
        }
    }
    let session_dir = output_spool.and_then(Path::parent);
    let mut stderr_spool_file = None;
    if let Some(dir) = session_dir {
        let path = dir.join("stderr.log");
        match SpoolRotator::open(
            &path,
            spawn_options.spool_max_bytes,
            spawn_options.keep_rotated_spool,
        ) {
            Ok(rotator) => {
                stderr_spool_file = Some(rotator);
            }
            Err(e) => {
                warn!(
                    path = %path.display(),
                    error = %e,
                    "Failed to open stderr spool file"
                );
            }
        }
    }

    const READ_BUF_SIZE: usize = 4096;
    let mut stdout_reader = BufReader::new(stdout);
    let mut output = String::new();
    let mut stdout_line_buf = String::new();

    let mut stderr_output = String::new();
    let execution_start = Instant::now();
    let mut last_activity = Instant::now();
    let last_stdout_activity = last_activity;
    let mut last_heartbeat = execution_start;
    let heartbeat_interval = resolve_heartbeat_interval();
    let mut liveness_dead_since: Option<Instant> = None;
    let mut next_liveness_poll_at: Option<Instant> = None;
    let mut received_first_output = false;
    let mut idle_timed_out = false;
    let mut workspace_boundary_timed_out = false;
    let mut workspace_boundary_error_hits = 0usize;
    let mut workspace_boundary_warned = false;
    let workspace_boundary_threshold = resolve_workspace_boundary_threshold();
    let mut timeout_note = String::new();
    let workspace_boundary_note = format!(
        "workspace boundary hits crossed threshold {workspace_boundary_threshold}; session continued (non-fatal)"
    );
    let mut persistent_rate_limit_note: Option<String> = None;
    let mut persistent_rate_limit_tracker = PersistentRateLimitTracker::default();
    let mut child_exited_early = false;
    let mut child_exited_early_note = String::new();
    let mut zombie_first_detected_at: Option<Instant> = None;
    let mut child_wait_consumed = false;
    macro_rules! kill_on_persistent_rate_limit {
        ($appended:expr, $stream:literal) => {
            if let Some(note) = persistent_rate_limit_tracker.observe_appended_output($appended) {
                warn!(
                    reason = %note,
                    stream = $stream,
                    "Killing child due to persistent repeated 429/quota output"
                );
                persistent_rate_limit_note = Some(note);
                terminate_child_process_group(&mut child, termination_grace_period).await;
                break;
            }
        };
    }

    if let Some(stderr_handle) = stderr {
        let mut stderr_reader = BufReader::new(stderr_handle);
        let mut stderr_line_buf = String::new();

        let mut stdout_done = false;
        let mut stderr_done = false;
        let mut stdout_buf = [0u8; READ_BUF_SIZE];
        let mut stderr_buf = [0u8; READ_BUF_SIZE];
        let mut watchdog_tick = tokio::time::interval(IDLE_POLL_INTERVAL);
        watchdog_tick.set_missed_tick_behavior(MissedTickBehavior::Delay);

        while !stdout_done || !stderr_done {
            tokio::select! {
                result = stdout_reader.read(&mut stdout_buf), if !stdout_done => {
                    match result {
                        Ok(0) => {
                            flush_line_buf(&mut stdout_line_buf, &mut output, stream_mode);
                            stdout_done = true;
                        }
                        Ok(n) => {
                            received_first_output = true;
                            last_activity = Instant::now();
                            last_heartbeat = last_activity;
                            liveness_dead_since = None;
                            next_liveness_poll_at = None;
                            let chunk = String::from_utf8_lossy(&stdout_buf[..n]);
                            spool_chunk(&mut spool_file, &stdout_buf[..n]);
                            if let (Some(dir), Some(spool)) = (session_dir, spool_file.as_ref()) {
                                record_spool_bytes_written(dir, spool.bytes_written());
                            }
                            let previous_output_len = output.len();
                            workspace_boundary_error_hits += accumulate_and_flush_lines(
                                &chunk,
                                &mut stdout_line_buf,
                                &mut output,
                                stream_mode,
                            );
                            kill_on_persistent_rate_limit!(&output[previous_output_len..], "stdout");
                            drain_if_over_high_water(&mut output);
                            note_workspace_boundary_threshold(
                                workspace_boundary_error_hits,
                                workspace_boundary_threshold,
                                &mut workspace_boundary_warned,
                                &mut workspace_boundary_timed_out,
                                &mut output,
                            );
                        }
                        Err(_) => {
                            flush_line_buf(&mut stdout_line_buf, &mut output, stream_mode);
                            stdout_done = true;
                        }
                    }
                }
                result = stderr_reader.read(&mut stderr_buf), if !stderr_done => {
                    match result {
                        Ok(0) => {
                            flush_stderr_buf(
                                &mut stderr_line_buf,
                                &mut stderr_output,
                                stream_mode,
                            );
                            stderr_done = true;
                        }
                        Ok(n) => {
                            // NOTE: Do NOT set received_first_output here.
                            // Only stdout counts as "first output" — stderr
                            // often contains diagnostic banners (e.g. systemd-run's
                            // "Running scope as unit...") that should not reset
                            // the initial_response_timeout.
                            last_activity = Instant::now();
                            last_heartbeat = last_activity;
                            liveness_dead_since = None;
                            next_liveness_poll_at = None;
                            let chunk = String::from_utf8_lossy(&stderr_buf[..n]);
                            spool_chunk(&mut stderr_spool_file, &stderr_buf[..n]);
                            let previous_stderr_len = stderr_output.len();
                            workspace_boundary_error_hits += accumulate_and_flush_stderr(
                                &chunk,
                                &mut stderr_line_buf,
                                &mut stderr_output,
                                stream_mode,
                            );
                            kill_on_persistent_rate_limit!(&stderr_output[previous_stderr_len..], "stderr");
                            drain_if_over_high_water(&mut stderr_output);
                            note_workspace_boundary_threshold(
                                workspace_boundary_error_hits,
                                workspace_boundary_threshold,
                                &mut workspace_boundary_warned,
                                &mut workspace_boundary_timed_out,
                                &mut output,
                            );
                        }
                        Err(_) => {
                            flush_stderr_buf(
                                &mut stderr_line_buf,
                                &mut stderr_output,
                                stream_mode,
                            );
                            stderr_done = true;
                        }
                    }
                }
                _ = watchdog_tick.tick() => {
                    let effective_idle = if !received_first_output {
                        initial_response_timeout.unwrap_or(idle_timeout)
                    } else {
                        idle_timeout
                    };
                    maybe_emit_heartbeat(
                        heartbeat_interval,
                        execution_start,
                        last_activity,
                        &mut last_heartbeat,
                        effective_idle,
                    );
                    let idle_termination = if !received_first_output && initial_response_timeout.is_some() {
                        should_terminate_for_initial_response(
                            last_stdout_activity,
                            effective_idle,
                            session_dir,
                            &mut next_liveness_poll_at,
                        )
                    } else {
                        should_terminate_for_idle(
                            &mut last_activity,
                            effective_idle,
                            liveness_dead_timeout,
                            session_dir,
                            &mut liveness_dead_since,
                            &mut next_liveness_poll_at,
                        )
                    };
                    if let Some(reason) = idle_termination {
                        idle_timed_out = true;
                        let (timeout_kind, note) = idle_timeout_note(
                            received_first_output,
                            initial_response_timeout,
                            reason,
                            effective_idle,
                            liveness_dead_timeout,
                        );
                        timeout_note = note;
                        warn!(
                            timeout_secs = effective_idle.as_secs(),
                            timeout_kind,
                            "Killing child due to {timeout_kind}"
                        );
                        terminate_child_process_group(&mut child, termination_grace_period).await;
                        break;
                    }
                    if !stdout_done && poll_child_exited(&mut child, &mut child_wait_consumed) {
                        let first = zombie_first_detected_at.get_or_insert_with(Instant::now);
                        if first.elapsed() >= IDLE_POLL_INTERVAL {
                            child_exited_early = true;
                            child_exited_early_note = format!(
                                "child process (pid {}) exited while stdout pipe still open; \
                                 possible auto-compaction spawned a subprocess that inherited stdout — \
                                 CSA detected process death and broke out of the read loop early",
                                child_pid.unwrap_or(0)
                            );
                            warn!(pid = child_pid.unwrap_or(0), "child process exited while stdout pipe still open; breaking read loop");
                            break;
                        }
                    } else if !stdout_done {
                        zombie_first_detected_at = None;
                    }
                }
            }
        }
    } else {
        let mut stdout_buf = [0u8; READ_BUF_SIZE];
        let mut watchdog_tick = tokio::time::interval(IDLE_POLL_INTERVAL);
        watchdog_tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                result = stdout_reader.read(&mut stdout_buf) => {
                    match result {
                        Ok(0) => {
                            flush_line_buf(&mut stdout_line_buf, &mut output, stream_mode);
                            break;
                        }
                        Ok(n) => {
                            received_first_output = true;
                            last_activity = Instant::now();
                            last_heartbeat = last_activity;
                            liveness_dead_since = None;
                            next_liveness_poll_at = None;
                            let chunk = String::from_utf8_lossy(&stdout_buf[..n]);
                            spool_chunk(&mut spool_file, &stdout_buf[..n]);
                            if let (Some(dir), Some(spool)) = (session_dir, spool_file.as_ref()) {
                                record_spool_bytes_written(dir, spool.bytes_written());
                            }
                            let previous_output_len = output.len();
                            workspace_boundary_error_hits += accumulate_and_flush_lines(
                                &chunk,
                                &mut stdout_line_buf,
                                &mut output,
                                stream_mode,
                            );
                            kill_on_persistent_rate_limit!(&output[previous_output_len..], "stdout");
                            drain_if_over_high_water(&mut output);
                            note_workspace_boundary_threshold(
                                workspace_boundary_error_hits,
                                workspace_boundary_threshold,
                                &mut workspace_boundary_warned,
                                &mut workspace_boundary_timed_out,
                                &mut output,
                            );
                        }
                        Err(_) => {
                            flush_line_buf(&mut stdout_line_buf, &mut output, stream_mode);
                            break;
                        }
                    }
                }
                _ = watchdog_tick.tick() => {
                    let effective_idle = if !received_first_output {
                        initial_response_timeout.unwrap_or(idle_timeout)
                    } else {
                        idle_timeout
                    };
                    maybe_emit_heartbeat(
                        heartbeat_interval,
                        execution_start,
                        last_activity,
                        &mut last_heartbeat,
                        effective_idle,
                    );
                    let idle_termination = if !received_first_output && initial_response_timeout.is_some() {
                        should_terminate_for_initial_response(
                            last_stdout_activity,
                            effective_idle,
                            session_dir,
                            &mut next_liveness_poll_at,
                        )
                    } else {
                        should_terminate_for_idle(
                            &mut last_activity,
                            effective_idle,
                            liveness_dead_timeout,
                            session_dir,
                            &mut liveness_dead_since,
                            &mut next_liveness_poll_at,
                        )
                    };
                    if let Some(reason) = idle_termination {
                        idle_timed_out = true;
                        let (timeout_kind, note) = idle_timeout_note(
                            received_first_output,
                            initial_response_timeout,
                            reason,
                            effective_idle,
                            liveness_dead_timeout,
                        );
                        timeout_note = note;
                        warn!(
                            timeout_secs = effective_idle.as_secs(),
                            timeout_kind,
                            "Killing child due to {timeout_kind}"
                        );
                        terminate_child_process_group(&mut child, termination_grace_period).await;
                        break;
                    }
                    if poll_child_exited(&mut child, &mut child_wait_consumed) {
                        let first = zombie_first_detected_at.get_or_insert_with(Instant::now);
                        if first.elapsed() >= IDLE_POLL_INTERVAL {
                            child_exited_early = true;
                            child_exited_early_note = format!(
                                "child process (pid {}) exited while stdout pipe still open; \
                                 possible auto-compaction spawned a subprocess that inherited stdout — \
                                 CSA detected process death and broke out of the read loop early",
                                child_pid.unwrap_or(0)
                            );
                            warn!(pid = child_pid.unwrap_or(0), "child process exited while stdout pipe still open; breaking read loop");
                            break;
                        }
                    } else {
                        zombie_first_detected_at = None;
                    }
                }
            }
        }
    }

    let status = child.wait().await.context("Failed to wait for command")?;

    let mut exit_code = status.code().unwrap_or_else(|| {
        warn!("Process terminated by signal, using exit code 1");
        1
    });
    if let Some(note) = persistent_rate_limit_note.as_deref() {
        exit_code = 1;
        if !stderr_output.is_empty() && !stderr_output.ends_with('\n') {
            stderr_output.push('\n');
        }
        stderr_output.push_str(note);
        stderr_output.push('\n');
    } else if idle_timed_out {
        exit_code = 137;
        if !stderr_output.is_empty() && !stderr_output.ends_with('\n') {
            stderr_output.push('\n');
        }
        stderr_output.push_str(&timeout_note);
        stderr_output.push('\n');
    } else if child_exited_early {
        if exit_code == 0 {
            exit_code = 1;
        }
        if !stderr_output.is_empty() && !stderr_output.ends_with('\n') {
            stderr_output.push('\n');
        }
        stderr_output.push_str(&child_exited_early_note);
        stderr_output.push('\n');
    } else if workspace_boundary_timed_out {
        if !stderr_output.is_empty() && !stderr_output.ends_with('\n') {
            stderr_output.push('\n');
        }
        stderr_output.push_str(&workspace_boundary_note);
        stderr_output.push('\n');
    }

    let summary = if let Some(note) = persistent_rate_limit_note {
        note
    } else if idle_timed_out {
        timeout_note
    } else if child_exited_early {
        child_exited_early_note.clone()
    } else if exit_code == 0 {
        extract_summary(&output)
    } else if workspace_boundary_timed_out {
        workspace_boundary_note
    } else {
        failure_summary(&output, &stderr_output, exit_code)
    };

    // Session-outcome signals for the classifier, derived from the raw output
    // before sanitization. Timeouts force non-completion (we killed the turn);
    // otherwise an explicit terminal envelope (codex `turn.completed`,
    // claude-code `result`) is authoritative, even if the process then exited
    // "early". With no envelope and an early exit the turn did not complete; a
    // clean exit with no envelope (e.g. gemini-cli) stays undetermined (`None`).
    let raw_process_exit_code = exit_code;
    let terminal_reason = if idle_timed_out || workspace_boundary_timed_out {
        Some("idle_timeout".to_string())
    } else {
        parse_legacy_terminal_reason(&output)
    };
    let model_completed = if idle_timed_out || workspace_boundary_timed_out {
        Some(false)
    } else if terminal_reason.is_some() {
        crate::model_completed_from_terminal_reason(terminal_reason.as_deref())
    } else if child_exited_early {
        Some(false)
    } else {
        None
    };

    let output = sanitize_opaque_object_payloads(&output);
    let mut stderr_output = sanitize_opaque_object_payloads(&stderr_output);
    let actionable_detail = resolve_actionable_failure_detail(&summary, exit_code);
    stderr_output = append_actionable_detail_for_opaque_payload(&stderr_output, &actionable_detail);

    let output_spool_plan = spool_file.take().map(|rotator| rotator.finalize());
    let stderr_spool_plan = stderr_spool_file.take().map(|rotator| rotator.finalize());
    if let Some(plan_result) = output_spool_plan {
        match plan_result {
            Ok(plan) => {
                if let Err(e) = sanitize_spool_plan(plan, None) {
                    warn!(error = %e, "Failed to sanitize output spool tail");
                }
            }
            Err(e) => {
                warn!(error = %e, "Failed to finalize output spool file");
            }
        }
    }
    if let Some(plan_result) = stderr_spool_plan {
        match plan_result {
            Ok(plan) => {
                if let Err(e) = sanitize_spool_plan(plan, Some(&actionable_detail)) {
                    warn!(error = %e, "Failed to sanitize stderr spool tail");
                }
            }
            Err(e) => {
                warn!(error = %e, "Failed to finalize stderr spool file");
            }
        }
    }

    Ok(ExecutionResult {
        output,
        stderr_output,
        summary,
        exit_code,
        raw_process_exit_code: Some(raw_process_exit_code),
        model_completed,
        terminal_reason,
        peak_memory_mb: None,
        ..Default::default()
    })
}
