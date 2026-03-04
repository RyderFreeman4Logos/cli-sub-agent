use std::path::Path;

use super::StreamMode;
use std::time::{Duration, Instant};

pub(super) const DEFAULT_HEARTBEAT_SECS: u64 = 20;
pub(super) const HEARTBEAT_INTERVAL_ENV: &str = "CSA_TOOL_HEARTBEAT_SECS";
const WORKSPACE_BOUNDARY_PATTERN_A: &str = "path not in workspace";
const WORKSPACE_BOUNDARY_PATTERN_B: &str = "outside the allowed workspace directories";
const OPAQUE_OBJECT_PATTERN: &str = "[object object]";
const OPAQUE_OBJECT_REPLACEMENT: &str = "(opaque error payload)";

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
) -> usize {
    let mut boundary_hits = 0usize;
    line_buf.push_str(chunk);
    while let Some(newline_pos) = line_buf.find('\n') {
        let line: String = line_buf.drain(..=newline_pos).collect();
        if stream_mode == StreamMode::TeeToStderr {
            eprint!("[stdout] {line}");
        }
        if is_workspace_boundary_error_line(&line) {
            boundary_hits += 1;
        }
        output.push_str(&line);
    }
    boundary_hits
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
) -> usize {
    let mut boundary_hits = 0usize;
    line_buf.push_str(chunk);
    while let Some(newline_pos) = line_buf.find('\n') {
        let line: String = line_buf.drain(..=newline_pos).collect();
        if stream_mode == StreamMode::TeeToStderr {
            eprint!("{line}");
        }
        if is_workspace_boundary_error_line(&line) {
            boundary_hits += 1;
        }
        stderr_output.push_str(&line);
    }
    boundary_hits
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
    if let Some(stdout_line) = last_non_opaque_failure_line(stdout) {
        return truncate_line(&stdout_line, 200);
    }

    if let Some(stderr_line) = last_non_opaque_failure_line(stderr) {
        return truncate_line(&stderr_line, 200);
    }

    if let Some(opaque_line) = last_opaque_failure_line_with_context(stdout)
        .or_else(|| last_opaque_failure_line_with_context(stderr))
    {
        return truncate_line(&opaque_line, 200);
    }

    if contains_opaque_object_payload(stdout) || contains_opaque_object_payload(stderr) {
        return format!("opaque tool error payload; exit code {exit_code}");
    }

    format!("exit code {exit_code}")
}

pub(super) fn sanitize_opaque_object_payloads(text: &str) -> String {
    replace_opaque_object_payload(text, OPAQUE_OBJECT_REPLACEMENT)
}

/// Best-effort tail sanitization for spool files.
///
/// Rewrites only bytes appended during the current run (`start_offset..`) so
/// historical spool content from previous turns is preserved.
pub(super) fn sanitize_spool_tail(path: &Path, start_offset: u64) -> std::io::Result<()> {
    use std::fs::OpenOptions;
    use std::io::{Read, Seek, SeekFrom, Write};

    let mut file = OpenOptions::new().read(true).write(true).open(path)?;
    let file_len = file.metadata()?.len();
    if file_len <= start_offset {
        return Ok(());
    }

    file.seek(SeekFrom::Start(start_offset))?;
    let mut tail_bytes = Vec::new();
    file.read_to_end(&mut tail_bytes)?;
    let tail_text = String::from_utf8_lossy(&tail_bytes);
    if !contains_opaque_object_payload(&tail_text) {
        return Ok(());
    }

    let sanitized_tail = sanitize_opaque_object_payloads(&tail_text);
    if sanitized_tail == tail_text {
        return Ok(());
    }

    file.set_len(start_offset)?;
    file.seek(SeekFrom::Start(start_offset))?;
    file.write_all(sanitized_tail.as_bytes())?;
    file.flush()?;
    Ok(())
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

fn is_workspace_boundary_error_line(line: &str) -> bool {
    let normalized = line.to_ascii_lowercase();
    normalized.contains(WORKSPACE_BOUNDARY_PATTERN_A)
        || normalized.contains(WORKSPACE_BOUNDARY_PATTERN_B)
}

fn last_non_opaque_failure_line(text: &str) -> Option<String> {
    for line in text.lines().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if contains_opaque_object_payload(trimmed) || is_structural_failure_noise_line(trimmed) {
            continue;
        }

        return Some(trimmed.to_string());
    }

    None
}

fn last_opaque_failure_line_with_context(text: &str) -> Option<String> {
    for line in text.lines().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty() || !contains_opaque_object_payload(trimmed) {
            continue;
        }

        let normalized = replace_opaque_object_payload(trimmed, "")
            .trim()
            .trim_end_matches(':')
            .trim()
            .to_string();

        if !normalized.is_empty() {
            return Some(format!("{normalized} (opaque error payload)"));
        }
    }

    None
}

fn contains_opaque_object_payload(text: &str) -> bool {
    text.to_ascii_lowercase().contains(OPAQUE_OBJECT_PATTERN)
}

fn replace_opaque_object_payload(text: &str, replacement: &str) -> String {
    let lowered = text.to_ascii_lowercase();
    let mut cursor = 0usize;
    let mut output = String::with_capacity(text.len());

    while let Some(offset) = lowered[cursor..].find(OPAQUE_OBJECT_PATTERN) {
        let start = cursor + offset;
        let end = start + OPAQUE_OBJECT_PATTERN.len();
        output.push_str(&text[cursor..start]);
        output.push_str(replacement);
        cursor = end;
    }

    output.push_str(&text[cursor..]);
    output
}

fn is_structural_failure_noise_line(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.starts_with("```") {
        return true;
    }

    !trimmed.is_empty()
        && trimmed.chars().all(|ch| {
            ch.is_whitespace()
                || matches!(
                    ch,
                    '{' | '}' | '[' | ']' | '(' | ')' | ',' | ':' | ';' | '"' | '\''
                )
        })
}
