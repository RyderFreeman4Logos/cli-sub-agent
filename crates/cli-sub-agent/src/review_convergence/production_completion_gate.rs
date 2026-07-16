//! Direct-argv final-gate driver used by production completion ports.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};

use super::gate_evidence::{
    FinalGateDriver, GateInvocation, GateProcessOutcome, GateProcessTermination,
};

/// Synchronous adapter around the established Tokio process-group cleanup primitives.
pub(super) struct BlockingDirectFinalGateDriver;

impl FinalGateDriver for BlockingDirectFinalGateDriver {
    fn run(&mut self, invocation: &GateInvocation) -> Result<GateProcessOutcome> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::try_current()
                .context("final-gate driver requires the CSA Tokio runtime")?
                .block_on(run_direct_final_gate(invocation))
        })
    }
}

async fn run_direct_final_gate(invocation: &GateInvocation) -> Result<GateProcessOutcome> {
    use tokio::process::Command;

    if !invocation.independent_process_group() {
        bail!("final-gate invocation does not request an independent process group");
    }
    let mut command = Command::new(invocation.command().program());
    command
        .args(invocation.command().argv())
        .current_dir(invocation.cwd())
        .env_clear()
        .envs(invocation.env().iter().cloned())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn().context("spawn final-gate direct argv")?;
    let pid = child.id();
    let stdout_capture = Arc::new(Mutex::new(
        crate::run_cmd_post_exec_gate_capture::BoundedTailCapture::default(),
    ));
    let stderr_capture = Arc::new(Mutex::new(
        crate::run_cmd_post_exec_gate_capture::BoundedTailCapture::default(),
    ));
    let stdout_pump = child.stdout.take().map(|stdout| {
        crate::run_cmd_post_exec_gate_capture::tee_gate_stream(
            stdout,
            tokio::io::sink(),
            Arc::clone(&stdout_capture),
        )
    });
    let stderr_pump = child.stderr.take().map(|stderr| {
        crate::run_cmd_post_exec_gate_capture::tee_gate_stream(
            stderr,
            tokio::io::sink(),
            Arc::clone(&stderr_capture),
        )
    });
    let termination = match tokio::time::timeout(invocation.command().timeout(), child.wait()).await
    {
        Ok(Ok(status)) => {
            let drain = crate::run_cmd_post_exec_gate_capture::drain_pumps_and_reap(
                stdout_pump,
                stderr_pump,
                pid,
            )
            .await;
            if drain.reaped_pipe_holding_descendants()
                || crate::run_cmd_post_exec_gate_capture::gate_process_group_has_live_members(pid)
            {
                GateProcessTermination::ChildSurvivor
            } else {
                GateProcessTermination::Exited(status.code().unwrap_or(1))
            }
        }
        Ok(Err(error)) => return Err(error).context("wait for final-gate process"),
        Err(_) => {
            crate::run_cmd_post_exec_gate_capture::kill_gate_process_group(pid).await;
            let _ = tokio::time::timeout(Duration::from_secs(3), child.wait()).await;
            let _ = crate::run_cmd_post_exec_gate_capture::drain_pumps_and_reap(
                stdout_pump,
                stderr_pump,
                pid,
            )
            .await;
            GateProcessTermination::TimedOut
        }
    };
    let stdout = stdout_capture
        .lock()
        .map_err(|_| anyhow!("final-gate stdout capture lock is poisoned"))?
        .render()
        .into_bytes();
    let stderr = stderr_capture
        .lock()
        .map_err(|_| anyhow!("final-gate stderr capture lock is poisoned"))?
        .render()
        .into_bytes();
    Ok(GateProcessOutcome::new(termination, stdout, stderr))
}
