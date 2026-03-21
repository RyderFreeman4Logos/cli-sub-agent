use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use super::StreamMode;
use chrono::Utc;
use std::time::{Duration, Instant};

/// Maximum bytes retained in in-memory output accumulators (stdout/stderr).
///
/// Matches the ACP `StreamingMetadata::TAIL_BUFFER_MAX_BYTES` so that both
/// the ACP and legacy capture paths have consistent memory bounds.
pub(super) const TAIL_BUFFER_MAX_BYTES: usize = 1024 * 1024; // 1 MiB

/// High-water mark for output accumulator trimming (2× target size).
///
/// The accumulator is allowed to grow to this size before being trimmed back
/// to [`TAIL_BUFFER_MAX_BYTES`].  This amortises the O(N) cost of
/// `String::drain` so trimming occurs once per MiB of new text rather than
/// per chunk, avoiding O(N²) behaviour.
pub(super) const TAIL_BUFFER_HIGH_WATER: usize = TAIL_BUFFER_MAX_BYTES * 2; // 2 MiB

pub(super) const DEFAULT_HEARTBEAT_SECS: u64 = 20;
pub(super) const HEARTBEAT_INTERVAL_ENV: &str = "CSA_TOOL_HEARTBEAT_SECS";
pub const DEFAULT_SPOOL_MAX_BYTES: u64 = 32 * 1024 * 1024;
pub const DEFAULT_SPOOL_KEEP_ROTATED: bool = true;
const WORKSPACE_BOUNDARY_PATTERN_A: &str = "path not in workspace";
const WORKSPACE_BOUNDARY_PATTERN_B: &str = "outside the allowed workspace directories";
const OPAQUE_OBJECT_PATTERN: &str = "[object object]";
const OPAQUE_OBJECT_REPLACEMENT: &str = "(opaque error payload)";
const OPAQUE_PAYLOAD_MARKER: &str = "(opaque error payload)";
const SPOOL_BUFFER_CAPACITY: usize = 64 * 1024;

#[derive(Debug)]
pub struct SpoolRotator {
    path: PathBuf,
    rotated_path: PathBuf,
    writer: Option<BufWriter<File>>,
    initial_offset: u64,
    current_file_bytes: u64,
    bytes_written: u64,
    max_bytes: u64,
    keep_rotated: bool,
    rotation_count: u64,
}

#[derive(Debug, Clone)]
pub struct SpoolSanitizationPlan {
    current_path: PathBuf,
    current_start_offset: u64,
    rotated: Option<(PathBuf, u64)>,
    keep_rotated: bool,
}

