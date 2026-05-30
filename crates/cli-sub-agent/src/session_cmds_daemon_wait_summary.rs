use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;

use anyhow::Result;

use super::SessionWaitOutputMode;

const WAIT_OUTPUT_MAX_BYTES: u64 = 1024 * 1024;

fn stream_wait_output(session_dir: &Path) -> Result<bool> {
    let stdout_log = session_dir.join("stdout.log");
    if !stdout_log.is_file() {
        return Ok(false);
    }
    let log = read_wait_output_log(&stdout_log)?;
    if log.truncated {
        eprintln!(
            "[csa] stdout.log exceeded {WAIT_OUTPUT_MAX_BYTES} bytes; showing bounded tail output"
        );
    }
    let Some(rendered) = render_wait_output_log(&log.raw, log.truncated) else {
        return Ok(false);
    };
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(rendered.as_bytes())?;
    let bytes = rendered.len() as u64;
    stdout.flush()?;
    Ok(bytes > 0)
}

pub(super) fn emit_wait_terminal_output(
    session_dir: &Path,
    session_id: &str,
    result: Option<&csa_session::SessionResult>,
    output_mode: SessionWaitOutputMode,
) -> Result<bool> {
    if output_mode == SessionWaitOutputMode::Verbose {
        return stream_wait_output(session_dir);
    }

    let Some(result) = result else {
        return Ok(false);
    };

    let rendered = match output_mode {
        SessionWaitOutputMode::CompactText => {
            render_wait_result_summary(session_dir, session_id, result)
        }
        SessionWaitOutputMode::CompactJson => {
            render_wait_result_json(session_dir, session_id, result)?
        }
        SessionWaitOutputMode::Verbose => unreachable!("handled above"),
    };
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(rendered.as_bytes())?;
    stdout.write_all(b"\n")?;
    stdout.flush()?;
    Ok(true)
}

fn render_wait_result_summary(
    session_dir: &Path,
    session_id: &str,
    result: &csa_session::SessionResult,
) -> String {
    let mut lines = vec![
        format!("Session: {session_id}"),
        format!("Status: {}", result.status),
        format!("Exit code: {}", result.exit_code),
        format!("Tool: {}", wait_result_tool_label(result)),
        format!("Elapsed: {}", format_wait_elapsed(result)),
    ];

    if let Some(tokens) = extract_wait_token_summary(result) {
        lines.push(format!("Tokens: {}", tokens.render_text()));
    }

    if let Some(verdict) = read_review_verdict_label(session_dir, result) {
        lines.push(format!("Review verdict: {verdict}"));
    }

    if let Some(summary) =
        crate::session_summary_text::human_session_summary(session_dir, &result.summary)
            .and_then(|text| compact_wait_summary_text(&text))
    {
        lines.push(format!("Summary: {summary}"));
    }

    lines.join("\n")
}

fn render_wait_result_json(
    session_dir: &Path,
    session_id: &str,
    result: &csa_session::SessionResult,
) -> Result<String> {
    let tokens = extract_wait_token_summary(result).map(|usage| {
        serde_json::json!({
            "input_tokens": usage.input_tokens,
            "output_tokens": usage.output_tokens,
            "total_tokens": usage.total_tokens,
            "cache_read_input_tokens": usage.cache_read_input_tokens,
        })
    });
    let value = serde_json::json!({
        "session_id": session_id,
        "status": result.status,
        "exit_code": result.exit_code,
        "tool": wait_result_tool_label(result),
        "elapsed_seconds": wait_elapsed_seconds(result),
        "tokens": tokens,
        "review_verdict": read_review_verdict_label(session_dir, result),
        "summary": crate::session_summary_text::human_session_summary(session_dir, &result.summary)
            .and_then(|text| compact_wait_summary_text(&text)),
    });
    serde_json::to_string_pretty(&value).map_err(Into::into)
}

fn wait_result_tool_label(result: &csa_session::SessionResult) -> String {
    match (&result.original_tool, &result.fallback_tool) {
        (Some(original), Some(fallback)) if original != fallback => {
            format!("{fallback} (fallback from {original})")
        }
        _ => result.tool.clone(),
    }
}

fn wait_elapsed_seconds(result: &csa_session::SessionResult) -> i64 {
    result
        .completed_at
        .signed_duration_since(result.started_at)
        .num_seconds()
        .max(0)
}

fn format_wait_elapsed(result: &csa_session::SessionResult) -> String {
    let seconds = wait_elapsed_seconds(result);
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        return format!("{hours}h {minutes}m {seconds}s");
    }
    if minutes > 0 {
        return format!("{minutes}m {seconds}s");
    }
    format!("{seconds}s")
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct WaitTokenSummary {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    total_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
}

