use super::StreamMode;
use std::time::{Duration, Instant};

pub(super) const DEFAULT_HEARTBEAT_SECS: u64 = 20;
pub(super) const HEARTBEAT_INTERVAL_ENV: &str = "CSA_TOOL_HEARTBEAT_SECS";

/// Write a raw byte chunk to the spool file and flush.
///
/// Best-effort: errors are silently ignored because the spool is a crash-recovery
/// aid, not the primary output path.
pub(super) fn spool_chunk(spool: &mut Option<std::fs::File>, bytes: &[u8]) {
    if let Some(f) = spool {
        use std::io::Write;
        let _ = f.write_all(bytes);
        let _ = f.flush();
    }
}

pub(super) fn resolve_heartbeat_interval() -> Option<Duration> {
    let raw = std::env::var(HEARTBEAT_INTERVAL_ENV).ok();
    let secs = match raw {
        Some(value) => match value.trim().parse::<u64>() {
            Ok(0) => return None,
            Ok(parsed) => parsed,
            Err(_) => DEFAULT_HEARTBEAT_SECS,
        },
        None => DEFAULT_HEARTBEAT_SECS,
    };
    Some(Duration::from_secs(secs))
}

pub(super) fn maybe_emit_heartbeat(
    heartbeat_interval: Option<Duration>,
    execution_start: Instant,
    last_activity: Instant,
    last_heartbeat: &mut Instant,
    idle_timeout: Duration,
) {
    let Some(interval) = heartbeat_interval else {
        return;
    };

    let now = Instant::now();
    let idle_for = now.saturating_duration_since(last_activity);
    if idle_for < interval {
        return;
    }
    if now.saturating_duration_since(*last_heartbeat) < interval {
        return;
    }

    let elapsed = now.saturating_duration_since(execution_start);
    eprintln!(
        "[csa-heartbeat] tool still running: elapsed={}s idle={}s idle-timeout={}s",
        elapsed.as_secs(),
        idle_for.as_secs(),
        idle_timeout.as_secs()
    );
    *last_heartbeat = now;
}

/// Accumulate a chunk of bytes into a line buffer, flushing complete lines to output.
///
/// When a `\n` is found, the complete line (including `\n`) is appended to `output`
/// and optionally tee'd to stderr. Partial data remains in `line_buf` until more
/// data arrives or EOF triggers `flush_line_buf`.
pub(super) fn accumulate_and_flush_lines(
    chunk: &str,
    line_buf: &mut String,
    output: &mut String,
    stream_mode: StreamMode,
) {
    line_buf.push_str(chunk);
    while let Some(newline_pos) = line_buf.find('\n') {
        let line: String = line_buf.drain(..=newline_pos).collect();
        if stream_mode == StreamMode::TeeToStderr {
            eprint!("[stdout] {line}");
        }
        output.push_str(&line);
    }
}

/// Flush any remaining partial line from the stdout line buffer on EOF.
pub(super) fn flush_line_buf(line_buf: &mut String, output: &mut String, stream_mode: StreamMode) {
    if !line_buf.is_empty() {
        if stream_mode == StreamMode::TeeToStderr {
            eprint!("[stdout] {line_buf}");
        }
        output.push_str(line_buf);
        line_buf.clear();
    }
}

/// Accumulate stderr chunk, flushing complete lines in real-time.
pub(super) fn accumulate_and_flush_stderr(
    chunk: &str,
    line_buf: &mut String,
    stderr_output: &mut String,
    stream_mode: StreamMode,
) {
    line_buf.push_str(chunk);
    while let Some(newline_pos) = line_buf.find('\n') {
        let line: String = line_buf.drain(..=newline_pos).collect();
        if stream_mode == StreamMode::TeeToStderr {
            eprint!("{line}");
        }
        stderr_output.push_str(&line);
    }
}

/// Flush any remaining partial stderr line on EOF.
pub(super) fn flush_stderr_buf(
    line_buf: &mut String,
    stderr_output: &mut String,
    stream_mode: StreamMode,
) {
    if !line_buf.is_empty() {
        if stream_mode == StreamMode::TeeToStderr {
            eprint!("{line_buf}");
        }
        stderr_output.push_str(line_buf);
        line_buf.clear();
    }
}

/// Extract summary from output (last non-empty line, truncated to 200 chars).
pub(super) fn extract_summary(output: &str) -> String {
    truncate_line(last_non_empty_line(output), 200)
}

/// Build summary for failed executions (exit_code != 0).
///
/// Priority chain:
/// 1. stdout last non-empty line (if present — some tools write errors to stdout)
/// 2. stderr last non-empty line (fallback for tools that write errors to stderr)
/// 3. `"exit code {N}"` (final fallback when both streams are empty)
pub(super) fn failure_summary(stdout: &str, stderr: &str, exit_code: i32) -> String {
    let stdout_line = last_non_empty_line(stdout);
    if !stdout_line.is_empty() {
        return truncate_line(stdout_line, 200);
    }

    let stderr_line = last_non_empty_line(stderr);
    if !stderr_line.is_empty() {
        return truncate_line(stderr_line, 200);
    }

    format!("exit code {exit_code}")
}

/// Return the last non-empty line from the given text, or `""` if none.
pub(super) fn last_non_empty_line(text: &str) -> &str {
    text.lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
}

/// Truncate a line to `max_chars` characters, appending "..." if truncated.
pub(super) fn truncate_line(line: &str, max_chars: usize) -> String {
    if line.chars().nth(max_chars).is_none() {
        line.to_string()
    } else {
        let truncated: String = line.chars().take(max_chars - 3).collect();
        format!("{truncated}...")
    }
}
