use std::time::Instant;
use std::{cell::RefCell, collections::VecDeque, rc::Rc};

use crate::tool_output_compaction::ToolOutputCompactionState;
use agent_client_protocol::{
    Client, ContentBlock, ContentChunk, RequestPermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse, SelectedPermissionOutcome, SessionNotification, SessionUpdate,
    ToolCallContent, ToolCallUpdateFields,
};

/// Maximum bytes retained in the tail text buffer; shared with `csa-process::output_helpers`.
const TAIL_BUFFER_MAX_BYTES: usize = 1024 * 1024;

/// High-water mark for trimming, set to 2x [`TAIL_BUFFER_MAX_BYTES`] to amortize drain cost.
const TAIL_BUFFER_HIGH_WATER: usize = TAIL_BUFFER_MAX_BYTES * 2;

/// Maximum ACP session events retained in memory; 10K events is about 2 MiB at ~200 B/event.
pub(crate) const MAX_RETAINED_EVENTS: usize = 10_000;

/// Maximum number of execute command titles retained for post-run policy checks.
const MAX_EXTRACTED_COMMANDS: usize = 100;

/// Incrementally accumulated metadata from streamed ACP events.
#[derive(Debug, Clone, Default)]
pub struct StreamingMetadata {
    /// Total number of events seen across the entire prompt turn, including dropped events.
    pub total_events_count: usize,
    /// Number of agent conversation turns observed (one per `AgentMessage`).
    ///
    /// Mirrors `csa_core::transport_events::StreamingMetadata::turn_count`; copied
    /// through `convert_acp_metadata` in the executor crate. See that field's
    /// docstring for the legacy fallback contract.
    pub turn_count: u32,
    /// Whether any `ToolCallStarted` event was observed.
    pub has_tool_calls: bool,
    /// Whether any execute `ToolCallStarted` event was observed.
    pub has_execute_tool_calls: bool,
    /// Whether a `--no-verify` or `-n` git commit command was observed.
    /// Tracked separately from `extracted_commands` because the command ring
    /// buffer may evict old entries, but this safety flag must never be lost.
    pub has_no_verify_commit: bool,
    /// Whether any `PlanUpdate` event was observed.
    pub has_plan_updates: bool,
    /// Tail of execute command titles observed during the prompt turn.
    pub extracted_commands: Vec<String>,
    /// Tail buffer of agent message/thought text (bounded by [`TAIL_BUFFER_MAX_BYTES`]).
    pub tail_text: String,
    /// Tail buffer of agent message text only (bounded by [`TAIL_BUFFER_MAX_BYTES`]).
    pub message_text: String,
    /// Tail buffer of agent thought text only (bounded by [`TAIL_BUFFER_MAX_BYTES`]).
    pub thought_text: String,
    /// Whether the output used thought text as fallback (no message text was produced).
    pub has_thought_fallback: bool,
    /// Total bytes written to the output spool file.
    pub(crate) spool_bytes_written: u64,
    /// Total input tokens reported by the underlying API response, when available.
    pub input_tokens: Option<u64>,
    /// Total output tokens reported by the underlying API response, when available.
    pub output_tokens: Option<u64>,
    /// Cache-read input tokens reported by the Anthropic API response.
    ///
    /// Anthropic returns `cache_read_input_tokens` in the response `usage`
    /// block when prompt caching is active. Older API responses and non-Claude
    /// backends may omit it, hence `Option`.
    pub cache_read_input_tokens: Option<u64>,
}

impl StreamingMetadata {
    pub(crate) fn sync_from_store(&mut self, store: &SessionEventStore) {
        self.total_events_count = store.total_events_count();
        self.turn_count = store.turn_count();
        self.has_tool_calls = store.has_tool_calls();
        self.has_execute_tool_calls = store.has_execute_tool_calls();
        self.has_no_verify_commit = store.has_no_verify_commit();
        self.has_plan_updates = store.has_plan_updates();
        self.extracted_commands = store.extracted_commands();
    }

    /// Ratio of cache-read input tokens to total input tokens (`cache_read / input_tokens`).
    ///
    /// Returns `None` when either field is missing or when `input_tokens` is
    /// zero (no meaningful denominator).
    pub fn cache_hit_ratio(&self) -> Option<f64> {
        let cache_read = self.cache_read_input_tokens? as f64;
        let total_input = self.input_tokens? as f64;
        if total_input == 0.0 {
            return None;
        }
        Some(cache_read / total_input)
    }

