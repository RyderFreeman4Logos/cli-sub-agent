use csa_process::SpoolRotator;

use crate::client::{
    SessionEvent, SharedEvents, StreamingMetadata, event_counts_as_initial_response,
};

/// Maximum bytes buffered before a newline-free chunk is force-flushed.
pub(crate) const LINE_BUF_CAP: usize = 64 * 1024;

/// Flush complete lines and keep incomplete tails unless the buffer exceeds [`LINE_BUF_CAP`].
fn flush_complete_lines(buf: &mut String, prefix: &str) {
    while let Some(pos) = buf.find('\n') {
        let line: String = buf.drain(..=pos).collect();
        eprint!("{prefix}{line}");
    }
    if buf.len() > LINE_BUF_CAP {
        let remainder = std::mem::take(buf);
        eprintln!("{prefix}{remainder}");
    }
}

/// Flush any remaining buffered content, appending a terminating newline.
fn flush_remaining_buf(buf: &mut String, prefix: &str) {
    if !buf.is_empty() {
        let remainder = std::mem::take(buf);
        eprintln!("{prefix}{remainder}");
    }
}

pub(crate) fn stream_new_agent_messages(
    events: &SharedEvents,
    processed_event_count: &mut usize,
    stream_stdout_to_stderr: bool,
    output_spool: &mut Option<SpoolRotator>,
    metadata: &mut StreamingMetadata,
    stdout_line_buf: &mut String,
    thought_line_buf: &mut String,
) -> bool {
    // Track progress against total seen events because the retained deque can evict old entries.
    let events_ref = events.borrow();
    metadata.sync_from_store(&events_ref);
    if *processed_event_count >= events_ref.total_events_count() {
        return false;
    }
    let retained_start = events_ref.retained_start_index();
    let stream_start = (*processed_event_count).max(retained_start);
    if stream_start > *processed_event_count {
        let skipped = stream_start - *processed_event_count;
        tracing::warn!(
            skipped,
            retained_start,
            processed = *processed_event_count,
            "ACP event ring buffer overrun: {skipped} events were evicted before being streamed to spool/stderr"
        );
        // Avoid splicing pre-overrun partial lines with the first retained chunk.
        stdout_line_buf.clear();
        thought_line_buf.clear();
    }
    let skip = stream_start.saturating_sub(retained_start);
    let mut saw_initial_response_event = false;

    for event in events_ref.retained_events().iter().skip(skip) {
        saw_initial_response_event |= event_counts_as_initial_response(event);
        match event {
            SessionEvent::AgentMessage(chunk) => {
                if stream_stdout_to_stderr {
                    flush_remaining_buf(thought_line_buf, "[thought] ");
                    stdout_line_buf.push_str(chunk);
                    flush_complete_lines(stdout_line_buf, "[stdout] ");
                }
                spool_chunk(output_spool, chunk.as_bytes(), metadata);
                metadata.append_message_text(chunk);
            }
            SessionEvent::AgentThought(chunk) => {
                if stream_stdout_to_stderr {
                    flush_remaining_buf(stdout_line_buf, "[stdout] ");
                    thought_line_buf.push_str(chunk);
                    flush_complete_lines(thought_line_buf, "[thought] ");
                }
                spool_chunk(output_spool, chunk.as_bytes(), metadata);
                metadata.append_thought_text(chunk);
            }
            SessionEvent::PlanUpdate(plan) => {
                metadata.has_plan_updates = true;
                let msg = format!("[plan] {plan}\n");
                if stream_stdout_to_stderr {
                    flush_remaining_buf(stdout_line_buf, "[stdout] ");
                    flush_remaining_buf(thought_line_buf, "[thought] ");
                    eprint!("{msg}");
                }
                spool_chunk(output_spool, msg.as_bytes(), metadata);
            }
            SessionEvent::ToolCallStarted { title, kind, .. } => {
                metadata.has_tool_calls = true;
                let msg = format!("[tool:started] {title} ({kind})\n");
                if stream_stdout_to_stderr {
                    flush_remaining_buf(stdout_line_buf, "[stdout] ");
                    flush_remaining_buf(thought_line_buf, "[thought] ");
                    eprint!("{msg}");
                }
                spool_chunk(output_spool, msg.as_bytes(), metadata);
            }
            SessionEvent::ToolCallCompleted { status, .. } => {
                let msg = format!("[tool:completed] {status}\n");
                if stream_stdout_to_stderr {
                    flush_remaining_buf(stdout_line_buf, "[stdout] ");
                    flush_remaining_buf(thought_line_buf, "[thought] ");
                    eprint!("{msg}");
                }
                spool_chunk(output_spool, msg.as_bytes(), metadata);
            }
            SessionEvent::Other(payload) => {
                let msg = format!("[other] {payload}\n");
                if stream_stdout_to_stderr {
                    flush_remaining_buf(stdout_line_buf, "[stdout] ");
                    flush_remaining_buf(thought_line_buf, "[thought] ");
                    eprint!("{msg}");
                }
                spool_chunk(output_spool, msg.as_bytes(), metadata);
            }
        }
    }

    if stream_stdout_to_stderr {
        flush_remaining_buf(stdout_line_buf, "[stdout] ");
        flush_remaining_buf(thought_line_buf, "[thought] ");
    }

    *processed_event_count = events_ref.total_events_count();
    saw_initial_response_event
}

fn spool_chunk(spool: &mut Option<SpoolRotator>, bytes: &[u8], metadata: &mut StreamingMetadata) {
    if let Some(writer) = spool {
        let _ = writer.write(bytes);
        metadata.spool_bytes_written = writer.bytes_written();
    }
}

/// Collect agent-visible output, falling back to thought text when no message text exists.
pub(crate) fn collect_agent_output(metadata: &mut StreamingMetadata) -> String {
    let message = metadata.message_text.trim();
    if !message.is_empty() {
        return metadata.message_text.clone();
    }
    let thought = metadata.thought_text.trim();
    if !thought.is_empty() {
        metadata.has_thought_fallback = true;
        tracing::warn!(
            thought_bytes = metadata.thought_text.len(),
            "agent produced no message output; falling back to thought text"
        );
        // Keep the marker on its own line so CSA section markers remain parseable.
        return format!("[thought-fallback]\n{}", metadata.thought_text);
    }
    String::new()
}
