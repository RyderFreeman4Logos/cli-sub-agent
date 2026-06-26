use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::Result;

pub(crate) fn wait_for_daemon_pre_spawn_memory_admission(
    project_root: &Path,
    session_id: &str,
    session_dir: &Path,
) -> Result<()> {
    let ready_path = crate::resource_admission::spawn_memory_admission_ready_path(session_dir);
    let timeout = daemon_pre_spawn_memory_admission_timeout(project_root);
    let start = Instant::now();

    loop {
        if ready_path.is_file() {
            return Ok(());
        }
        if let Some(message) =
            daemon_pre_spawn_failure_message(project_root, session_id, session_dir)?
        {
            anyhow::bail!("{message}");
        }
        if start.elapsed() >= timeout {
            eprintln!(
                "CSA: warning — timed out after {secs}s waiting for daemon pre-spawn host-memory admission; emitting a waitable session handle.",
                secs = timeout.as_secs()
            );
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn daemon_pre_spawn_memory_admission_timeout(project_root: &Path) -> Duration {
    let config = csa_config::ProjectConfig::load(project_root).ok().flatten();
    let slot_wait =
        crate::run_cmd_tool_selection::resolve_slot_wait_timeout_seconds(config.as_ref());
    Duration::from_secs(slot_wait.saturating_add(5))
}

fn daemon_pre_spawn_failure_message(
    project_root: &Path,
    session_id: &str,
    session_dir: &Path,
) -> Result<Option<String>> {
    if let Some(result) = csa_session::load_result(project_root, session_id)? {
        let summary = result.summary.trim();
        let detail = if summary.is_empty() {
            format!(
                "daemon exited before pre-spawn host-memory admission completed (exit_code={})",
                result.exit_code
            )
        } else {
            summary.to_string()
        };
        let prefix = if detail.contains("host memory admission denied") {
            "CSA: host memory admission was denied before daemon session handoff"
        } else {
            "CSA: daemon failed before pre-spawn host-memory admission completed"
        };
        return Ok(Some(format!(
            "{prefix}; no session-start marker was emitted, and no `csa session wait` is needed.\nSummary: {detail}"
        )));
    }

    if let Some(packet) = crate::session_cmds_daemon::load_daemon_completion_packet(session_dir)?
        && packet.exit_code != 0
        && !csa_process::ToolLiveness::daemon_pid_is_alive(session_dir)
    {
        return Ok(Some(format!(
            "CSA: daemon exited before pre-spawn host-memory admission completed \
             (exit_code={}, status={}); no session-start marker was emitted, and no \
             `csa session wait` is needed. Inspect daemon stderr at {}",
            packet.exit_code,
            packet.status,
            session_dir.join("stderr.log").display()
        )));
    }

    Ok(None)
}
