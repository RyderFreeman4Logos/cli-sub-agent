#[test]
fn initial_response_event_filter_accepts_agent_plan_and_tool_events() {
    for event in [
        SessionEvent::AgentMessage("msg".to_string()),
        SessionEvent::AgentThought("thought".to_string()),
        SessionEvent::PlanUpdate("plan".to_string()),
        SessionEvent::ToolCallStarted {
            id: "tool-1".to_string(),
            title: "Run".to_string(),
            kind: "execute".to_string(),
        },
        SessionEvent::ToolCallCompleted {
            id: "tool-1".to_string(),
            status: "completed".to_string(),
        },
    ] {
        assert!(
            crate::client::event_counts_as_initial_response(&event),
            "event should count as initial-response progress: {event:?}"
        );
    }
}

#[test]
fn initial_response_event_filter_ignores_other_events() {
    assert!(
        !crate::client::event_counts_as_initial_response(&SessionEvent::Other(
            "protocol overhead".to_string()
        )),
        "protocol-only events must not satisfy the initial-response watchdog"
    );
}

#[test]
fn stream_new_agent_messages_reports_initial_response_progress_only_for_eligible_events() {
    let events = shared_events(vec![SessionEvent::Other("overhead".to_string())]);
    let mut index = 0;
    let mut spool: Option<SpoolRotator> = None;
    let mut metadata = StreamingMetadata::default();

    assert!(
        !stream_new_agent_messages(
            &events,
            &mut index,
            false,
            &mut spool,
            &mut metadata,
            &mut String::new(),
            &mut String::new(),
        ),
        "Other-only batches must not count as initial-response progress"
    );

    events
        .borrow_mut()
        .push(SessionEvent::AgentMessage("hello".to_string()));
    assert!(
        stream_new_agent_messages(
            &events,
            &mut index,
            false,
            &mut spool,
            &mut metadata,
            &mut String::new(),
            &mut String::new(),
        ),
        "AgentMessage must satisfy initial-response progress"
    );
}