impl SpoolRotator {
    pub fn open(path: &Path, max_bytes: u64, keep_rotated: bool) -> io::Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        let initial_offset = file.metadata()?.len();
        Ok(Self {
            path: path.to_path_buf(),
            rotated_path: path.with_extension("log.rotated"),
            writer: Some(BufWriter::with_capacity(SPOOL_BUFFER_CAPACITY, file)),
            initial_offset,
            current_file_bytes: initial_offset,
            bytes_written: initial_offset,
            max_bytes: max_bytes.max(1),
            keep_rotated,
            rotation_count: 0,
        })
    }

    pub fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }

        let incoming = bytes.len() as u64;
        if self.current_file_bytes > self.max_bytes
            || (self.current_file_bytes > 0
                && self.current_file_bytes.saturating_add(incoming) > self.max_bytes)
        {
            self.rotate()?;
        }

        self.writer_mut()?.write_all(bytes)?;
        self.current_file_bytes = self.current_file_bytes.saturating_add(incoming);
        self.bytes_written = self.bytes_written.saturating_add(incoming);
        Ok(())
    }

    pub fn bytes_written(&self) -> u64 {
        self.bytes_written
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.writer_mut()?.flush()
    }

    pub fn finalize(mut self) -> io::Result<SpoolSanitizationPlan> {
        self.flush()?;
        self.writer.take();
        Ok(self.sanitization_plan())
    }

    fn writer_mut(&mut self) -> io::Result<&mut BufWriter<File>> {
        self.writer
            .as_mut()
            .ok_or_else(|| io::Error::other("spool writer already finalized"))
    }

    fn rotate(&mut self) -> io::Result<()> {
        if let Some(mut writer) = self.writer.take() {
            writer.flush()?;
            let file = writer.into_inner()?;
            drop(file);
        }

        if self.rotated_path.exists() {
            match std::fs::remove_file(&self.rotated_path) {
                Ok(()) => {}
                Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                Err(err) => return Err(err),
            }
        }

        std::fs::rename(&self.path, &self.rotated_path)?;

        let mut writer = BufWriter::with_capacity(
            SPOOL_BUFFER_CAPACITY,
            OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&self.path)?,
        );
        let sentinel = format!(
            "[CSA:TRUNCATED bytes_written={} rotated_at={}]\n",
            self.bytes_written,
            Utc::now().to_rfc3339()
        );
        writer.write_all(sentinel.as_bytes())?;
        self.current_file_bytes = sentinel.len() as u64;
        self.bytes_written = self.bytes_written.saturating_add(sentinel.len() as u64);
        self.rotation_count = self.rotation_count.saturating_add(1);
        self.writer = Some(writer);
        Ok(())
    }

    fn sanitization_plan(&self) -> SpoolSanitizationPlan {
        let current_start_offset = if self.rotation_count == 0 {
            self.initial_offset
        } else {
            0
        };
        let rotated = if self.rotation_count == 0 {
            None
        } else {
            let rotated_start_offset = if self.rotation_count == 1 {
                self.initial_offset
            } else {
                0
            };
            Some((self.rotated_path.clone(), rotated_start_offset))
        };

        SpoolSanitizationPlan {
            current_path: self.path.clone(),
            current_start_offset,
            rotated,
            keep_rotated: self.keep_rotated,
        }
    }
}

/// Write a raw byte chunk to the spool file and flush.
///
/// Best-effort: errors are silently ignored because the spool is a crash-recovery
/// aid, not the primary output path.
pub(super) fn spool_chunk(spool: &mut Option<SpoolRotator>, bytes: &[u8]) {
    if let Some(spool) = spool {
        let _ = spool.write(bytes);
        let _ = spool.flush();
    }
}