    /// Append agent message text to both the message-specific and combined tail buffers.
    pub(crate) fn append_message_text(&mut self, text: &str) {
        self.tail_text.push_str(text);
        trim_tail_buffer(&mut self.tail_text);
        self.message_text.push_str(text);
        trim_tail_buffer(&mut self.message_text);
    }

    /// Append agent thought text to both the thought-specific and combined tail buffers.
    pub(crate) fn append_thought_text(&mut self, text: &str) {
        self.tail_text.push_str(text);
        trim_tail_buffer(&mut self.tail_text);
        self.thought_text.push_str(text);
        trim_tail_buffer(&mut self.thought_text);
    }
}

/// Trim a tail buffer back to [`TAIL_BUFFER_MAX_BYTES`] once it exceeds [`TAIL_BUFFER_HIGH_WATER`].
pub(crate) fn trim_tail_buffer(buf: &mut String) {
    if buf.len() > TAIL_BUFFER_HIGH_WATER {
        let excess = buf.len() - TAIL_BUFFER_MAX_BYTES;
        let mut trim_at = excess;
        while trim_at < buf.len() && !buf.is_char_boundary(trim_at) {
            trim_at += 1;
        }
        buf.drain(..trim_at);
    }
}

/// Streaming session events collected from ACP notifications.
#[derive(Debug, Clone, serde::Serialize)]
pub enum SessionEvent {
    AgentMessage(String),
    AgentThought(String),
    ToolCallStarted {
        id: String,
        title: String,
        kind: String,
    },
    ToolCallCompleted {
        id: String,
        status: String,
    },
    ToolCallOutput {
        id: String,
        title: Option<String>,
        status: String,
        output: String,
    },
    PlanUpdate(String),
    Other(String),
}

pub(crate) fn event_counts_as_initial_response(event: &SessionEvent) -> bool {
    matches!(
        event,
        SessionEvent::AgentMessage(_)
            | SessionEvent::AgentThought(_)
            | SessionEvent::PlanUpdate(_)
            | SessionEvent::ToolCallStarted { .. }
            | SessionEvent::ToolCallCompleted { .. }
            | SessionEvent::ToolCallOutput { .. }
    )
}

/// Bounded in-memory ACP event retention with incremental metadata extraction.
#[derive(Debug, Clone, Default)]
pub(crate) struct SessionEventStore {
    events: VecDeque<SessionEvent>,
    total_events_count: usize,
    turn_count: u32,
    has_tool_calls: bool,
    has_execute_tool_calls: bool,
    has_no_verify_commit: bool,
    has_plan_updates: bool,
    extracted_commands: VecDeque<String>,
}

