use std::time::Instant;
use std::{cell::RefCell, rc::Rc};

use agent_client_protocol::{
    Client, ContentBlock, ContentChunk, RequestPermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse, SelectedPermissionOutcome, SessionNotification, SessionUpdate,
};

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

pub(crate) type SharedEvents = Rc<RefCell<Vec<SessionEvent>>>;
pub(crate) type SharedActivity = Rc<RefCell<Instant>>;

#[derive(Debug, Clone)]
pub(crate) struct AcpClient {
    events: SharedEvents,
    last_activity: SharedActivity,
}

impl AcpClient {
    pub(crate) fn new(events: SharedEvents, last_activity: SharedActivity) -> Self {
        Self {
            events,
            last_activity,
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
                        status: format!("{:?}", status),
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
        // Always refresh activity timestamp so idle-timeout considers
        // protocol-level traffic as proof of liveness, even when the
        // event itself is suppressed from collected output.
        *self.last_activity.borrow_mut() = Instant::now();
        if let Some(event) = Self::update_to_event(args.update) {
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

    use super::{AcpClient, SessionEvent};

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

        let events = Rc::new(RefCell::new(Vec::new()));
        let last_activity = Rc::new(RefCell::new(Instant::now() - Duration::from_secs(2)));
        let before = *last_activity.borrow();
        let client = AcpClient::new(Rc::clone(&events), Rc::clone(&last_activity));

        let chunk = ContentChunk::new(ContentBlock::Text(TextContent::new("hello")));
        let notification =
            SessionNotification::new("test-session", SessionUpdate::AgentMessageChunk(chunk));
        client.session_notification(notification).await.unwrap();

        let stored = events.borrow();
        assert_eq!(stored.len(), 1);
        match &stored[0] {
            SessionEvent::AgentMessage(text) => assert_eq!(text, "hello"),
            other => panic!("unexpected stored event: {other:?}"),
        }
        assert!(
            *last_activity.borrow() > before,
            "session_notification must refresh last_activity"
        );
    }

    #[tokio::test]
    async fn test_session_notification_suppresses_protocol_event_but_refreshes_activity() {
        use agent_client_protocol::{
            AvailableCommand, AvailableCommandsUpdate, Client, SessionNotification,
        };

        let events = Rc::new(RefCell::new(Vec::new()));
        let last_activity = Rc::new(RefCell::new(Instant::now() - Duration::from_secs(2)));
        let before = *last_activity.borrow();
        let client = AcpClient::new(Rc::clone(&events), Rc::clone(&last_activity));

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
    }
}
