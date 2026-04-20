use std::time::Instant;
use std::{cell::RefCell, collections::VecDeque, rc::Rc};

use agent_client_protocol::{
    Client, ContentBlock, ContentChunk, RequestPermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse, SelectedPermissionOutcome, SessionNotification, SessionUpdate,
};

/// Maximum bytes retained in the tail text buffer.
///
/// Agent message and thought text beyond this limit is discarded from memory
/// after being written to the output spool on disk.  1 MiB is sufficient for
/// summary extraction and token-usage parsing, which only inspect the tail.
///
/// Canonical values shared with `csa-process::output_helpers` (same 1 MiB / 2 MiB).
const TAIL_BUFFER_MAX_BYTES: usize = 1024 * 1024;

/// High-water mark for tail buffer trimming (2× the target size).
///
/// We allow the buffer to grow to this size before trimming it back to
/// [`TAIL_BUFFER_MAX_BYTES`].  This amortises the O(N) cost of
/// `String::drain` so that trimming occurs once per MiB of new text
/// rather than once per chunk, avoiding O(N²) behaviour.
const TAIL_BUFFER_HIGH_WATER: usize = TAIL_BUFFER_MAX_BYTES * 2;

/// Maximum number of ACP session events retained in memory.
///
/// Set high enough to absorb bursts from parallel test output (cargo
/// nextest can emit thousands of lines per second) without overrunning
/// the 200ms polling interval in `stream_new_agent_messages`.  At ~200
/// bytes per event, 10K events ≈ 2 MiB — negligible vs the old unbounded
/// accumulation that reached 6+ GiB.
pub(crate) const MAX_RETAINED_EVENTS: usize = 10_000;

/// Maximum number of execute command titles retained for post-run policy checks.
const MAX_EXTRACTED_COMMANDS: usize = 100;

/// Incrementally accumulated metadata from streamed ACP events.
///
/// Built up by [`super::connection::stream_new_agent_messages`] as events flow
/// through, avoiding the need to keep the full event vector in memory.
#[derive(Debug, Clone, Default)]
pub struct StreamingMetadata {
    /// Total number of events seen across the entire prompt turn, including dropped events.
    pub total_events_count: usize,
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
}

impl StreamingMetadata {
    pub(crate) fn sync_from_store(&mut self, store: &SessionEventStore) {
        self.total_events_count = store.total_events_count();
        self.has_tool_calls = store.has_tool_calls();
        self.has_execute_tool_calls = store.has_execute_tool_calls();
        self.has_no_verify_commit = store.has_no_verify_commit();
        self.has_plan_updates = store.has_plan_updates();
        self.extracted_commands = store.extracted_commands();
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

/// Trim a tail buffer back to [`TAIL_BUFFER_MAX_BYTES`] when it exceeds
/// [`TAIL_BUFFER_HIGH_WATER`], respecting UTF-8 char boundaries.
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
    )
}