/// Drain the front of an output accumulator if it exceeds the high-water mark.
///
/// This bounds the in-memory accumulator to ~[`TAIL_BUFFER_HIGH_WATER`] bytes,
/// preventing unbounded growth from long-running tools.  After trimming, the
/// accumulator retains the most recent [`TAIL_BUFFER_MAX_BYTES`] of content.
///
/// The trim point is always on a char boundary to avoid splitting multi-byte
/// UTF-8 characters.
pub(super) fn drain_if_over_high_water(buf: &mut String) {
    if buf.len() > TAIL_BUFFER_HIGH_WATER {
        let excess = buf.len() - TAIL_BUFFER_MAX_BYTES;
        let mut trim_at = excess;
        while trim_at < buf.len() && !buf.is_char_boundary(trim_at) {
            trim_at += 1;
        }
        buf.drain(..trim_at);
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

pub(super) fn resolve_actionable_failure_detail(summary: &str, exit_code: i32) -> String {
    let trimmed = summary.trim();
    if trimmed.is_empty() || is_opaque_failure_detail(trimmed) {
        return format!("exit code {exit_code}");
    }
    trimmed.to_string()
}

pub(super) fn append_actionable_detail_for_opaque_payload(text: &str, detail: &str) -> String {
    if detail.trim().is_empty() {
        return text.to_string();
    }
    if !contains_opaque_payload_marker(text) {
        return text.to_string();
    }

    let mut output = remove_opaque_payload_lines(text);
    let annotation = format!("resolved failure detail: {}", detail.trim());
    if output.lines().any(|line| line.trim() == annotation) {
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        return output;
    }
    if !output.is_empty() && !output.ends_with('\n') {
        output.push('\n');
    }
    output.push_str(&annotation);
    output.push('\n');
    output
}

/// Best-effort tail sanitization for spool files.
///
/// Rewrites only bytes appended during the current run (`start_offset..`) so
/// historical spool content from previous turns is preserved.
pub(super) fn sanitize_spool_tail(
    path: &Path,
    start_offset: u64,
    actionable_detail: Option<&str>,
) -> std::io::Result<()> {
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
    if !contains_opaque_object_payload(&tail_text) && !contains_opaque_payload_marker(&tail_text) {
        return Ok(());
    }

    let mut sanitized_tail = sanitize_opaque_object_payloads(&tail_text);
    if let Some(detail) = actionable_detail {
        sanitized_tail = append_actionable_detail_for_opaque_payload(&sanitized_tail, detail);
    }
    if sanitized_tail == tail_text {
        return Ok(());
    }

    file.set_len(start_offset)?;
    file.seek(SeekFrom::Start(start_offset))?;
    file.write_all(sanitized_tail.as_bytes())?;
    file.flush()?;
    Ok(())
}

pub fn sanitize_spool_plan(
    plan: SpoolSanitizationPlan,
    actionable_detail: Option<&str>,
) -> io::Result<()> {
    sanitize_spool_tail(
        &plan.current_path,
        plan.current_start_offset,
        actionable_detail,
    )?;
    if let Some((rotated_path, rotated_start_offset)) = plan.rotated.as_ref()
        && rotated_path.exists()
    {
        sanitize_spool_tail(rotated_path, *rotated_start_offset, actionable_detail)?;
    }

    if !plan.keep_rotated
        && let Some((rotated_path, _)) = plan.rotated
    {
        match std::fs::remove_file(rotated_path) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
    }

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

fn contains_opaque_payload_marker(text: &str) -> bool {
    text.to_ascii_lowercase().contains(OPAQUE_PAYLOAD_MARKER)
}

fn is_opaque_failure_detail(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("opaque error payload") || lower.contains("opaque tool error payload")
}

fn remove_opaque_payload_lines(text: &str) -> String {
    let mut kept_lines = Vec::new();
    for line in text.lines() {
        if !contains_opaque_payload_marker(line) {
            kept_lines.push(line);
        }
    }

    if kept_lines.is_empty() {
        return String::new();
    }

    let mut output = kept_lines.join("\n");
    if text.ends_with('\n') {
        output.push('\n');
    }
    output
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

/// Result of attempting to compress tool output.
pub enum CompressDecision {
    /// Output is small enough or contains protected markers; pass through unchanged.
    PassThrough,
    /// Output exceeds threshold and should be compressed.
    ///
    /// Contains the original bytes and a replacement summary line.
    Compress {
        original_bytes: usize,
        replacement: String,
    },
}

/// Markers that must never be compressed.
///
/// Includes fork-call protocol, structured output, review verdicts, and
/// workflow variable declarations — compressing these would break downstream
/// consumers (verdict parsing, `${STEP_N_OUTPUT}` injection, etc.).
const PROTECTED_MARKERS: &[&str] = &[
    "CSA:SECTION",
    "ReturnPacket",
    "<!-- CSA:SECTION:",
    "final_decision:",
    "CSA_VAR:",
];

/// Decide whether a tool output should be compressed.
///
/// Returns `PassThrough` when the output is below `threshold_bytes` or
/// contains protected markers (CSA:SECTION, ReturnPacket).
pub fn should_compress_output(output: &str, threshold_bytes: u64) -> CompressDecision {
    let byte_len = output.len();
    if (byte_len as u64) <= threshold_bytes {
        return CompressDecision::PassThrough;
    }
    // Never compress outputs containing protocol markers.
    for marker in PROTECTED_MARKERS {
        if output.contains(marker) {
            return CompressDecision::PassThrough;
        }
    }
    CompressDecision::Compress {
        original_bytes: byte_len,
        replacement: format!("[Tool output compressed: {byte_len} bytes]"),
    }
}