impl WaitTokenSummary {
    fn render_text(self) -> String {
        let mut parts = Vec::new();
        if let Some(input) = self.input_tokens {
            parts.push(format!("input={input}"));
        }
        if let Some(output) = self.output_tokens {
            parts.push(format!("output={output}"));
        }
        if let Some(total) = self.total_tokens {
            parts.push(format!("total={total}"));
        }
        if let Some(cache_read) = self.cache_read_input_tokens {
            parts.push(format!("cache_read={cache_read}"));
        }
        parts.join(", ")
    }
}

fn extract_wait_token_summary(result: &csa_session::SessionResult) -> Option<WaitTokenSummary> {
    let summary_json: serde_json::Value = serde_json::from_str(&result.summary).ok()?;
    let usage = summary_json.get("usage")?;
    let input_tokens = usage
        .get("input_tokens")
        .and_then(serde_json::Value::as_u64);
    let output_tokens = usage
        .get("output_tokens")
        .and_then(serde_json::Value::as_u64);
    let total_tokens = usage
        .get("total_tokens")
        .and_then(serde_json::Value::as_u64)
        .or_else(|| {
            input_tokens
                .zip(output_tokens)
                .map(|(input, output)| input + output)
        });
    let cache_read_input_tokens = usage
        .get("cache_read_input_tokens")
        .or_else(|| usage.get("cached_input_tokens"))
        .and_then(serde_json::Value::as_u64);
    Some(WaitTokenSummary {
        input_tokens,
        output_tokens,
        total_tokens,
        cache_read_input_tokens,
    })
}

fn compact_wait_summary_text(summary: &str) -> Option<String> {
    let summary = summary.trim();
    if summary.is_empty() {
        return None;
    }
    const MAX_CHARS: usize = 500;
    let mut compact = summary.replace(['\r', '\n'], " ");
    if compact.chars().count() > MAX_CHARS {
        compact = compact.chars().take(MAX_CHARS).collect::<String>();
        compact.push_str("...");
    }
    Some(compact)
}

fn read_review_verdict_label(
    session_dir: &Path,
    result: &csa_session::SessionResult,
) -> Option<String> {
    let verdict_path = session_dir.join("output").join("review-verdict.json");
    if verdict_path.is_file()
        && let Ok(raw) = std::fs::read_to_string(&verdict_path)
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw)
    {
        return value
            .get("decision")
            .and_then(serde_json::Value::as_str)
            .or_else(|| {
                value
                    .get("verdict_legacy")
                    .and_then(serde_json::Value::as_str)
            })
            .map(|value| normalize_review_verdict_label(value, result));
    }

    let meta_path = session_dir.join("review_meta.json");
    if meta_path.is_file()
        && let Ok(raw) = std::fs::read_to_string(&meta_path)
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw)
    {
        return value
            .get("decision")
            .and_then(serde_json::Value::as_str)
            .or_else(|| value.get("verdict").and_then(serde_json::Value::as_str))
            .map(|value| normalize_review_verdict_label(value, result));
    }

    None
}

fn normalize_review_verdict_label(value: &str, result: &csa_session::SessionResult) -> String {
    match value.trim().to_ascii_uppercase().as_str() {
        "PASS" | "CLEAN" if result.exit_code != 0 => "UNAVAILABLE".to_string(),
        "PASS" | "CLEAN" => "PASS".to_string(),
        "FAIL" | "FAILED" | "HAS_ISSUES" => "FAIL".to_string(),
        other => other.to_string(),
    }
}

struct WaitOutputLog {
    raw: Vec<u8>,
    truncated: bool,
}

fn read_wait_output_log(stdout_log: &Path) -> Result<WaitOutputLog> {
    let mut file = File::open(stdout_log)?;
    let len = file.metadata()?.len();
    if len <= WAIT_OUTPUT_MAX_BYTES {
        let mut raw = Vec::with_capacity(len as usize);
        file.read_to_end(&mut raw)?;
        return Ok(WaitOutputLog {
            raw,
            truncated: false,
        });
    }

    let start = len - WAIT_OUTPUT_MAX_BYTES;
    file.seek(SeekFrom::Start(start))?;
    let mut reader = BufReader::new(file);
    discard_partial_line_if_needed(&mut reader, stdout_log, start)?;
    let mut raw = Vec::with_capacity(WAIT_OUTPUT_MAX_BYTES as usize);
    reader.take(WAIT_OUTPUT_MAX_BYTES).read_to_end(&mut raw)?;
    Ok(WaitOutputLog {
        raw,
        truncated: true,
    })
}

fn discard_partial_line_if_needed(
    reader: &mut BufReader<File>,
    stdout_log: &Path,
    start: u64,
) -> Result<()> {
    if start == 0 {
        return Ok(());
    }
    let mut boundary = File::open(stdout_log)?;
    boundary.seek(SeekFrom::Start(start - 1))?;
    let mut previous = [0_u8; 1];
    boundary.read_exact(&mut previous)?;
    if previous[0] == b'\n' {
        return Ok(());
    }
    let mut discarded = Vec::new();
    reader.read_until(b'\n', &mut discarded)?;
    Ok(())
}