impl SessionEventStore {
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    pub(crate) fn push(&mut self, event: SessionEvent) {
        self.total_events_count += 1;
        self.update_metadata(&event);
        self.events.push_back(event);
        if self.events.len() > MAX_RETAINED_EVENTS {
            let _ = self.events.pop_front();
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.events.len()
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn events(&self) -> Vec<SessionEvent> {
        self.events.iter().cloned().collect()
    }

    pub(crate) fn retained_events(&self) -> &VecDeque<SessionEvent> {
        &self.events
    }

    pub(crate) fn retained_start_index(&self) -> usize {
        self.total_events_count.saturating_sub(self.events.len())
    }

    pub(crate) fn total_events_count(&self) -> usize {
        self.total_events_count
    }

    pub(crate) fn turn_count(&self) -> u32 {
        self.turn_count
    }

    pub(crate) fn has_tool_calls(&self) -> bool {
        self.has_tool_calls
    }

    pub(crate) fn has_execute_tool_calls(&self) -> bool {
        self.has_execute_tool_calls
    }

    pub(crate) fn has_no_verify_commit(&self) -> bool {
        self.has_no_verify_commit
    }

    pub(crate) fn has_plan_updates(&self) -> bool {
        self.has_plan_updates
    }

    pub(crate) fn extracted_commands(&self) -> Vec<String> {
        self.extracted_commands.iter().cloned().collect()
    }

    pub(crate) fn take_events(&mut self) -> Vec<SessionEvent> {
        let retained = self.events.drain(..).collect();
        self.clear();
        retained
    }

    fn update_metadata(&mut self, event: &SessionEvent) {
        match event {
            SessionEvent::ToolCallStarted { title, kind, .. } => {
                self.has_tool_calls = true;
                if kind.eq_ignore_ascii_case("execute") {
                    self.has_execute_tool_calls = true;
                    self.push_extracted_command(title);
                }
            }
            SessionEvent::PlanUpdate(_) => {
                self.has_plan_updates = true;
            }
            SessionEvent::AgentMessage(_) => {
                self.turn_count = self.turn_count.saturating_add(1);
            }
            SessionEvent::ToolCallOutput { .. } => {
                self.has_tool_calls = true;
            }
            SessionEvent::AgentThought(_)
            | SessionEvent::ToolCallCompleted { .. }
            | SessionEvent::Other(_) => {}
        }
    }

    fn push_extracted_command(&mut self, title: &str) {
        let command = title.trim();
        if command.is_empty() {
            return;
        }
        // Sticky flag: once a --no-verify commit is seen, it can never be
        // evicted from the ring buffer and lost.
        if !self.has_no_verify_commit && command_looks_like_no_verify_commit(command) {
            self.has_no_verify_commit = true;
        }
        if self.extracted_commands.len() == MAX_EXTRACTED_COMMANDS {
            let _ = self.extracted_commands.pop_front();
        }
        self.extracted_commands.push_back(command.to_string());
    }
}

mod no_verify_detect;
use no_verify_detect::command_looks_like_no_verify_commit;

pub(crate) type SharedEvents = Rc<RefCell<SessionEventStore>>;
pub(crate) type SharedActivity = Rc<RefCell<Instant>>;
pub(crate) type SharedToolOutputCompactor = Rc<RefCell<Option<ToolOutputCompactionState>>>;

#[derive(Debug, Clone)]
pub(crate) struct AcpClient {
    events: SharedEvents,
    last_activity: SharedActivity,
    last_meaningful_activity: SharedActivity,
    tool_output_compactor: SharedToolOutputCompactor,
}

impl AcpClient {
    #[cfg(test)]
    pub(crate) fn new(
        events: SharedEvents,
        last_activity: SharedActivity,
        last_meaningful_activity: SharedActivity,
    ) -> Self {
        Self::new_with_tool_output_compactor(
            events,
            last_activity,
            last_meaningful_activity,
            Rc::new(RefCell::new(None)),
        )
    }

    pub(crate) fn new_with_tool_output_compactor(
        events: SharedEvents,
        last_activity: SharedActivity,
        last_meaningful_activity: SharedActivity,
        tool_output_compactor: SharedToolOutputCompactor,
    ) -> Self {
        Self {
            events,
            last_activity,
            last_meaningful_activity,
            tool_output_compactor,
        }
    }

    fn chunk_to_text(chunk: &ContentChunk) -> String {
        match &chunk.content {
            ContentBlock::Text(text) => text.text.clone(),
            other => serde_json::to_string(other).unwrap_or_else(|_| "<non-text-content>".into()),
        }
    }

    /// Convert an ACP `SessionUpdate` into an optional `SessionEvent`.
    ///
    /// Returns `None` for protocol-level overhead events that carry no
    /// meaningful content (e.g. `AvailableCommandsUpdate`, mode/config
    /// updates).  These are logged at trace level but excluded from
    /// collected output to prevent multi-KB JSON blobs from polluting
    /// stdout and summary extraction.
    #[cfg(test)]
    fn update_to_event(update: SessionUpdate) -> Option<SessionEvent> {
        Self::update_to_event_with_compactor(update, None)
    }

    fn update_to_event_with_compactor(
        update: SessionUpdate,
        compactor: Option<&mut ToolOutputCompactionState>,
    ) -> Option<SessionEvent> {
        match update {
            SessionUpdate::AgentMessageChunk(chunk) => {
                Some(SessionEvent::AgentMessage(Self::chunk_to_text(&chunk)))
            }
            SessionUpdate::AgentThoughtChunk(chunk) => {
                Some(SessionEvent::AgentThought(Self::chunk_to_text(&chunk)))
            }
            SessionUpdate::ToolCall(tool_call) => Some(SessionEvent::ToolCallStarted {
                id: tool_call.tool_call_id.0.to_string(),
                title: tool_call.title,
                kind: format!("{:?}", tool_call.kind),
            }),
            SessionUpdate::ToolCallUpdate(tool_call_update) => {
                let id = tool_call_update.tool_call_id.0.to_string();
                let output = tool_update_output_text(&tool_call_update.fields);
                let title = tool_call_update.fields.title.clone();
                let status = tool_call_update
                    .fields
                    .status
                    .as_ref()
                    .map(|status| format!("{status:?}"))
                    .unwrap_or_else(|| "Updated".to_string());
                if let Some(output) = output {
                    let output = if let Some(compactor) = compactor {
                        compactor.render_tool_output(
                            &id,
                            title.as_deref(),
                            status.as_str(),
                            output.as_str(),
                        )
                    } else {
                        output
                    };
                    return Some(SessionEvent::ToolCallOutput {
                        id,
                        title,
                        status,
                        output,
                    });
                }
                if let Some(status) = tool_call_update.fields.status {
                    Some(SessionEvent::ToolCallCompleted {
                        id,
                        status: format!("{status:?}"),
                    })
                } else {
                    Some(SessionEvent::Other(
                        serde_json::to_string(&tool_call_update)
                            .unwrap_or_else(|_| "tool_call_update".into()),
                    ))
                }
            }
            SessionUpdate::Plan(plan) => {
                let serialized = serde_json::to_string(&plan)
                    .unwrap_or_else(|_| "<plan-serialize-failed>".into());
                Some(SessionEvent::PlanUpdate(serialized))
            }
            // Protocol overhead: these carry large JSON payloads (slash
            // command lists, config toggles, mode switches) that are not
            // meaningful agent output.  Suppress from events to keep
            // stdout clean and summary extraction accurate.
            SessionUpdate::AvailableCommandsUpdate(_)
            | SessionUpdate::ConfigOptionUpdate(_)
            | SessionUpdate::CurrentModeUpdate(_)
            | SessionUpdate::UserMessageChunk(_) => {
                tracing::trace!("suppressed protocol-level SessionUpdate (not content)");
                None
            }
            // Catch-all for future ACP protocol variants (enum is
            // non-exhaustive).  Emit as Other for visibility.
            other => Some(SessionEvent::Other(
                serde_json::to_string(&other).unwrap_or_else(|_| "<unknown-update>".into()),
            )),
        }
    }

    fn update_to_event_for_session(&self, update: SessionUpdate) -> Option<SessionEvent> {
        let mut compactor_ref = self.tool_output_compactor.borrow_mut();
        Self::update_to_event_with_compactor(update, compactor_ref.as_mut())
    }
}

fn tool_update_output_text(fields: &ToolCallUpdateFields) -> Option<String> {
    let mut chunks = Vec::new();
    if let Some(contents) = &fields.content {
        for content in contents {
            match content {
                ToolCallContent::Content(content) => match &content.content {
                    ContentBlock::Text(text) => chunks.push(text.text.clone()),
                    other => chunks.push(
                        serde_json::to_string(other)
                            .unwrap_or_else(|_| "<non-text-tool-output>".to_string()),
                    ),
                },
                other => chunks.push(
                    serde_json::to_string(other)
                        .unwrap_or_else(|_| "<non-text-tool-output>".to_string()),
                ),
            }
        }
    }
    if let Some(raw_output) = &fields.raw_output {
        let rendered = raw_output
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| serde_json::to_string_pretty(raw_output).unwrap_or_default());
        if !rendered.is_empty() {
            chunks.push(rendered);
        }
    }
    if chunks.is_empty() {
        None
    } else {
        Some(chunks.join("\n"))
    }
}

#[async_trait::async_trait(?Send)]
impl Client for AcpClient {
    async fn request_permission(
        &self,
        args: RequestPermissionRequest,
    ) -> agent_client_protocol::Result<RequestPermissionResponse> {
        let outcome = args
            .options
            .first()
            .map(|first| {
                RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                    first.option_id.clone(),
                ))
            })
            .unwrap_or(RequestPermissionOutcome::Cancelled);

        Ok(RequestPermissionResponse::new(outcome))
    }

    async fn session_notification(
        &self,
        args: SessionNotification,
    ) -> agent_client_protocol::Result<()> {
        let now = Instant::now();
        // Idle-timeout remains broad: any ACP session notification counts
        // as transport liveness, even when suppressed from collected output.
        *self.last_activity.borrow_mut() = now;
        if let Some(event) = self.update_to_event_for_session(args.update) {
            if event_counts_as_initial_response(&event) {
                *self.last_meaningful_activity.borrow_mut() = now;
            }
            self.events.borrow_mut().push(event);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
