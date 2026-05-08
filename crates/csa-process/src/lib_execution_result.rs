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
    /// Exit code (1 if signal-killed).
    pub exit_code: i32,
    /// Peak memory usage in MB from cgroup `memory.peak`.
    /// `None` when cgroup monitoring is unavailable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peak_memory_mb: Option<u64>,
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