fn render_wait_output_log(raw: &[u8], truncated: bool) -> Option<String> {
    if truncated {
        let raw_text = String::from_utf8_lossy(raw);
        return crate::codex_transcript_filter::extract_codex_json_event_text(raw_text.as_ref())
            .or_else(|| crate::codex_transcript_filter::render_codex_or_plain_output(raw));
    }
    crate::codex_transcript_filter::render_codex_or_plain_output(raw)
}

#[cfg(test)]
mod wait_output_tests {
    use chrono::Utc;

    use super::{
        WAIT_OUTPUT_MAX_BYTES, read_wait_output_log, render_wait_output_log,
        render_wait_result_summary,
    };

    #[test]
    fn read_wait_output_log_tails_large_stdout_without_loading_prefix() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let stdout_log = temp.path().join("stdout.log");
        let prefix = vec![b'a'; WAIT_OUTPUT_MAX_BYTES as usize];
        let suffix = b"\nfinal visible line\n";
        let mut content = prefix;
        content.extend_from_slice(suffix);
        std::fs::write(&stdout_log, content).expect("stdout log should be written");

        let log = read_wait_output_log(&stdout_log).expect("stdout log should be read");

        assert!(log.truncated);
        assert!(log.raw.len() <= WAIT_OUTPUT_MAX_BYTES as usize);
        let rendered = String::from_utf8(log.raw).expect("tail should be valid utf-8");
        assert_eq!(rendered, "final visible line\n");
    }

    #[test]
    fn render_truncated_codex_json_tail_filters_agent_messages() {
        let raw = [
            r#"{"type":"item.completed","item":{"type":"tool_result","text":"hidden shell output"}}"#,
            r#"{"type":"item.completed","item":{"type":"agent_message","text":"visible summary"}}"#,
        ]
        .join("\n");

        let rendered = render_wait_output_log(raw.as_bytes(), true)
            .expect("truncated codex transcript should render");

        assert_eq!(rendered, "visible summary");
        assert!(!rendered.contains("hidden shell output"));
    }

    #[test]
    fn compact_summary_includes_usage_and_review_verdict() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let output_dir = temp.path().join("output");
        std::fs::create_dir_all(&output_dir).expect("output dir should be created");
        std::fs::write(
            output_dir.join("review-verdict.json"),
            r#"{"decision":"pass","verdict_legacy":"CLEAN"}"#,
        )
        .expect("review verdict should be written");
        let now = Utc::now();
        let result = csa_session::SessionResult {
            status: "success".to_string(),
            exit_code: 0,
            summary: r#"{"type":"turn.completed","usage":{"input_tokens":100,"cached_input_tokens":40,"output_tokens":25}}"#.to_string(),
            tool: "codex".to_string(),
            original_tool: None,
            fallback_tool: None,
            fallback_reason: None,
            started_at: now,
            completed_at: now + chrono::TimeDelta::seconds(65),
            events_count: 0,
            artifacts: Vec::new(),
            peak_memory_mb: None,
            fallback_chain: None,
        gate_timeout: false,
            warnings: Vec::new(),
            raw_process_exit_code: None,
            manager_fields: Default::default(),
        };

        let summary = render_wait_result_summary(temp.path(), "01TESTWAITSUMMARY", &result);

        assert!(summary.len() <= 2048);
        assert!(summary.contains("Session: 01TESTWAITSUMMARY"));
        assert!(summary.contains("Elapsed: 1m 5s"));
        assert!(summary.contains("Tokens: input=100, output=25, total=125, cache_read=40"));
        assert!(summary.contains("Review verdict: PASS"));
    }

    #[test]
    fn compact_summary_does_not_print_pass_when_result_failed() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let output_dir = temp.path().join("output");
        std::fs::create_dir_all(&output_dir).expect("output dir should be created");
        std::fs::write(
            output_dir.join("review-verdict.json"),
            r#"{"decision":"pass","verdict_legacy":"CLEAN"}"#,
        )
        .expect("review verdict should be written");
        let now = Utc::now();
        let result = csa_session::SessionResult {
            status: "failed".to_string(),
            exit_code: 137,
            summary: "fatal backend error: process killed".to_string(),
            tool: "codex".to_string(),
            original_tool: None,
            fallback_tool: None,
            fallback_reason: None,
            started_at: now,
            completed_at: now + chrono::TimeDelta::seconds(65),
            events_count: 0,
            artifacts: Vec::new(),
            peak_memory_mb: None,
            fallback_chain: None,
            gate_timeout: false,
            warnings: Vec::new(),
            raw_process_exit_code: None,
            manager_fields: Default::default(),
        };

        let summary = render_wait_result_summary(temp.path(), "01TESTWAITFAILPASS", &result);

        assert!(!summary.contains("Review verdict: PASS"));
        assert!(summary.contains("Review verdict: UNAVAILABLE"));
        assert!(summary.contains("Summary: fatal backend error: process killed"));
    }
}
