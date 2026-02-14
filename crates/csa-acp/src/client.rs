use std::{cell::RefCell, rc::Rc};

use agent_client_protocol::{
    Client, ContentBlock, ContentChunk, RequestPermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse, SelectedPermissionOutcome, SessionNotification, SessionUpdate,
};

/// Streaming session events collected from ACP notifications.
#[derive(Debug, Clone)]
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

#[derive(Debug, Clone)]
pub(crate) struct AcpClient {
    events: SharedEvents,
}

impl AcpClient {
    pub(crate) fn new(events: SharedEvents) -> Self {
        Self { events }
    }

    pub(crate) fn push_event(&self, event: SessionEvent) {
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
