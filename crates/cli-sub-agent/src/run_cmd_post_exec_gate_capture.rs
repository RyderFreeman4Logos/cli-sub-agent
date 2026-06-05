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
//!    write-end and holds it open past the grace, the pump tasks are aborted and
//!    the gate child's process group is killed, so the gate `timeout_seconds`
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

/// Grace between the process-group `SIGTERM` and the escalating `SIGKILL`,
/// matching the project's two-phase subprocess-termination pattern (Rust 015).
const GATE_GROUP_TERM_GRACE: Duration = Duration::from_millis(100);

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

/// `SIGTERM`-then-`SIGKILL` the gate child's process GROUP (negative PID),
/// reaping any descendant that inherited the gate's stdout/stderr pipe.
///
/// ## PGID-reuse safety
/// Both call sites guarantee the target PGID cannot have been recycled at the
/// instant the signal is sent, so `-pid` reaches only this gate's descendants
/// and never a PID-reuse victim:
///  * the **timeout** path calls this BEFORE reaping the group leader, so the
///    un-reaped leader anchors the PGID;
///  * the **drain-grace-expiry** path calls this only when a pipe write-end is
///    still open — i.e. a live descendant remains in the group. On Linux the
///    kernel keeps a PGID's numeric value reserved for as long as any process
///    is a member of that group (the `struct pid` is pinned via `PIDTYPE_PGID`),
///    so the leader having already been reaped does not free it for reuse.
pub(crate) async fn kill_gate_process_group(child_pid: Option<u32>) {
    #[cfg(unix)]
    {
        if let Some(pid) = child_pid {
            // SAFETY: kill(2) is async-signal-safe; a negative PID targets the
            // process group created by `process_group(0)` at spawn. The PGID is
            // not reused at this instant (see the fn-level PGID-reuse-safety
            // note), so the signal reaches only this gate's own descendants.
            unsafe {
                libc::kill(-(pid as i32), libc::SIGTERM);
            }
            tokio::time::sleep(GATE_GROUP_TERM_GRACE).await;
            // SAFETY: same PGID-anchoring invariant as the SIGTERM above.
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = child_pid;
    }
}

/// Drain the tee pump tasks under [`GATE_PUMP_DRAIN_GRACE`]. Returns once both
/// pumps reach EOF (the child closed its pipes) or the grace expires. On expiry
/// — a backgrounded descendant inherited a pipe write-end and is holding it
/// open — the gate child's process group is killed (reaping the descendant and
/// closing the pipe) and the pump tasks are aborted, so the runner returns
/// instead of blocking forever and the gate `timeout_seconds` bounds the TOTAL
/// operation rather than only `child.wait()`.
pub(crate) async fn drain_pumps_and_reap(
    stdout_pump: Option<JoinHandle<()>>,
    stderr_pump: Option<JoinHandle<()>>,
    child_pid: Option<u32>,
) {
    // Capture abort handles before moving the join handles into the drain
    // future: dropping a `JoinHandle` only detaches the task, so an explicit
    // abort is required to stop a pump still blocked on a held-open pipe.
    let aborts: Vec<AbortHandle> = [&stdout_pump, &stderr_pump]
        .into_iter()
        .flatten()
        .map(JoinHandle::abort_handle)
        .collect();

    let join = async move {
        if let Some(pump) = stdout_pump {
            let _ = pump.await;
        }
        if let Some(pump) = stderr_pump {
            let _ = pump.await;
        }
    };

    if tokio::time::timeout(GATE_PUMP_DRAIN_GRACE, join)
        .await
        .is_err()
    {
        kill_gate_process_group(child_pid).await;
        for abort in aborts {
            abort.abort();
        }
    }
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
