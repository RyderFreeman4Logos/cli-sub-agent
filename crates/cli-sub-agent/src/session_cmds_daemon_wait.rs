use super::*;

/// Exit code reserved for `csa session wait` memory warning early-exit.
pub(crate) const SESSION_WAIT_MEMORY_WARN_EXIT_CODE: i32 = 33;
const SESSION_WAIT_MEMORY_SAMPLE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(15);

#[derive(Debug, Clone, Copy)]
pub(crate) struct WaitLoopTiming {
    pub(crate) poll_interval: std::time::Duration,
    pub(crate) memory_sample_interval: std::time::Duration,
}

impl Default for WaitLoopTiming {
    fn default() -> Self {
        Self {
            poll_interval: std::time::Duration::from_secs(1),
            memory_sample_interval: SESSION_WAIT_MEMORY_SAMPLE_INTERVAL,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct WaitBehavior {
    pub(crate) wait_timeout_secs: u64,
    pub(crate) memory_warn_mb: Option<u64>,
    pub(crate) timing: WaitLoopTiming,
}

impl WaitBehavior {
    fn new(wait_timeout_secs: u64, memory_warn_mb: Option<u64>) -> Self {
        Self {
            wait_timeout_secs,
            memory_warn_mb,
            timing: WaitLoopTiming::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WaitReconciliationOutcome {
    pub(crate) result_became_available: bool,
    pub(crate) synthetic: bool,
}

/// Wait for a daemon session to reach a terminal result and daemon exit.
/// Exits 0 on completion, 124 on timeout, and 1 if the daemon dies without a result.
#[cfg(test)]
pub(crate) fn handle_session_wait(
    session: String,
    cd: Option<String>,
    wait_timeout_secs: u64,
) -> Result<i32> {
    handle_session_wait_with_memory_warn(session, cd, wait_timeout_secs, None)
}

pub(crate) fn handle_session_wait_with_memory_warn(
    session: String,
    cd: Option<String>,
    wait_timeout_secs: u64,
    memory_warn_mb: Option<u64>,
) -> Result<i32> {
    handle_session_wait_with_hooks(
        session,
        cd,
        WaitBehavior::new(wait_timeout_secs, memory_warn_mb),
        |project_root, session_id, trigger| {
            let reconciled = crate::session_cmds::ensure_terminal_result_for_dead_active_session(
                project_root,
                session_id,
                trigger,
            )?;
            Ok(WaitReconciliationOutcome {
                result_became_available: reconciled.result_became_available(),
                synthetic: reconciled.synthesized_failure(),
            })
        },
        emit_wait_completion_signal,
    )
}

pub(crate) fn handle_session_wait_with_hooks<R, E>(
    session: String,
    cd: Option<String>,
    wait_behavior: WaitBehavior,
    mut reconcile_dead_active_session: R,
    mut emit_completion_signal: E,
) -> Result<i32>
where
    R: for<'a, 'b, 'c> FnMut(&'a Path, &'b str, &'c str) -> Result<WaitReconciliationOutcome>,
    E: for<'a, 'b> FnMut(&'a str, &'b str, i32, bool, bool),
{
    let mut cached_memory_sampler: Option<csa_session::SessionTreeMemorySampler> = None;
    handle_session_wait_with_hooks_and_sampler(
        session,
        cd,
        wait_behavior,
        &mut reconcile_dead_active_session,
        &mut emit_completion_signal,
        |project_root, session_id| {
            if cached_memory_sampler.is_none() {
                cached_memory_sampler = Some(csa_session::SessionTreeMemorySampler::new(
                    project_root,
                    session_id,
                )?);
            }
            cached_memory_sampler
                .as_ref()
                .expect("cached sampler initialized above")
                .sample_rss_mb()
        },
        emit_wait_memory_warn_marker,
    )
}

pub(crate) fn handle_session_wait_with_hooks_and_sampler<R, E, S, M>(
    session: String,
    cd: Option<String>,
    wait_behavior: WaitBehavior,
    mut reconcile_dead_active_session: R,
    mut emit_completion_signal: E,
    mut sample_session_tree_rss_mb: S,
    mut emit_memory_warn_marker: M,
) -> Result<i32>
where
    R: FnMut(&Path, &str, &str) -> Result<WaitReconciliationOutcome>,
    E: FnMut(&str, &str, i32, bool, bool),
    S: FnMut(&Path, &str) -> std::io::Result<u64>,
    M: FnMut(&str, u64, u64),
{
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let resolved = resolve_session_prefix_with_global_fallback(&project_root, &session)?;
    // For cross-project sessions, derive session_dir from the resolved sessions_dir
    let session_dir = resolved.sessions_dir.join(&resolved.session_id);

    // Use the foreign project root for cross-project sessions, local otherwise.
    let effective_root = resolved
        .foreign_project_root
        .as_deref()
        .unwrap_or(&project_root);
    let is_cross_project = resolved.foreign_project_root.is_some();

    let start = std::time::Instant::now();
    let memory_warn_mb = wait_behavior.memory_warn_mb.filter(|limit| *limit > 0);
    let mut next_memory_sample_at =
        memory_warn_mb.map(|_| start + wait_behavior.timing.memory_sample_interval);

    loop {
        if let Some(completion) = load_daemon_completion_packet(&session_dir)?
            && !session_has_terminal_process(&session_dir)
        {
            let refreshed_result = refresh_result_for_wait(
                effective_root,
                &resolved.session_id,
                &session_dir,
                is_cross_project,
            );
            if let Err(err) = &refreshed_result {
                tracing::debug!(
                    session_id = %resolved.session_id,
                    error = %err,
                    "Failed to refresh result after daemon completion packet"
                );
            }
            let refreshed_result = refreshed_result.ok().flatten();
            let mut synthetic = false;
            let refreshed_result_available = refreshed_result.is_some();
            let mut loaded_result = refreshed_result.filter(|result| {
                match (
                    fs::metadata(session_dir.join(csa_session::result::RESULT_FILE_NAME))
                        .ok()
                        .and_then(|metadata| metadata.modified().ok()),
                    fs::metadata(daemon_completion_path(&session_dir))
                        .ok()
                        .and_then(|metadata| metadata.modified().ok()),
                ) {
                    (Some(result_modified), Some(completion_modified))
                        if result_modified > completion_modified =>
                    {
                        true
                    }
                    (Some(result_modified), Some(completion_modified))
                        if result_modified == completion_modified =>
                    {
                        completion.exit_code == 0 && result.exit_code != 0
                    }
                    _ => false,
                }
            });
            if refreshed_result_available {
                crate::session_cmds::retire_if_dead_with_result(
                    effective_root,
                    &resolved.session_id,
                    "session wait",
                )?;
            } else {
                let reconciled = reconcile_dead_active_session(
                    effective_root,
                    &resolved.session_id,
                    "session wait",
                )?;
                synthetic = reconciled.synthetic;
                if reconciled.result_became_available {
                    loaded_result = load_completed_daemon_result_adaptive(
                        effective_root,
                        &resolved.session_id,
                        &session_dir,
                        is_cross_project,
                    )?;
                }
            }
            let streamed_output = stream_wait_output(&session_dir)?;
            emit_wait_next_step_if_needed(&session_dir)?;
            #[rustfmt::skip]
            let (completion_status, exit_code) = resolve_wait_completion_status_and_exit(completion.status.as_str(), completion.exit_code, synthetic, loaded_result.as_ref());
            emit_completion_signal(
                &resolved.session_id,
                completion_status.as_ref(),
                exit_code,
                synthetic,
                !streamed_output,
            );
            return Ok(exit_code);
        }

        if let Some(result) = load_completed_daemon_result_adaptive(
            effective_root,
            &resolved.session_id,
            &session_dir,
            is_cross_project,
        )? {
            let streamed_output = stream_wait_output(&session_dir)?;
            emit_wait_next_step_if_needed(&session_dir)?;
            #[rustfmt::skip]
            let (completion_status, exit_code) = resolve_wait_completion_status_and_exit(result.status.as_str(), result.exit_code, false, Some(&result));
            emit_completion_signal(
                &resolved.session_id,
                completion_status.as_ref(),
                exit_code,
                false,
                !streamed_output,
            );
            return Ok(exit_code);
        }

        // Synthesize terminal result for dead Active sessions.
        let reconciled =
            reconcile_dead_active_session(effective_root, &resolved.session_id, "session wait")?;
        if reconciled.result_became_available
            && let Some(result) = load_completed_daemon_result_adaptive(
                effective_root,
                &resolved.session_id,
                &session_dir,
                is_cross_project,
            )?
        {
            let streamed_output = stream_wait_output(&session_dir)?;
            emit_wait_next_step_if_needed(&session_dir)?;
            #[rustfmt::skip]
            let (completion_status, exit_code) = resolve_wait_completion_status_and_exit(result.status.as_str(), result.exit_code, reconciled.synthetic, Some(&result));
            emit_completion_signal(
                &resolved.session_id,
                completion_status.as_ref(),
                exit_code,
                reconciled.synthetic,
                !streamed_output,
            );
            if reconciled.synthetic && !streamed_output {
                eprintln!(
                    "Session {} reached a synthesized terminal result because no live daemon process remained.",
                    resolved.session_id,
                );
            }
            return Ok(exit_code);
        }

        if !session_has_terminal_process(&session_dir) {
            if let Some(result) = load_completed_daemon_result_adaptive(
                effective_root,
                &resolved.session_id,
                &session_dir,
                is_cross_project,
            )? {
                let streamed_output = stream_wait_output(&session_dir)?;
                emit_wait_next_step_if_needed(&session_dir)?;
                #[rustfmt::skip]
                let (completion_status, exit_code) = resolve_wait_completion_status_and_exit(result.status.as_str(), result.exit_code, false, Some(&result));
                emit_completion_signal(
                    &resolved.session_id,
                    completion_status.as_ref(),
                    exit_code,
                    false,
                    !streamed_output,
                );
                return Ok(exit_code);
            }
            eprintln!(
                "Session {} has no live daemon process and no terminal result packet.",
                resolved.session_id,
            );
            eprintln!(
                "Run `csa session result --session {}` for diagnostics.",
                resolved.session_id
            );
            return Ok(1);
        }

        if let (Some(limit_mb), Some(sample_at)) = (memory_warn_mb, next_memory_sample_at)
            && std::time::Instant::now() >= sample_at
        {
            match sample_session_tree_rss_mb(effective_root, &resolved.session_id) {
                Ok(actual_rss_mb) => {
                    if actual_rss_mb > limit_mb {
                        emit_memory_warn_marker(&resolved.session_id, actual_rss_mb, limit_mb);
                        return Ok(SESSION_WAIT_MEMORY_WARN_EXIT_CODE);
                    }
                    next_memory_sample_at = Some(
                        std::time::Instant::now() + wait_behavior.timing.memory_sample_interval,
                    );
                }
                Err(err) => {
                    tracing::debug!(
                        session_id = %resolved.session_id,
                        error = %err,
                        "Session wait memory sampler failed; will retry"
                    );
                    next_memory_sample_at = (err.kind() != std::io::ErrorKind::Unsupported)
                        .then_some(
                            std::time::Instant::now() + wait_behavior.timing.memory_sample_interval,
                        );
                }
            }
        }

        let elapsed = start.elapsed().as_secs();
        if elapsed >= wait_behavior.wait_timeout_secs {
            eprintln!(
                "Timeout: session {} did not complete within {}s",
                resolved.session_id, wait_behavior.wait_timeout_secs,
            );
            // Emit structured retry hint for orchestrators / agents.
            let cd_arg = cd
                .as_ref()
                .map(|path| format!(" --cd '{}'", path))
                .unwrap_or_default();
            eprintln!(
                "<!-- CSA:SESSION_WAIT_TIMEOUT session={} elapsed={}s cmd=\"csa session wait --session {}{}\" -->",
                resolved.session_id, elapsed, resolved.session_id, cd_arg,
            );
            eprintln!(
                "Hint: Call `csa session wait` again individually (not in a tight loop script). \
                 The {}s timeout is designed to let the calling agent generate tokens between waits, \
                 keeping its KV cache warm.",
                wait_behavior.wait_timeout_secs,
            );
            eprintln!(
                "<!-- CSA:CALLER_HINT action=\"retry_wait\" \
                 rule=\"Process this output NOW and generate tokens to warm your KV cache, \
                 then call 'csa session wait --session {sid}{cd}' again in a NEW Bash call. \
                 NEVER batch multiple session waits in one Bash call.\" -->",
                sid = resolved.session_id,
                cd = cd
                    .as_ref()
                    .map(|p| format!(" --cd '{p}'"))
                    .unwrap_or_default(),
            );
            return Ok(124);
        }

        std::thread::sleep(wait_behavior.timing.poll_interval);
    }
}

fn stream_wait_output(session_dir: &std::path::Path) -> Result<bool> {
    let stdout_log = session_dir.join("stdout.log");
    if !stdout_log.is_file() {
        return Ok(false);
    }

    let mut file = std::fs::File::open(&stdout_log)?;
    let mut stdout = std::io::stdout().lock();
    let bytes = std::io::copy(&mut file, &mut stdout)?;
    stdout.flush()?;
    Ok(bytes > 0)
}

pub(crate) fn synthesized_wait_next_step(session_dir: &Path) -> Result<Option<String>> {
    let stdout_path = session_dir.join("stdout.log");
    if let Ok(stdout) = fs::read_to_string(&stdout_path)
        && csa_hooks::parse_next_step_directive(&stdout).is_some()
    {
        return Ok(None);
    }

    let unpushed_commits_path = session_dir.join("output").join("unpushed_commits.json");
    if unpushed_commits_path.is_file() {
        match fs::read_to_string(&unpushed_commits_path) {
            Ok(contents) => {
                match serde_json::from_str::<UnpushedCommitsRecoveryPacket>(&contents) {
                    Ok(recovery) if !recovery.recovery_command.trim().is_empty() => {
                        return Ok(Some(csa_hooks::format_next_step_directive(
                            &recovery.recovery_command,
                            true,
                        )));
                    }
                    Ok(_) => {}
                    Err(err) => {
                        tracing::warn!(
                            sidecar_path = %unpushed_commits_path.display(),
                            error = %err,
                            "Ignoring malformed unpushed commit recovery sidecar while synthesizing wait next-step"
                        );
                    }
                }
            }
            Err(err) => {
                tracing::debug!(
                    sidecar_path = %unpushed_commits_path.display(),
                    error = %err,
                    "Ignoring unreadable unpushed commit recovery sidecar while synthesizing wait next-step"
                );
            }
        }
    }

    let review_meta_path = session_dir.join("review_meta.json");
    if !review_meta_path.is_file() {
        return Ok(None);
    }

    let review_meta: ReviewSessionMeta =
        serde_json::from_str(&fs::read_to_string(review_meta_path)?)?;
    if review_meta.decision != "pass" {
        return Ok(None);
    }
    if !(review_meta.scope.starts_with("base:") || review_meta.scope.starts_with("range:")) {
        return Ok(None);
    }

    Ok(Some(csa_hooks::format_next_step_directive(
        POST_REVIEW_PR_BOT_CMD,
        true,
    )))
}

fn emit_wait_next_step_if_needed(session_dir: &Path) -> Result<()> {
    if let Some(directive) = synthesized_wait_next_step(session_dir)? {
        println!("{directive}");
    }
    Ok(())
}

fn resolve_wait_completion_status_and_exit<'a>(
    fallback_status: &'a str,
    fallback_exit_code: i32,
    synthetic: bool,
    real_result: Option<&'a csa_session::SessionResult>,
) -> (Cow<'a, str>, i32) {
    if synthetic {
        return (Cow::Borrowed("failure"), 1);
    }
    real_result.map_or_else(
        || (Cow::Borrowed(fallback_status), fallback_exit_code),
        |result| (Cow::Borrowed(result.status.as_str()), result.exit_code),
    )
}

fn load_completed_daemon_result(
    project_root: &std::path::Path,
    session_id: &str,
    session_dir: &std::path::Path,
) -> Result<Option<csa_session::SessionResult>> {
    let daemon_alive_at_refresh_start = session_has_terminal_process(session_dir);
    let result =
        match crate::session_observability::refresh_and_repair_result(project_root, session_id) {
            Ok(Some(result)) => result,
            Ok(None) => return Ok(None),
            Err(err) if daemon_alive_at_refresh_start => {
                tracing::debug!(
                    session_id,
                    error = %err,
                    "Ignoring transient result refresh failure while daemon is still alive"
                );
                return Ok(None);
            }
            Err(err) => return Err(err),
        };

    if session_has_terminal_process(session_dir) {
        return Ok(None);
    }
    Ok(Some(result))
}

/// Refresh result via session_dir for cross-project sessions or via project_root otherwise.
fn refresh_result_for_wait(
    project_root: &std::path::Path,
    session_id: &str,
    session_dir: &std::path::Path,
    is_cross_project: bool,
) -> Result<Option<csa_session::SessionResult>> {
    if is_cross_project {
        crate::session_observability::refresh_and_repair_result_from_dir(session_dir)
    } else {
        crate::session_observability::refresh_and_repair_result(project_root, session_id)
    }
}

/// Load completed daemon result, adapting for cross-project sessions.
fn load_completed_daemon_result_adaptive(
    project_root: &std::path::Path,
    session_id: &str,
    session_dir: &std::path::Path,
    is_cross_project: bool,
) -> Result<Option<csa_session::SessionResult>> {
    if is_cross_project {
        let daemon_alive_at_refresh_start = session_has_terminal_process(session_dir);
        let result = match crate::session_observability::refresh_and_repair_result_from_dir(
            session_dir,
        ) {
            Ok(Some(result)) => result,
            Ok(None) => return Ok(None),
            Err(err) if daemon_alive_at_refresh_start => {
                tracing::debug!(
                    session_id,
                    error = %err,
                    "Ignoring transient result refresh failure (cross-project) while daemon is still alive"
                );
                return Ok(None);
            }
            Err(err) => return Err(err),
        };
        if session_has_terminal_process(session_dir) {
            return Ok(None);
        }
        Ok(Some(result))
    } else {
        load_completed_daemon_result(project_root, session_id, session_dir)
    }
}

fn emit_wait_completion_signal(
    session_id: &str,
    status: &str,
    exit_code: i32,
    synthetic: bool,
    _mirror_to_stdout: bool,
) {
    let signal = format!(
        "<!-- CSA:SESSION_WAIT_COMPLETED session={} status={} exit={} synthetic={} -->",
        session_id, status, exit_code, synthetic
    );
    println!("{signal}");
    eprintln!("{signal}");
    eprintln!(
        "<!-- CSA:CALLER_HINT action=\"next_session\" \
         rule=\"If you have more sessions to wait for, call each 'csa session wait' \
         in a SEPARATE Bash call. NEVER batch. \
         Generate tokens between waits to keep your KV cache warm.\" -->"
    );
}

fn emit_wait_memory_warn_marker(session_id: &str, rss_mb: u64, limit_mb: u64) {
    println!(
        "<!-- CSA:MEMORY_WARN session={} rss_mb={} limit_mb={} -->",
        session_id, rss_mb, limit_mb
    );
}
