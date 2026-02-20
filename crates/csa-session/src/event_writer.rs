use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::SecondsFormat;
use csa_acp::SessionEvent;
use serde::Serialize;
use tracing::warn;

const TRANSCRIPT_SCHEMA_VERSION: u8 = 1;
const FLUSH_SIZE_BYTES: usize = 64 * 1024;
const FLUSH_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventWriteStats {
    pub lines_written: u64,
    pub bytes_written: u64,
    pub write_failures: u64,
}

#[derive(Debug)]
pub struct EventWriter {
    output_path: PathBuf,
    writer: Option<BufWriter<File>>,
    pending: Vec<u8>,
    pending_lines: u64,
    seq: u64,
    lines_written: u64,
    bytes_written: u64,
    write_failures: u64,
    last_flush: Instant,
}

#[derive(Serialize)]
struct JsonlEvent<'a> {
    v: u8,
    seq: u64,
    ts: String,
    #[serde(rename = "type")]
    event_type: &'static str,
    data: &'a SessionEvent,
}

impl EventWriter {
    pub fn new(output_path: &Path) -> Self {
        let (writer, write_failures) = match open_transcript_file(output_path) {
            Ok(file) => (Some(BufWriter::new(file)), 0),
            Err(err) => {
                warn!(
                    path = %output_path.display(),
                    error = %err,
                    "failed to initialize ACP transcript writer"
                );
                (None, 1)
            }
        };

        Self {
            output_path: output_path.to_path_buf(),
            writer,
            pending: Vec::new(),
            pending_lines: 0,
            seq: 0,
            lines_written: 0,
            bytes_written: 0,
            write_failures,
            last_flush: Instant::now(),
        }
    }

    pub fn append(&mut self, event: &SessionEvent) {
        let payload = JsonlEvent {
            v: TRANSCRIPT_SCHEMA_VERSION,
            seq: self.seq,
            ts: chrono::Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
            event_type: event_type(event),
            data: event,
        };

        match serde_json::to_vec(&payload) {
            Ok(mut line) => {
                self.seq = self.seq.saturating_add(1);
                line.push(b'\n');
                self.pending.extend_from_slice(&line);
                self.pending_lines = self.pending_lines.saturating_add(1);
                if self.should_flush() {
                    self.flush_internal();
                }
            }
            Err(err) => {
                self.write_failures = self.write_failures.saturating_add(1);
                warn!(
                    path = %self.output_path.display(),
                    seq = self.seq,
                    error = %err,
                    "failed to serialize ACP transcript event"
                );
            }
        }
    }

    pub fn append_all<'a, I>(&mut self, events: I)
    where
        I: IntoIterator<Item = &'a SessionEvent>,
    {
        for event in events {
            self.append(event);
        }
    }

    pub fn flush(&mut self) {
        self.flush_internal();
    }

    pub fn stats(&self) -> EventWriteStats {
        EventWriteStats {
            lines_written: self.lines_written,
            bytes_written: self.bytes_written,
            write_failures: self.write_failures,
        }
    }

    fn should_flush(&self) -> bool {
        self.pending.len() >= FLUSH_SIZE_BYTES || self.last_flush.elapsed() >= FLUSH_INTERVAL
    }

    fn flush_internal(&mut self) {
        if self.pending.is_empty() {
            self.last_flush = Instant::now();
            return;
        }

        let Some(writer) = self.writer.as_mut() else {
            self.write_failures = self.write_failures.saturating_add(1);
            self.pending.clear();
            self.pending_lines = 0;
            self.last_flush = Instant::now();
            warn!(
                path = %self.output_path.display(),
                "dropping buffered ACP transcript events because writer is unavailable"
            );
            return;
        };

        let pending_bytes = self.pending.len() as u64;
        let pending_lines = self.pending_lines;
        let write_result = writer.write_all(&self.pending).and_then(|_| writer.flush());

        self.last_flush = Instant::now();
        match write_result {
            Ok(()) => {
                self.bytes_written = self.bytes_written.saturating_add(pending_bytes);
                self.lines_written = self.lines_written.saturating_add(pending_lines);
            }
            Err(err) => {
                self.write_failures = self.write_failures.saturating_add(1);
                warn!(
                    path = %self.output_path.display(),
                    error = %err,
                    "failed to flush ACP transcript buffer"
                );
            }
        }

        self.pending.clear();
        self.pending_lines = 0;
    }
}

impl Drop for EventWriter {
    fn drop(&mut self) {
        self.flush_internal();
    }
}

fn event_type(event: &SessionEvent) -> &'static str {
    match event {
        SessionEvent::AgentMessage(_) => "message",
        SessionEvent::AgentThought(_) => "thought",
        SessionEvent::ToolCallStarted { .. } | SessionEvent::ToolCallCompleted { .. } => {
            "tool_call"
        }
        SessionEvent::PlanUpdate(_) => "plan",
        SessionEvent::Other(_) => "other",
    }
}

fn open_transcript_file(path: &Path) -> std::io::Result<File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new().create(true).append(true).open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_writer_persists_jsonl_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("output").join("acp-events.jsonl");
        let mut writer = EventWriter::new(&path);
        writer.append(&SessionEvent::AgentMessage("hello".to_string()));
        writer.append(&SessionEvent::PlanUpdate("{\"step\":\"x\"}".to_string()));
        writer.flush();

        let content = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("\"v\":1"));
        assert!(lines[0].contains("\"seq\":0"));
        assert!(lines[0].contains("\"type\":\"message\""));
        assert!(lines[1].contains("\"seq\":1"));
        assert!(lines[1].contains("\"type\":\"plan\""));

        let stats = writer.stats();
        assert_eq!(stats.lines_written, 2);
        assert_eq!(stats.write_failures, 0);
        assert!(stats.bytes_written > 0);
    }

    #[test]
    fn test_writer_flushes_on_drop() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("output").join("acp-events.jsonl");
        {
            let mut writer = EventWriter::new(&path);
            writer.append(&SessionEvent::AgentThought("thinking".to_string()));
        }

        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content.lines().count(), 1);
    }

    #[test]
    fn test_writer_flushes_on_interval_boundary() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("output").join("acp-events.jsonl");
        let mut writer = EventWriter::new(&path);
        writer.append(&SessionEvent::AgentMessage("a".to_string()));
        std::thread::sleep(Duration::from_millis(120));
        writer.append(&SessionEvent::AgentMessage("b".to_string()));
        writer.flush();

        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content.lines().count(), 2);
    }

    #[test]
    fn test_writer_is_resilient_on_open_failure() {
        let path = PathBuf::from("/dev/null/csa/acp-events.jsonl");
        let mut writer = EventWriter::new(&path);
        writer.append(&SessionEvent::Other("x".to_string()));
        writer.flush();
        assert!(writer.stats().write_failures >= 1);
    }

    #[cfg(unix)]
    #[test]
    fn test_writer_sets_strict_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("output").join("acp-events.jsonl");
        let mut writer = EventWriter::new(&path);
        writer.append(&SessionEvent::AgentMessage("hello".to_string()));
        writer.flush();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}