/// Bounded in-memory ACP event retention with incremental metadata extraction.
#[derive(Debug, Clone, Default)]
pub(crate) struct SessionEventStore {
    events: VecDeque<SessionEvent>,
    total_events_count: usize,
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
            SessionEvent::AgentMessage(_)
            | SessionEvent::AgentThought(_)
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

/// Quick heuristic: does a tool-call title look like `git commit --no-verify`
/// or `git commit -n`?  This is intentionally simpler than the full shell
/// parser in `run_cmd_shell.rs` because it only needs to catch the common
/// pattern within ACP execute-tool-call titles (which are short, single
/// commands).  The authoritative check still runs in
/// `apply_no_verify_commit_policy`; this flag merely ensures the event is
/// never silently evicted from the bounded ring buffer.
fn command_looks_like_no_verify_commit(cmd: &str) -> bool {
    let lower = cmd.to_ascii_lowercase();
    if !lower.contains("git") || !lower.contains("commit") {
        return false;
    }
    lower.contains("--no-verify")
        || lower.contains("-n ")
        || lower.ends_with("-n")
        || lower.contains("--no-gpg-sign") // also suspicious in same context
}

pub(crate) type SharedEvents = Rc<RefCell<SessionEventStore>>;
pub(crate) type SharedActivity = Rc<RefCell<Instant>>;

#[derive(Debug, Clone)]
pub(crate) struct AcpClient {
    events: SharedEvents,
    last_activity: SharedActivity,
    last_meaningful_activity: SharedActivity,
}

impl AcpClient {
    pub(crate) fn new(
        events: SharedEvents,
        last_activity: SharedActivity,
        last_meaningful_activity: SharedActivity,
    ) -> Self {
        Self {
            events,
            last_activity,
            last_meaningful_activity,
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
    fn update_to_event(update: SessionUpdate) -> Option<SessionEvent> {
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
        if let Some(event) = Self::update_to_event(args.update) {
            if event_counts_as_initial_response(&event) {
                *self.last_meaningful_activity.borrow_mut() = now;
            }
            self.events.borrow_mut().push(event);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{
        cell::RefCell,
        rc::Rc,
        time::{Duration, Instant},
    };

    use agent_client_protocol::{
        ContentBlock, ContentChunk, SessionUpdate, TextContent, ToolCall, ToolCallStatus,
        ToolCallUpdate, ToolCallUpdateFields, ToolKind,
    };

    use super::{
        AcpClient, MAX_EXTRACTED_COMMANDS, MAX_RETAINED_EVENTS, SessionEvent, SessionEventStore,
    };

    #[test]
    fn test_update_to_event_agent_message_chunk() {
        let chunk = ContentChunk::new(ContentBlock::Text(TextContent::new("hello")));
        let event = AcpClient::update_to_event(SessionUpdate::AgentMessageChunk(chunk))
            .expect("AgentMessageChunk should produce an event");

        match event {
            SessionEvent::AgentMessage(text) => assert_eq!(text, "hello"),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn test_update_to_event_agent_thought_chunk() {
        let chunk = ContentChunk::new(ContentBlock::Text(TextContent::new("thinking")));
        let event = AcpClient::update_to_event(SessionUpdate::AgentThoughtChunk(chunk))
            .expect("AgentThoughtChunk should produce an event");

        match event {
            SessionEvent::AgentThought(text) => assert_eq!(text, "thinking"),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn test_update_to_event_tool_call_started() {
        let tool_call = ToolCall::new("call-1", "Run tests").kind(ToolKind::Execute);
        let event = AcpClient::update_to_event(SessionUpdate::ToolCall(tool_call))
            .expect("ToolCall should produce an event");

        match event {
            SessionEvent::ToolCallStarted { id, title, kind } => {
                assert_eq!(id, "call-1");
                assert_eq!(title, "Run tests");
                assert_eq!(kind, "Execute");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn test_update_to_event_tool_call_completed() {
        let fields = ToolCallUpdateFields::new().status(ToolCallStatus::Completed);
        let update = ToolCallUpdate::new("call-2", fields);
        let event = AcpClient::update_to_event(SessionUpdate::ToolCallUpdate(update))
            .expect("ToolCallUpdate with status should produce an event");

        match event {
            SessionEvent::ToolCallCompleted { id, status } => {
                assert_eq!(id, "call-2");
                assert_eq!(status, "Completed");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn test_update_to_event_suppresses_protocol_overhead() {
        use agent_client_protocol::{AvailableCommand, AvailableCommandsUpdate};

        let commands_update =
            AvailableCommandsUpdate::new(vec![AvailableCommand::new("/help", "Get help")]);
        let result =
            AcpClient::update_to_event(SessionUpdate::AvailableCommandsUpdate(commands_update));
        assert!(
            result.is_none(),
            "AvailableCommandsUpdate should be suppressed"
        );
    }

    #[tokio::test]
    async fn test_session_notification_appends_content_event() {
        use agent_client_protocol::{Client, SessionNotification};

        let events = Rc::new(RefCell::new(SessionEventStore::default()));
        let last_activity = Rc::new(RefCell::new(Instant::now() - Duration::from_secs(2)));
        let last_meaningful_activity =
            Rc::new(RefCell::new(Instant::now() - Duration::from_secs(2)));
        let before = *last_activity.borrow();
        let meaningful_before = *last_meaningful_activity.borrow();
        let client = AcpClient::new(
            Rc::clone(&events),
            Rc::clone(&last_activity),
            Rc::clone(&last_meaningful_activity),
        );

        let chunk = ContentChunk::new(ContentBlock::Text(TextContent::new("hello")));
        let notification =
            SessionNotification::new("test-session", SessionUpdate::AgentMessageChunk(chunk));
        client.session_notification(notification).await.unwrap();

        let stored = events.borrow();
        let retained = stored.events();
        assert_eq!(retained.len(), 1);
        match &retained[0] {
            SessionEvent::AgentMessage(text) => assert_eq!(text, "hello"),
            other => panic!("unexpected stored event: {other:?}"),
        }
        assert!(
            *last_activity.borrow() > before,
            "session_notification must refresh last_activity"
        );
        assert!(
            *last_meaningful_activity.borrow() > meaningful_before,
            "content events must refresh last_meaningful_activity"
        );
    }

    #[tokio::test]
    async fn test_session_notification_suppresses_protocol_event_but_refreshes_activity() {
        use agent_client_protocol::{
            AvailableCommand, AvailableCommandsUpdate, Client, SessionNotification,
        };

        let events = Rc::new(RefCell::new(SessionEventStore::default()));
        let last_activity = Rc::new(RefCell::new(Instant::now() - Duration::from_secs(2)));
        let last_meaningful_activity =
            Rc::new(RefCell::new(Instant::now() - Duration::from_secs(2)));
        let before = *last_activity.borrow();
        let meaningful_before = *last_meaningful_activity.borrow();
        let client = AcpClient::new(
            Rc::clone(&events),
            Rc::clone(&last_activity),
            Rc::clone(&last_meaningful_activity),
        );

        let commands_update =
            AvailableCommandsUpdate::new(vec![AvailableCommand::new("/help", "Get help")]);
        let notification = SessionNotification::new(
            "test-session",
            SessionUpdate::AvailableCommandsUpdate(commands_update),
        );
        client.session_notification(notification).await.unwrap();

        assert!(
            events.borrow().is_empty(),
            "protocol overhead should not produce events"
        );
        assert!(
            *last_activity.borrow() > before,
            "session_notification must refresh last_activity even for suppressed events"
        );
        assert_eq!(
            *last_meaningful_activity.borrow(),
            meaningful_before,
            "protocol-only notifications must not refresh last_meaningful_activity"
        );
    }

    #[test]
    fn session_event_store_bounds_retained_events_and_metadata() {
        let mut store = SessionEventStore::default();
        for i in 0..(MAX_RETAINED_EVENTS + 25) {
            store.push(SessionEvent::AgentMessage(format!("msg-{i}")));
        }
        store.push(SessionEvent::PlanUpdate("step".to_string()));
        for i in 0..(MAX_EXTRACTED_COMMANDS + 10) {
            store.push(SessionEvent::ToolCallStarted {
                id: format!("call-{i}"),
                title: format!("cmd-{i}"),
                kind: "Execute".to_string(),
            });
        }

        assert_eq!(store.len(), MAX_RETAINED_EVENTS);
        assert_eq!(
            store.total_events_count(),
            MAX_RETAINED_EVENTS + 25 + 1 + MAX_EXTRACTED_COMMANDS + 10
        );
        assert!(store.has_tool_calls());
        assert!(store.has_execute_tool_calls());
        assert!(store.has_plan_updates());

        let retained = store.events();
        let first_retained_message_index = store.total_events_count() - MAX_RETAINED_EVENTS;
        match retained.first() {
            Some(SessionEvent::AgentMessage(text)) => {
                assert_eq!(text, &format!("msg-{first_retained_message_index}"))
            }
            other => panic!("unexpected first retained event: {other:?}"),
        }

        let commands = store.extracted_commands();
        assert_eq!(commands.len(), MAX_EXTRACTED_COMMANDS);
        assert_eq!(commands.first().map(String::as_str), Some("cmd-10"));
        let expected_last = format!("cmd-{}", MAX_EXTRACTED_COMMANDS + 9);
        assert_eq!(
            commands.last().map(String::as_str),
            Some(expected_last.as_str())
        );
    }
}
