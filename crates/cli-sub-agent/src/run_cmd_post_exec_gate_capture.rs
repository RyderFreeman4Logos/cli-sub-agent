//! Bounded, process-group-safe capture of post-exec gate output (#1726).
//!
//! The gate runner tees the child's stdout/stderr to the parent (so the raw
//! transcript `output/full.md` stays intact) while accumulating a combined copy
//! for the structured failure report. This module makes that accumulation
//! bounded in BOTH memory and time so a noisy or adversarial gate cannot wedge
//! or exhaust the writer-session orchestrator:
//!
//!  - [`BoundedTailCapture`] retains only the last [`GATE_CAPTURE_MAX_BYTES`]
//!    bytes (gate failures surface at the END of the transcript), marking
//!    truncation, so resident memory is capped regardless of output volume;
//!  - [`drain_pumps_and_reap`] bounds the post-exit pump drain by
//!    [`GATE_PUMP_DRAIN_GRACE`]; if a backgrounded grandchild inherited a pipe
//!    write-end and holds it open past the grace, the gate child's process group
//!    is terminated with an ownership-safe `SIGTERM`→(conditional)`SIGKILL`
//!    escalation and the pump tasks are aborted, so the gate `timeout_seconds`
//!    bounds the TOTAL operation (wait + drain), not just `child.wait()`.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::task::{AbortHandle, JoinHandle};

/// Maximum bytes retained in memory for the structured failure report's
/// captured output. The report only needs the TAIL of the gate transcript (the
/// failure summary / panic / failing-test list land at the end), so the capture
/// keeps the last N bytes and flags truncation when a noisy or flooding gate
/// exceeds it. 1 MiB is far larger than the report's own
/// `GATE_OUTPUT_TAIL_MAX_BYTES` (8 KiB) tail — so `output_tail` is never starved
/// — yet small enough to cap resident memory for any gate. The full transcript
/// is unaffected: every chunk is still tee'd through to the parent's streams.
pub(crate) const GATE_CAPTURE_MAX_BYTES: usize = 1024 * 1024;

/// Grace window for the stdout/stderr pump tasks to reach EOF AFTER the gate
/// child has exited (or been killed). A well-behaved gate closes its pipes on
/// exit and the pumps drain immediately; this bound exists so a backgrounded
/// grandchild that inherited the pipe write-end cannot make the drain block
/// forever (which would defeat the gate timeout). On expiry the pumps are
/// aborted and the process group is reaped.
pub(crate) const GATE_PUMP_DRAIN_GRACE: Duration = Duration::from_secs(2);

/// Grace between the process-group `SIGTERM` and the escalating `SIGKILL` on the
/// TIMEOUT path, matching the project's two-phase subprocess-termination pattern
/// (Rust 015). Sound to fire the second signal unconditionally there because the
/// un-reaped leader anchors the PGID across the window (see
/// [`kill_gate_process_group`]).
const GATE_GROUP_TERM_GRACE: Duration = Duration::from_millis(100);

/// Grace the drain-grace-expiry escalation waits AFTER its `SIGTERM` for the
/// surviving pipe-holder to close the pipe (pumps reach EOF) before deciding
/// whether a `SIGKILL` is still warranted. Unlike the timeout path, here the
/// leader has already been reaped, so the escalation sends `SIGKILL` ONLY if a
/// pump is STILL open when this elapses — i.e. a live group member still anchors
/// the PGID, keeping the group `SIGKILL` reuse-safe (#1726). Generous enough to
/// let a SIGTERM-responsive descendant exit and the pump observe EOF, yet
/// negligible against any real gate `timeout_seconds`.
const GATE_GROUP_ESCALATION_GRACE: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GatePumpDrainOutcome {
    Drained,
    ReapedPipeHoldingDescendants,
}

impl GatePumpDrainOutcome {
    pub(crate) fn reaped_pipe_holding_descendants(self) -> bool {
        matches!(self, Self::ReapedPipeHoldingDescendants)
    }
}

/// Accumulates the combined gate output while retaining only the last
/// [`GATE_CAPTURE_MAX_BYTES`] bytes, so a noisy or flooding gate cannot grow
/// resident memory without bound. Tracks the total bytes observed so the
/// rendered output can disclose truncation explicitly.
#[derive(Debug, Default)]
pub(crate) struct BoundedTailCapture {
    /// Retained tail bytes. Held between `GATE_CAPTURE_MAX_BYTES` and
    /// `2 * GATE_CAPTURE_MAX_BYTES`: trimming only when it reaches 2x means the
    /// front memmove runs at most once per `GATE_CAPTURE_MAX_BYTES` of input —
    /// amortized O(1) per byte rather than O(n) per chunk — so even a
    /// `yes`-style flood stays cheap (memory AND CPU bounded).
    tail: Vec<u8>,
    /// Total bytes observed across the whole stream (pre-trim), for the marker.
    total: u64,
}

