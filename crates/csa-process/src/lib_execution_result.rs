use serde::Serialize;

/// Result of executing a command.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ExecutionResult {
    /// Combined stdout output.
    pub output: String,
    /// Captured stderr output.
    ///
    /// In `StreamMode::TeeToStderr`, stderr is also forwarded to parent stderr
    /// in real-time. In `StreamMode::BufferOnly`, stderr is captured only.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub stderr_output: String,
    /// Last non-empty line or truncated output (max 200 chars).
    pub summary: String,
    /// Effective exit code: the value the rest of the pipeline reads and the
    /// session's own contract with its caller. `1` if signal-killed; rewritten by
    /// CSA-own gates and by the outcome classifier (which may downgrade an
    /// incidental nonzero process exit to `0` — see [`crate::ExecutionResult`] docs
    /// on `raw_process_exit_code`).
    pub exit_code: i32,
    /// Peak memory usage in MB from cgroup `memory.peak`.
    /// `None` when cgroup monitoring is unavailable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peak_memory_mb: Option<u64>,
    /// Whether the model turn reached a normal terminal state (e.g. ACP `end_turn`
    /// / `max_tokens`; legacy `turn.completed` / `subtype=success`).
    ///
    /// `None` = the transport could not determine completion (legacy CLI without a
    /// parseable terminal envelope). `Some(false)` = the turn was cancelled, timed
    /// out, or never produced a terminal envelope. The session-outcome classifier
    /// uses this to distinguish an incidental nonzero process exit (model completed;
    /// a hook or in-turn command failed) from a genuine model failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_completed: Option<bool>,
    /// Raw transport terminal reason — ACP stop reason (`end_turn`, `cancelled`,
    /// `idle_timeout`, …) or legacy terminal token (`turn.completed`). Preserved as a
    /// diagnostic; `None` when the transport recorded no reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_reason: Option<String>,
    /// Raw tool-process exit code as reported by the transport, BEFORE any CSA-own
    /// gate or the outcome classifier rewrote `exit_code`. `None` when not separately
    /// captured (treat as equal to `exit_code`). Always preserved so a downgraded
    /// session can still be diagnosed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_process_exit_code: Option<i32>,
    /// CSA-own deterministic gate failure marker. `Some(reason)` means one of CSA's
    /// own post-run gates (edit guard / new-file guard / commit policy / no-op /
    /// worker-blocked / no-progress / tool exhaustion) fired and the nonzero
    /// `exit_code` is authoritative-fatal — NOT an incidental hook / in-turn-command
    /// exit. The outcome classifier treats this as a hard failure regardless of
    /// `model_completed`. Set via [`ExecutionResult::mark_gate_failure`] (forces
    /// `exit_code` to `1`) or [`ExecutionResult::note_gate_failure`] (preserves a
    /// pre-existing nonzero exit code).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub csa_gate_failure: Option<String>,
    /// Non-fatal warnings recorded during execution / outcome classification — e.g.
    /// the downgrade note when an incidental nonzero process exit is treated as
    /// success-with-warnings. Surfaced to the caller via `SessionResult`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

impl ExecutionResult {
    /// Mark a CSA-own deterministic gate as failed: force the effective `exit_code`
    /// to `1` and record an explicit failure `reason` so the outcome classifier treats
    /// this session as authoritative-fatal (never downgraded to success-with-warnings).
    ///
    /// A gate failure invalidates any prior incidental-exit downgrade, so accumulated
    /// `warnings` are cleared. The FIRST reason recorded wins as the canonical marker;
    /// subsequent gate firings remain fatal but do not overwrite it.
    pub fn mark_gate_failure(&mut self, reason: impl Into<String>) {
        self.exit_code = 1;
        self.warnings.clear();
        if self.csa_gate_failure.is_none() {
            self.csa_gate_failure = Some(reason.into());
        }
    }

    /// Record a CSA-own gate failure while preserving a more specific pre-existing
    /// failure exit code. Like [`Self::mark_gate_failure`], but escalates `exit_code`
    /// to `1` only when it is currently `0`: a commit policy firing on top of an
    /// already-failed run (e.g. the tool itself exited `2`) keeps the original code
    /// for diagnostics. Still records the explicit gate marker (first reason wins)
    /// and clears non-fatal warnings, so the outcome classifier treats the session
    /// as authoritative-fatal.
    pub fn note_gate_failure(&mut self, reason: impl Into<String>) {
        if self.exit_code == 0 {
            self.exit_code = 1;
        }
        self.warnings.clear();
        if self.csa_gate_failure.is_none() {
            self.csa_gate_failure = Some(reason.into());
        }
    }
}

/// Map a transport terminal reason to a model-turn completion signal for the
/// session-outcome classifier.
///
/// - `Some(true)` — the turn reached a normal/defined terminal state (ACP
///   `end_turn` / `max_tokens` / `max_turn_requests` / `refusal`; legacy
///   `turn.completed` / `completed` / `success`). An incidental nonzero process
///   exit on such a turn may be downgraded to success-with-warnings.
/// - `Some(false)` — the turn was cut short (`cancelled`, `idle_timeout`,
///   `initial_response_timeout`, `error`, `failed`).
/// - `None` — unrecognized or absent reason; the transport could not determine
///   completion, so the classifier must rely on other signals (e.g. final-output
///   presence).
pub fn model_completed_from_terminal_reason(reason: Option<&str>) -> Option<bool> {
    match reason {
        Some(
            "end_turn" | "max_tokens" | "max_turn_requests" | "refusal" | "turn.completed"
            | "completed" | "success",
        ) => Some(true),
        Some("cancelled" | "idle_timeout" | "initial_response_timeout" | "error" | "failed") => {
            Some(false)
        }
        _ => None,
    }
}

impl ExecutionResult {
    /// Consolidate consecutive retry/quota-exhaustion messages in stderr to
    /// reduce noise for orchestrators.  Replaces N consecutive retry lines with
    /// a single summary, preserving the last message for context.
    pub fn consolidate_stderr_retries(&mut self) {
        if self.stderr_output.is_empty() {
            return;
        }

        let lines: Vec<&str> = self.stderr_output.lines().collect();
        let mut consolidated = String::with_capacity(self.stderr_output.len());
        let mut retry_count = 0u32;
        let mut last_retry_line = "";

        for line in &lines {
            if is_retry_noise(line) {
                retry_count += 1;
                last_retry_line = line;
            } else {
                flush_retries(&mut consolidated, retry_count, last_retry_line);
                retry_count = 0;
                last_retry_line = "";
                consolidated.push_str(line);
                consolidated.push('\n');
            }
        }
        flush_retries(&mut consolidated, retry_count, last_retry_line);

        self.stderr_output = consolidated;
    }
}

fn flush_retries(buf: &mut String, count: u32, last_line: &str) {
    match count {
        0 => {}
        1 => {
            buf.push_str(last_line);
            buf.push('\n');
        }
        n => {
            buf.push_str(&format!("[{n} retry messages consolidated] {last_line}\n"));
        }
    }
}

fn is_retry_noise(line: &str) -> bool {
    let l = line.to_ascii_lowercase();
    // gemini-cli specific: "Attempt N failed: You have exhausted your capacity ... Retrying after Xms..."
    if l.contains("attempt") && l.contains("failed") && l.contains("retrying after") {
        return true;
    }
    // gemini-cli quota: "exhausted your capacity ... quota will reset"
    if l.contains("exhausted your capacity") && l.contains("quota will reset") {
        return true;
    }
    false
}
