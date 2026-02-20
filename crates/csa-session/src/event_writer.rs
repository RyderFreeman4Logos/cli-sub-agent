use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::SecondsFormat;
use csa_acp::SessionEvent;
use serde::{Deserialize, Serialize};
use tracing::warn;

const TRANSCRIPT_SCHEMA_VERSION: u8 = 1;
const FLUSH_SIZE_BYTES: usize = 64 * 1024;
const FLUSH_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy, Default)]
struct ResumeState {
    next_seq: u64,
    existing_lines: u64,
}

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

#[derive(Deserialize)]
struct JsonlSeq {
    seq: u64,
}

impl EventWriter {
    pub fn new(output_path: &Path) -> Self {
        let resume_state = match load_resume_state(output_path) {
            Ok(state) => state,
            Err(err) => {
                warn!(
                    path = %output_path.display(),
                    error = %err,
                    "failed to inspect existing ACP transcript state"
                );
                ResumeState::default()
            }
        };

        let (writer, write_failures) = match open_transcript_file(output_path) {
            Ok(mut file) => match truncate_partial_trailing_line(output_path, &mut file) {
                Ok(()) => (Some(BufWriter::new(file)), 0),
                Err(err) => {
                    warn!(
                        path = %output_path.display(),
                        error = %err,
                        "failed to truncate partial trailing ACP transcript line"
                    );
                    (None, 1)
                }
            },
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
            seq: resume_state.next_seq,
            lines_written: resume_state.existing_lines,
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
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .read(true)
        .open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(file)
}

fn truncate_partial_trailing_line(path: &Path, file: &mut File) -> std::io::Result<()> {
    let file_len = file.metadata()?.len();
    if file_len == 0 {
        return Ok(());
    }

    file.seek(SeekFrom::End(-1))?;
    let mut last_byte = [0_u8; 1];
    file.read_exact(&mut last_byte)?;

    if last_byte[0] == b'\n' {
        file.seek(SeekFrom::End(0))?;
        return Ok(());
    }

    let bytes = std::fs::read(path)?;
    let truncate_len = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0_u64, |pos| pos as u64 + 1);

    file.set_len(truncate_len)?;
    file.seek(SeekFrom::End(0))?;
    Ok(())
}

fn load_resume_state(path: &Path) -> std::io::Result<ResumeState> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(ResumeState::default()),
        Err(err) => return Err(err),
    };

    if file.metadata()?.len() == 0 {
        return Ok(ResumeState::default());
    }

    let mut reader = BufReader::new(file);
    let mut line_buf = Vec::new();
    let mut existing_lines = 0_u64;
    let mut last_valid_next_seq: Option<u64> = None;

    loop {
        line_buf.clear();
        let read_bytes = reader.read_until(b'\n', &mut line_buf)?;
        if read_bytes == 0 {
            break;
        }

        let Some(last_byte) = line_buf.last() else {
            continue;
        };
        if *last_byte != b'\n' {
            continue;
        }

        existing_lines = existing_lines.saturating_add(1);
        let complete_line = &line_buf[..line_buf.len() - 1];
        if let Ok(parsed) = serde_json::from_slice::<JsonlSeq>(complete_line) {
            last_valid_next_seq = Some(parsed.seq.saturating_add(1));
        }
    }

    let next_seq = last_valid_next_seq.unwrap_or(0);

    Ok(ResumeState {
        next_seq,
        existing_lines,
    })
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
    fn test_writer_resumes_seq_and_total_line_count() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("output").join("acp-events.jsonl");

        {
            let mut first = EventWriter::new(&path);
            first.append(&SessionEvent::AgentMessage("hello".to_string()));
            first.append(&SessionEvent::AgentThought("thinking".to_string()));
            first.flush();
            assert_eq!(first.stats().lines_written, 2);
        }

        let mut resumed = EventWriter::new(&path);
        resumed.append(&SessionEvent::PlanUpdate(
            "{\"step\":\"resume\"}".to_string(),
        ));
        resumed.flush();

        let content = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 3);

        let seqs: Vec<u64> = lines
            .iter()
            .map(|line| {
                serde_json::from_str::<serde_json::Value>(line)
                    .unwrap()
                    .get("seq")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap()
            })
            .collect();
        assert_eq!(seqs, vec![0, 1, 2]);

        let stats = resumed.stats();
        assert_eq!(stats.lines_written, 3);
        assert_eq!(stats.write_failures, 0);
    }

    #[test]
    fn test_writer_resumes_from_last_valid_seq_when_tail_is_corrupted() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("output").join("acp-events.jsonl");

        {
            let mut first = EventWriter::new(&path);
            first.append(&SessionEvent::AgentMessage("hello".to_string()));
            first.append(&SessionEvent::AgentThought("thinking".to_string()));
            first.flush();
        }

        {
            let mut file = OpenOptions::new().append(true).open(&path).unwrap();
            file.write_all(b"{\"seq\":not-json}\n").unwrap();
        }

        let mut resumed = EventWriter::new(&path);
        resumed.append(&SessionEvent::PlanUpdate(
            "{\"step\":\"resume-after-corruption\"}".to_string(),
        ));
        resumed.flush();

        let content = std::fs::read_to_string(&path).unwrap();
        let valid_seqs: Vec<u64> = content
            .lines()
            .filter_map(|line| {
                serde_json::from_str::<serde_json::Value>(line)
                    .ok()?
                    .get("seq")
                    .and_then(serde_json::Value::as_u64)
            })
            .collect();
        assert_eq!(valid_seqs, vec![0, 1, 2]);
    }

    #[test]
    fn test_writer_truncates_partial_trailing_line_before_appending() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("output").join("acp-events.jsonl");

        {
            let mut first = EventWriter::new(&path);
            first.append(&SessionEvent::AgentMessage("hello".to_string()));
            first.append(&SessionEvent::AgentThought("thinking".to_string()));
            first.flush();
        }

        {
            let mut file = OpenOptions::new().append(true).open(&path).unwrap();
            file.write_all(br#"{"v":1,"seq":999,"marker":"PARTIAL-TAIL-DO-NOT-KEEP""#)
                .unwrap();
        }

        let mut resumed = EventWriter::new(&path);
        resumed.append(&SessionEvent::PlanUpdate(
            "{\"step\":\"resume-after-partial-tail\"}".to_string(),
        ));
        resumed.flush();

        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(bytes.last(), Some(&b'\n'));

        let content = String::from_utf8(bytes).unwrap();
        assert!(!content.contains("PARTIAL-TAIL-DO-NOT-KEEP"));

        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 3);

        let seqs: Vec<u64> = lines
            .iter()
            .map(|line| {
                serde_json::from_str::<serde_json::Value>(line)
                    .unwrap()
                    .get("seq")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap()
            })
            .collect();
        assert_eq!(seqs, vec![0, 1, 2]);

        let stats = resumed.stats();
        assert_eq!(stats.lines_written, 3);
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