impl BoundedTailCapture {
    /// Append a chunk, retaining only the most recent bytes within the cap.
    fn push(&mut self, bytes: &[u8]) {
        self.total = self.total.saturating_add(bytes.len() as u64);

        // A single chunk at/above the cap supersedes everything buffered so
        // far; only its own tail can survive.
        if bytes.len() >= GATE_CAPTURE_MAX_BYTES {
            self.tail.clear();
            self.tail
                .extend_from_slice(&bytes[bytes.len() - GATE_CAPTURE_MAX_BYTES..]);
            return;
        }

        self.tail.extend_from_slice(bytes);
        if self.tail.len() > 2 * GATE_CAPTURE_MAX_BYTES {
            let excess = self.tail.len() - GATE_CAPTURE_MAX_BYTES;
            self.tail.drain(..excess);
        }
    }

    /// Render the retained tail as lossy UTF-8. When the stream exceeded the
    /// cap, the result LEADS with an explicit truncation marker so both
    /// `gate-failure.log` and the report's bounded tail disclose the elision.
    pub(crate) fn render(&mut self) -> String {
        // The lazy 2x trim may leave more than the cap buffered; bring it down
        // to exactly the cap so the body is the true last N bytes.
        if self.tail.len() > GATE_CAPTURE_MAX_BYTES {
            let excess = self.tail.len() - GATE_CAPTURE_MAX_BYTES;
            self.tail.drain(..excess);
        }

        let body = String::from_utf8_lossy(&self.tail);
        if self.total > self.tail.len() as u64 {
            let total = self.total;
            let kept = self.tail.len();
            format!(
                "[csa: gate output truncated — {total} bytes captured, retained last {kept} bytes (cap {GATE_CAPTURE_MAX_BYTES} bytes)]\n{body}"
            )
        } else {
            body.into_owned()
        }
    }
}

/// Read `reader` to EOF, re-emitting each chunk to the parent's `sink` (so the
/// raw transcript stays intact) while appending it to the shared bounded
/// `captured` buffer for structured failure surfacing (#1726).
pub(crate) fn tee_gate_stream<R, W>(
    reader: R,
    sink: W,
    captured: Arc<Mutex<BoundedTailCapture>>,
) -> JoinHandle<()>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    tokio::spawn(async move {
        let mut reader = reader;
        let mut sink = sink;
        let mut chunk = [0u8; 8192];
        loop {
            match reader.read(&mut chunk).await {
                Ok(0) => break,
                Ok(n) => {
                    let bytes = &chunk[..n];
                    let _ = sink.write_all(bytes).await;
                    let _ = sink.flush().await;
                    if let Ok(mut guard) = captured.lock() {
                        guard.push(bytes);
                    }
                }
                Err(_) => break,
            }
        }
    })
}

/// Send `signal` to the gate child's process GROUP via a negative PID. The
/// caller MUST guarantee the target PGID is still anchored at this instant — by
/// an un-reaped (zombie) leader, or by a still-alive group member — so the signal
/// can only reach this gate's own descendants and never a PID-reuse victim.
#[cfg(unix)]
fn signal_gate_process_group(pid: i32, signal: i32) {
    // SAFETY: kill(2) is async-signal-safe. A negative PID targets the process
    // group created by `process_group(0)` at spawn. The caller guarantees the
    // PGID is still anchored at this instant (see each call site), so the signal
    // reaches only this gate's own descendants, never a recycled group.
    unsafe {
        libc::kill(-pid, signal);
    }
}

#[cfg(target_os = "linux")]
fn linux_proc_state_and_pgrp(pid: u32) -> Option<(char, i32)> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let close_paren = stat.rfind(')')?;
    let fields = stat.get(close_paren + 2..)?;
    let mut fields = fields.split_whitespace();
    let state = fields.next()?.chars().next()?;
    let _ppid = fields.next()?;
    let pgrp = fields.next()?.parse().ok()?;
    Some((state, pgrp))
}

