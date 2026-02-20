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

    pub(crate) fn push_event(&self, event: SessionEvent) {
        *self.last_activity.borrow_mut() = Instant::now();
        self.events.borrow_mut().push(event);
    }

    fn chunk_to_text(chunk: &ContentChunk) -> String {
        match &chunk.content {
            ContentBlock::Text(text) => text.text.clone(),
            other => serde_json::to_string(other).unwrap_or_else(|_| "<non-text-content>".into()),
        }
    }

    fn update_to_event(update: SessionUpdate) -> SessionEvent {
        match update {
            SessionUpdate::AgentMessageChunk(chunk) => {
                SessionEvent::AgentMessage(Self::chunk_to_text(&chunk))
            }
            SessionUpdate::AgentThoughtChunk(chunk) => {
                SessionEvent::AgentThought(Self::chunk_to_text(&chunk))
            }
            SessionUpdate::ToolCall(tool_call) => SessionEvent::ToolCallStarted {
                id: tool_call.tool_call_id.0.to_string(),
                title: tool_call.title,
                kind: format!("{:?}", tool_call.kind),
            },
            SessionUpdate::ToolCallUpdate(tool_call_update) => {
                let id = tool_call_update.tool_call_id.0.to_string();
                if let Some(status) = tool_call_update.fields.status {
                    SessionEvent::ToolCallCompleted {
                        id,
                        status: format!("{:?}", status),
                    }
                } else {
                    SessionEvent::Other(
                        serde_json::to_string(&tool_call_update)
                            .unwrap_or_else(|_| "tool_call_update".into()),
                    )
                }
            }
            SessionUpdate::Plan(plan) => {
                let serialized = serde_json::to_string(&plan)
                    .unwrap_or_else(|_| "<plan-serialize-failed>".into());
                SessionEvent::PlanUpdate(serialized)
            }
            other => SessionEvent::Other(
                serde_json::to_string(&other).unwrap_or_else(|_| "<unknown-update>".into()),
            ),
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
        self.push_event(Self::update_to_event(args.update));
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
        let event = AcpClient::update_to_event(SessionUpdate::AgentMessageChunk(chunk));

        match event {
            SessionEvent::AgentMessage(text) => assert_eq!(text, "hello"),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn test_update_to_event_agent_thought_chunk() {
        let chunk = ContentChunk::new(ContentBlock::Text(TextContent::new("thinking")));
        let event = AcpClient::update_to_event(SessionUpdate::AgentThoughtChunk(chunk));

        match event {
            SessionEvent::AgentThought(text) => assert_eq!(text, "thinking"),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn test_update_to_event_tool_call_started() {
        let tool_call = ToolCall::new("call-1", "Run tests").kind(ToolKind::Execute);
        let event = AcpClient::update_to_event(SessionUpdate::ToolCall(tool_call));

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
        let event = AcpClient::update_to_event(SessionUpdate::ToolCallUpdate(update));

        match event {
            SessionEvent::ToolCallCompleted { id, status } => {
                assert_eq!(id, "call-2");
                assert_eq!(status, "Completed");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn test_push_event_appends_to_shared_buffer() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let last_activity = Rc::new(RefCell::new(Instant::now() - Duration::from_secs(2)));
        let before = *last_activity.borrow();
        let client = AcpClient::new(Rc::clone(&events), Rc::clone(&last_activity));

        client.push_event(SessionEvent::Other("payload".to_string()));

        let stored = events.borrow();
        assert_eq!(stored.len(), 1);
        match &stored[0] {
            SessionEvent::Other(payload) => assert_eq!(payload, "payload"),
            other => panic!("unexpected stored event: {other:?}"),
        }
        assert!(
            *last_activity.borrow() > before,
            "push_event must refresh last_activity for all session events"
        );
    }
}
