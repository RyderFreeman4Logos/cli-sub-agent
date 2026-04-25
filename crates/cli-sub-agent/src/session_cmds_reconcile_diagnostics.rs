use csa_session::MetaSessionState;
use std::fs;
use std::io::{ErrorKind, Read, Seek, SeekFrom};
use std::path::Path;

/// Maximum bytes to read from the tail of a diagnostic log file.
/// Uses `take()` to cap the read even if the file grows between `metadata()` and `read`,
/// preventing unbounded memory allocation on large/growing logs.
const DIAGNOSTIC_TAIL_BYTES: u64 = 64 * 1024;
const DIAGNOSTIC_VALUE_MAX_CHARS: usize = 500;

pub(super) fn synthetic_failure_diagnostics(
    session_dir: &Path,
    session: &MetaSessionState,
    liveness_reason: &str,
) -> String {
    let mut lines = vec![
        "Diagnostics:".to_string(),
        format!("session_phase={:?}", session.phase),
        format!(
            "termination_reason={}",
            option_or_missing(session.termination_reason.as_deref())
        ),
        format!("last_accessed={}", session.last_accessed.to_rfc3339()),
        format!("liveness_reason={liveness_reason}"),
    ];

    push_file_diagnostic(
        &mut lines,
        "daemon_completion",
        &session_dir.join("daemon-completion.toml"),
    );
    push_file_diagnostic(&mut lines, "daemon_pid", &session_dir.join("daemon.pid"));
    push_file_diagnostic(&mut lines, "output_log", &session_dir.join("output.log"));
    push_file_diagnostic(&mut lines, "stderr_log", &session_dir.join("stderr.log"));
    push_file_diagnostic(
        &mut lines,
        "acp_events",
        &session_dir.join("output").join("acp-events.jsonl"),
    );
    push_file_diagnostic(
        &mut lines,
        "liveness_snapshot",
        &session_dir.join(".liveness.snapshot"),
    );

    if let Some(packet) = read_daemon_completion_summary(session_dir) {
        lines.push(format!("daemon_completion_packet={packet}"));
    }
    if let Some(pid_record) = read_small_file_compact(&session_dir.join("daemon.pid")) {
        lines.push(format!("daemon_pid_record={pid_record}"));
    }
    if let Some(heartbeat) = last_line_matching(&session_dir.join("output.log"), |line| {
        line.trim_start().starts_with("[csa-heartbeat]")
    }) {
        lines.push(format!("last_heartbeat={heartbeat}"));
    }
    if let Some(stderr_tail) = read_tail_compact(&session_dir.join("stderr.log")) {
        lines.push(format!("stderr_tail={stderr_tail}"));
        if let Some(hint) = classify_diagnostic_hint(&stderr_tail) {
            lines.push(format!("diagnostic_hint={hint}"));
        }
    }
    if let Some(acp_last_event) =
        last_nonempty_line(&session_dir.join("output").join("acp-events.jsonl"))
    {
        lines.push(format!("acp_last_event={acp_last_event}"));
    }

    format!("\n\n{}", lines.join("\n"))
}

fn push_file_diagnostic(lines: &mut Vec<String>, label: &str, path: &Path) {
    match fs::metadata(path) {
        Ok(metadata) => {
            let mtime = format_optional_file_mtime(path).unwrap_or_else(|| "unknown".to_string());
            lines.push(format!(
                "{label}=present size_bytes={} mtime={mtime}",
                metadata.len()
            ));
        }
        Err(err) if err.kind() == ErrorKind::NotFound => {
            lines.push(format!("{label}=missing"));
        }
        Err(err) => {
            lines.push(format!(
                "{label}=unreadable error={}",
                compact_diagnostic_value(&err.to_string())
            ));
        }
    }
}

fn read_daemon_completion_summary(session_dir: &Path) -> Option<String> {
    let contents = read_small_file_compact(&session_dir.join("daemon-completion.toml"))?;
    Some(contents.replace(" = ", "="))
}

fn read_small_file_compact(path: &Path) -> Option<String> {
    let metadata = fs::metadata(path).ok()?;
    if metadata.len() > DIAGNOSTIC_TAIL_BYTES {
        return read_tail_compact(path);
    }
    let contents = fs::read_to_string(path).ok()?;
    Some(compact_diagnostic_value(&contents))
}

fn read_tail_compact(path: &Path) -> Option<String> {
    Some(compact_diagnostic_value(&read_tail_lossy(path)?))
}