/// Return true when a post-exec gate's process group still contains a live
/// descendant after the gate command itself exited and the output pumps drained.
///
/// This catches descendants that intentionally close stdout/stderr (so the pump
/// drain cannot observe them) while they keep mutating or validating the
/// worktree in the same gate process group (#2348). Zombies are excluded: they
/// no longer execute or mutate state, and the OS/init will reap them.
#[cfg(target_os = "linux")]
pub(crate) fn gate_process_group_has_live_members(child_pid: Option<u32>) -> bool {
    let Some(child_pid) = child_pid else {
        return false;
    };
    let target_pgrp = child_pid as i32;
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return false;
    };

    entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_string_lossy().parse::<u32>().ok())
        .any(|pid| {
            linux_proc_state_and_pgrp(pid)
                .is_some_and(|(state, pgrp)| pgrp == target_pgrp && !matches!(state, 'Z' | 'X'))
        })
}

#[cfg(all(unix, not(target_os = "linux")))]
pub(crate) fn gate_process_group_has_live_members(child_pid: Option<u32>) -> bool {
    let Some(pid) = child_pid else {
        return false;
    };
    let pid = pid as i32;
    // SAFETY: kill(2) with signal 0 performs existence/permission checking only.
    // It sends no signal, so a stale or reused PGID can at worst produce a
    // fail-closed residual diagnostic, never terminate unrelated processes.
    let result = unsafe { libc::kill(-pid, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
pub(crate) fn gate_process_group_has_live_members(_child_pid: Option<u32>) -> bool {
    false
}

/// `SIGTERM`-then-`SIGKILL` the gate child's process GROUP (negative PID),
/// reaping any descendant that inherited the gate's stdout/stderr pipe.
///
/// ## PGID-reuse safety — TIMEOUT PATH ONLY
/// The UNCONDITIONAL second (`SIGKILL`) signal here is sound ONLY while the group
/// leader has not yet been reaped: an un-reaped (zombie) leader pins the PGID via
/// `PIDTYPE_PGID`, so the value cannot be recycled across the `SIGTERM`→`SIGKILL`
/// window and `-pid` reaches only this gate's descendants. The timeout caller
/// guarantees this by calling here BEFORE `child.wait()`.
///
/// The drain-grace-expiry path must NOT use this helper: by then the leader has
/// already been reaped, so the surviving pipe-holder is the ONLY PGID anchor — if
/// it exits on the `SIGTERM`, the unconditional `SIGKILL` could race PGID reuse.
/// That path uses the ownership-safe escalation in [`drain_pumps_and_reap`]
/// instead (#1726).
pub(crate) async fn kill_gate_process_group(child_pid: Option<u32>) {
    #[cfg(unix)]
    {
        if let Some(pid) = child_pid {
            let pid = pid as i32;
            signal_gate_process_group(pid, libc::SIGTERM);
            tokio::time::sleep(GATE_GROUP_TERM_GRACE).await;
            signal_gate_process_group(pid, libc::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = child_pid;
    }
}

/// Await every still-pending pump task until they all reach EOF or `grace`
/// elapses, whichever comes first. Pump handles polled to completion in an
/// earlier call MUST be pruned by the caller (via [`JoinHandle::is_finished`])
/// between calls so this never re-polls a finished handle (which would panic).
async fn await_pumps_bounded(pumps: &mut [JoinHandle<()>], grace: Duration) {
    if pumps.is_empty() {
        return;
    }
    let _ = tokio::time::timeout(grace, async move {
        for pump in pumps.iter_mut() {
            // `&mut JoinHandle` is a `Future`; awaiting a not-yet-completed
            // handle is sound. The caller prunes completed handles between calls,
            // so a handle is never awaited after it already returned `Ready`.
            let _ = pump.await;
        }
    })
    .await;
}

/// Drain the tee pump tasks under [`GATE_PUMP_DRAIN_GRACE`]. Returns once both
/// pumps reach EOF (the child closed its pipes) or the grace expires. On
/// expiry — a backgrounded descendant inherited a pipe write-end and is holding
/// it open after the caller already reaped the group leader — the descendant is
/// reaped with an OWNERSHIP-SAFE escalation so the gate `timeout_seconds` bounds
/// the TOTAL operation rather than only `child.wait()`:
///
///  1. `SIGTERM` the gate's process group (politely ask the pipe-holder to go);
///  2. RE-WAIT the pumps for [`GATE_GROUP_ESCALATION_GRACE`]. If they reach EOF,
///     the pipe-holder died from the `SIGTERM` and released the pipe — the PGID
///     may now be unanchored, so NO `SIGKILL` is sent (a blind second signal
///     could race PGID reuse and hit an unrelated recycled group);
///  3. otherwise a pump is STILL open, proving a live group member still holds
///     the write-end and thus re-anchors the PGID, so `SIGKILL` to the group is
///     reuse-safe. Send it, then abort the leaked pump tasks (#1726).
///
/// Contrast the TIMEOUT path, which calls [`kill_gate_process_group`] BEFORE
/// reaping the leader: there the un-reaped leader anchors the PGID, so its
/// unconditional `SIGTERM`→`SIGKILL` cannot race reuse.
pub(crate) async fn drain_pumps_and_reap(
    stdout_pump: Option<JoinHandle<()>>,
    stderr_pump: Option<JoinHandle<()>>,
    child_pid: Option<u32>,
) -> GatePumpDrainOutcome {
    let mut pumps: Vec<JoinHandle<()>> = [stdout_pump, stderr_pump].into_iter().flatten().collect();
    // Capture abort handles up front: dropping a `JoinHandle` only detaches the
    // task, so an explicit abort is required to stop a pump still blocked on a
    // descendant-held pipe. Aborting an already-finished pump is a harmless no-op.
    let aborts: Vec<AbortHandle> = pumps.iter().map(JoinHandle::abort_handle).collect();

    // Phase 1: wait for both pumps to reach EOF under the drain grace. A
    // well-behaved gate closes its pipes on exit and this returns immediately.
    await_pumps_bounded(&mut pumps, GATE_PUMP_DRAIN_GRACE).await;
    pumps.retain(|pump| !pump.is_finished());
    if pumps.is_empty() {
        return GatePumpDrainOutcome::Drained;
    }

    // A backgrounded descendant is still holding a pipe write-end past the drain
    // grace, and the caller has ALREADY reaped the group leader — so escalate
    // ownership-safely (see the fn docs) rather than via the unconditional
    // `kill_gate_process_group`, whose second signal would race PGID reuse here.
    #[cfg(unix)]
    if let Some(pid) = child_pid {
        let pid = pid as i32;
        // Phase 2: politely terminate the group, then re-wait for the pumps to
        // reach EOF under a short escalation grace.
        signal_gate_process_group(pid, libc::SIGTERM);
        await_pumps_bounded(&mut pumps, GATE_GROUP_ESCALATION_GRACE).await;
        pumps.retain(|pump| !pump.is_finished());
        // Phase 3: escalate to `SIGKILL` ONLY while a pump is STILL open — a live
        // group member then anchors the PGID, so the group `SIGKILL` is reuse-safe.
        // If the pumps drained, the pipe-holder already exited on the `SIGTERM`
        // and a blind `SIGKILL` could hit a recycled PGID, so it is deliberately
        // skipped (there is nothing left to kill).
        if !pumps.is_empty() {
            signal_gate_process_group(pid, libc::SIGKILL);
            tokio::time::sleep(GATE_GROUP_TERM_GRACE).await;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = child_pid;
    }

    for abort in aborts {
        abort.abort();
    }
    GatePumpDrainOutcome::ReapedPipeHoldingDescendants
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_capture_keeps_full_output_below_cap_without_marker() {
        let mut capture = BoundedTailCapture::default();
        capture.push(b"line one\n");
        capture.push(b"line two\n");
        let rendered = capture.render();
        assert_eq!(rendered, "line one\nline two\n");
        assert!(!rendered.contains("truncated"));
    }

    #[test]
    fn bounded_capture_retains_tail_and_marks_truncation_over_cap() {
        let mut capture = BoundedTailCapture::default();
        // Push well past 2x the cap in one shot AND in pieces to exercise both
        // the single-oversized-chunk path and the incremental-trim path.
        let oversized = vec![b'a'; GATE_CAPTURE_MAX_BYTES * 2 + 4096];
        capture.push(&oversized);
        // A trailing marker that MUST survive as the retained tail.
        capture.push(b"TAIL-SENTINEL");

        let rendered = capture.render();
        // Body is bounded to ~the cap (plus the short marker line).
        assert!(
            rendered.len() <= GATE_CAPTURE_MAX_BYTES + 256,
            "rendered length {} must stay within the cap + marker slack",
            rendered.len()
        );
        // The TAIL is retained (not the head).
        assert!(rendered.ends_with("TAIL-SENTINEL"));
        // Truncation is disclosed.
        assert!(rendered.starts_with("[csa: gate output truncated"));
    }

    #[test]
    fn bounded_capture_single_chunk_at_cap_is_not_marked_truncated() {
        let mut capture = BoundedTailCapture::default();
        capture.push(&vec![b'z'; GATE_CAPTURE_MAX_BYTES]);
        let rendered = capture.render();
        assert_eq!(rendered.len(), GATE_CAPTURE_MAX_BYTES);
        assert!(!rendered.contains("truncated"));
    }
}
