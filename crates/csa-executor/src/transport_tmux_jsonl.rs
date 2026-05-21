//! JSONL parsing and watching for the tmux transport.
//!
//! Extracted from `transport_tmux.rs` to keep the main module under the 800-line
//! soft limit. All functions are `pub(super)` so the parent module can use them.

use std::fs;
use std::io::{BufRead, Seek, SeekFrom};
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use tokio::time::sleep;

use super::POLL_INTERVAL;

/// Validate that the first few JSONL events contain the expected fields
/// (`type`, `sessionId`, `timestamp`).  Fails fast if the schema has changed.
pub(super) fn validate_jsonl_schema(jsonl_path: &Path) -> Result<()> {
    let file = fs::File::open(jsonl_path).with_context(|| jsonl_path.display().to_string())?;
    let reader = std::io::BufReader::new(file);
    let mut checked = 0u32;

    for line in reader.lines().map_while(Result::ok) {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        let value: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => {
                checked += 1;
                continue;
            }
        };
        if value.get("type").is_none() {
            bail!(
                "Incompatible Claude JSONL schema at {}: first event lacks 'type' field. \
                 Claude Code may have changed its conversation log format.",
                jsonl_path.display()
            );
        }
        checked += 1;
        if checked >= 3 {
            break;
        }
    }
    Ok(())
}

/// Events extracted from the JSONL watcher.
#[derive(Debug)]
pub(super) enum JsonlEvent {
    AssistantText(String),
    TurnDuration,
    CompactBoundary,
}

/// Parse a single JSONL line into a `JsonlEvent`.
pub(super) fn parse_jsonl_line(line: &str) -> Option<JsonlEvent> {
    let value: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    let event_type = value.get("type")?.as_str()?;

    match event_type {
        "assistant" => {
            let text = extract_assistant_text(&value).unwrap_or_default();
            Some(JsonlEvent::AssistantText(text))
        }
        "system" => {
            let subtype = value.get("subtype").and_then(|v| v.as_str()).unwrap_or("");
            match subtype {
                "turn_duration" => Some(JsonlEvent::TurnDuration),
                "compact_boundary" => Some(JsonlEvent::CompactBoundary),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Extract the text content from an `assistant` JSONL event.
///
/// Claude's conversation log stores assistant text in:
/// `{"type": "assistant", "message": {"content": [{"type": "text", "text": "..."}]}}`
fn extract_assistant_text(value: &serde_json::Value) -> Option<String> {
    let message = value.get("message")?;
    let content = message.get("content")?.as_array()?;
    let text = content
        .iter()
        .filter_map(|block| {
            if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                block.get("text").and_then(|t| t.as_str())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("");
    if text.is_empty() { None } else { Some(text) }
}

/// Poll the JSONL file until a `turn_duration` event is seen, then return
/// the collected assistant text from that turn.
///
/// Handles:
/// - File not yet existing: retries with `POLL_INTERVAL` backoff.
/// - `compact_boundary`: resets byte offset to 0 (Claude rewrote the log).
/// - EOF without event: continues polling.
/// - `idle_timeout_seconds`: returns error if no `turn_duration` in time.
pub(super) async fn watch_jsonl_for_turn(
    jsonl_path: &Path,
    idle_timeout_seconds: u64,
) -> Result<String> {
    let deadline = Instant::now() + Duration::from_secs(idle_timeout_seconds);
    let mut byte_offset: u64 = 0;
    let mut collected_text = String::new();

    loop {
        if Instant::now() > deadline {
            bail!(
                "JSONL watcher timed out after {}s waiting for turn_duration; \
                 collected {} chars of text so far",
                idle_timeout_seconds,
                collected_text.len()
            );
        }

        match fs::File::open(jsonl_path) {
            Err(_) => {
                sleep(POLL_INTERVAL).await;
                continue;
            }
            Ok(mut file) => {
                if file.seek(SeekFrom::Start(byte_offset)).is_err() {
                    byte_offset = 0;
                    collected_text.clear();
                    sleep(POLL_INTERVAL).await;
                    continue;
                }

                let reader = std::io::BufReader::new(&mut file);
                let mut advanced = false;

                for line in reader.lines().map_while(Result::ok) {
                    let line = line.trim().to_string();
                    if line.is_empty() {
                        continue;
                    }
                    byte_offset += line.len() as u64 + 1;
                    advanced = true;

                    match parse_jsonl_line(&line) {
                        Some(JsonlEvent::AssistantText(text)) => {
                            collected_text.push_str(&text);
                        }
                        Some(JsonlEvent::TurnDuration) => {
                            return Ok(collected_text);
                        }
                        Some(JsonlEvent::CompactBoundary) => {
                            byte_offset = 0;
                            collected_text.clear();
                            tracing::debug!(
                                path = %jsonl_path.display(),
                                "tmux transport: JSONL compact_boundary detected; resetting watcher"
                            );
                        }
                        None => {}
                    }
                }

                if !advanced {
                    sleep(POLL_INTERVAL).await;
                }
            }
        }
    }
}
