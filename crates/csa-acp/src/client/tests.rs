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
    command_looks_like_no_verify_commit,
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
    let last_meaningful_activity = Rc::new(RefCell::new(Instant::now() - Duration::from_secs(2)));
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
    let last_meaningful_activity = Rc::new(RefCell::new(Instant::now() - Duration::from_secs(2)));
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

#[test]
fn command_looks_like_no_verify_commit_ignores_message_values_starting_with_dash() {
    assert!(!command_looks_like_no_verify_commit(
        "git commit -m 'msg' -m '- Verification: pre-commit'"
    ));
}

#[test]
fn command_looks_like_no_verify_commit_detects_real_no_verify_flags() {
    assert!(command_looks_like_no_verify_commit(
        "git commit -m 'msg' -n"
    ));
    assert!(!command_looks_like_no_verify_commit("git commit -am 'msg'"));
    assert!(command_looks_like_no_verify_commit("git commit -nm 'msg'"));
}

#[test]
fn command_looks_like_no_verify_commit_stops_at_shell_operators() {
    assert!(!command_looks_like_no_verify_commit(
        "git commit -m msg && echo -n ok"
    ));
}

#[test]
fn command_looks_like_no_verify_commit_detects_quoted_flags() {
    assert!(command_looks_like_no_verify_commit("git commit \"-n\""));
}

#[test]
fn command_looks_like_no_verify_commit_detects_shell_wrapped_commits() {
    assert!(command_looks_like_no_verify_commit(
        "bash -lc \"git commit -n -m unsafe\""
    ));
}

#[test]
fn command_looks_like_no_verify_commit_treats_newline_as_command_separator() {
    assert!(!command_looks_like_no_verify_commit(
        "git commit -m msg\necho -n ok"
    ));
}

#[test]
fn command_looks_like_no_verify_commit_detects_later_commit_in_shell_payload() {
    assert!(command_looks_like_no_verify_commit(
        "bash -lc \"git commit -m safe; git commit -n -m unsafe\""
    ));
}

#[test]
fn command_looks_like_no_verify_commit_detects_prefixed_shell_wrappers() {
    assert!(command_looks_like_no_verify_commit(
        "sudo bash -lc \"git commit -n -m unsafe\""
    ));
}