fn read_tail_lossy(path: &Path) -> Option<String> {
    let mut file = fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let cap = DIAGNOSTIC_TAIL_BYTES;
    if len <= cap {
        // Small file — read the whole thing (no seek needed).
        let mut buf = String::new();
        file.read_to_string(&mut buf).ok()?;
        return Some(buf);
    }
    // Seek to tail and cap the read with `take()` so a growing file cannot
    // cause unbounded allocation.
    file.seek(SeekFrom::Start(len - cap)).ok()?;
    let mut bytes = Vec::with_capacity(cap as usize);
    file.take(cap).read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    // The seek may land in the middle of a multi-byte UTF-8 char, producing a
    // replacement char at the start. Drop everything up to (and including) the
    // first newline so the returned tail starts at a clean line boundary.
    let trimmed = match text.find('\n') {
        Some(pos) => &text[pos + 1..],
        None => &text,
    };
    Some(trimmed.to_string())
}

fn last_line_matching(path: &Path, predicate: impl Fn(&str) -> bool) -> Option<String> {
    let tail = read_tail_lossy(path)?;
    tail.lines()
        .rev()
        .find(|line| predicate(line))
        .map(compact_diagnostic_value)
}

fn last_nonempty_line(path: &Path) -> Option<String> {
    let tail = read_tail_lossy(path)?;
    tail.lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .map(compact_diagnostic_value)
}

fn classify_diagnostic_hint(stderr_tail: &str) -> Option<&'static str> {
    let lowered = stderr_tail.to_ascii_lowercase();
    if lowered.contains("empty prompt from stdin") {
        return Some("empty_stdin_before_session_execution");
    }
    if lowered.contains("out of memory")
        || lowered
            .split_whitespace()
            .any(|word| word.trim_matches(|c: char| !c.is_alphanumeric()) == "oom")
        || lowered.contains("sigkill")
        || lowered.contains("exit code 137")
    {
        return Some("possible_oom_or_sigkill");
    }
    if lowered.contains("permission denied")
        || lowered.contains("operation not permitted")
        || lowered.contains("eacces")
        || lowered.contains("sandbox")
    {
        return Some("possible_sandbox_or_permission_denial");
    }
    if lowered.contains("server shut down unexpectedly") || lowered.contains("transport") {
        return Some("possible_transport_crash");
    }
    None
}

fn compact_diagnostic_value(value: &str) -> String {
    let mut compact = String::new();
    let mut chars = 0;

    for word in value.split_whitespace() {
        if !compact.is_empty() {
            if chars == DIAGNOSTIC_VALUE_MAX_CHARS {
                compact.push_str("...");
                return compact;
            }
            compact.push(' ');
            chars += 1;
        }

        for ch in word.chars() {
            if chars == DIAGNOSTIC_VALUE_MAX_CHARS {
                compact.push_str("...");
                return compact;
            }
            compact.push(ch);
            chars += 1;
        }
    }

    compact
}

fn option_or_missing(value: Option<&str>) -> &str {
    value.unwrap_or("missing")
}

fn format_optional_file_mtime(path: &Path) -> Option<String> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    let modified = chrono::DateTime::<chrono::Utc>::from(modified);
    Some(modified.to_rfc3339())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_hint_matches_oom_as_punctuated_word() {
        assert_eq!(
            classify_diagnostic_hint("kernel killed process: OOM."),
            Some("possible_oom_or_sigkill")
        );
    }

    #[test]
    fn diagnostic_hint_ignores_oom_substrings() {
        assert_eq!(classify_diagnostic_hint("room broom zoom"), None);
    }

    #[test]
    fn read_tail_lossy_small_file_returns_full_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("small.log");
        fs::write(&path, "hello\nworld\n").unwrap();
        let result = read_tail_lossy(&path).unwrap();
        assert_eq!(result, "hello\nworld\n");
    }

    #[test]
    fn read_tail_lossy_large_file_returns_bounded_tail() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.log");
        // Write a file larger than DIAGNOSTIC_TAIL_BYTES (64 KiB).
        let line = "A".repeat(80) + "\n"; // 81 bytes per line
        let lines_needed = (DIAGNOSTIC_TAIL_BYTES as usize / line.len()) + 200;
        let content: String = std::iter::repeat(line.as_str())
            .take(lines_needed)
            .collect();
        assert!(content.len() as u64 > DIAGNOSTIC_TAIL_BYTES);
        fs::write(&path, &content).unwrap();

        let result = read_tail_lossy(&path).unwrap();
        // Result must be smaller than cap (we drop the partial first line).
        assert!(result.len() <= DIAGNOSTIC_TAIL_BYTES as usize);
        // Result must not be empty.
        assert!(!result.is_empty());
        // Result must not start with a partial line (no replacement char).
        assert!(!result.starts_with('\u{FFFD}'));
    }

    #[test]
    fn read_tail_lossy_missing_file_returns_none() {
        let result = read_tail_lossy(Path::new("/nonexistent/file.log"));
        assert!(result.is_none());
    }
}
