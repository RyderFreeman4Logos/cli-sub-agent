use csa_acp::{SessionEvent, client::StreamingMetadata};
use csa_process::ExecutionResult;

fn codex_acp_stall_result(events: Vec<SessionEvent>) -> super::TransportResult {
    super::TransportResult {
        execution: ExecutionResult {
            summary: "initial response timeout: no ACP events/stderr for 300s; process killed"
                .to_string(),
            output: String::new(),
            stderr_output:
                "initial response timeout: no ACP events/stderr for 300s; process killed"
                    .to_string(),
            exit_code: 137,
            peak_memory_mb: None,
        },
        provider_session_id: Some("provider-session".to_string()),
        events,
        metadata: StreamingMetadata::default(),
    }
}

#[test]
fn codex_acp_stall_classifier_detects_no_event_after_init() {
    let result = codex_acp_stall_result(vec![SessionEvent::Other(
        "availableCommandsUpdate".to_string(),
    )]);

    let classification =
        super::transport_acp_crash_retry::classify_codex_acp_initial_stall(&result, Some(300))
            .expect("stall should classify");

    assert_eq!(classification.timeout_seconds, 300);
}

#[test]
fn codex_acp_stall_classifier_ignores_prompt_events_after_first_chunk() {
    for events in [
        vec![SessionEvent::AgentMessage("hello".to_string())],
        vec![SessionEvent::AgentThought("thinking".to_string())],
        vec![SessionEvent::PlanUpdate("plan".to_string())],
        vec![SessionEvent::ToolCallStarted {
            id: "call-1".to_string(),
            title: "Run tests".to_string(),
            kind: "execute".to_string(),
        }],
        vec![SessionEvent::ToolCallCompleted {
            id: "call-1".to_string(),
            status: "completed".to_string(),
        }],
    ] {
        let result = codex_acp_stall_result(events.clone());
        assert!(
            super::transport_acp_crash_retry::classify_codex_acp_initial_stall(&result, Some(300))
                .is_none(),
            "initial-response classifier must ignore real prompt progress: {events:?}"
        );
    }
}

#[test]
fn codex_acp_stall_retry_budget_respected() {
    let classification = super::transport_acp_crash_retry::CodexAcpInitialStallClassification {
        timeout_seconds: 300,
    };
    let mut execution = ExecutionResult {
        summary: "initial response timeout: raw".to_string(),
        output: String::new(),
        stderr_output: "initial response timeout: raw".to_string(),
        exit_code: 137,
        peak_memory_mb: None,
    };

    super::transport_acp_crash_retry::apply_codex_acp_initial_stall_summary(
        &mut execution,
        &classification,
        true,
    );

    assert!(
        execution
            .summary
            .contains("codex_acp_initial_stall: no AgentMessageChunk/AgentThought/PlanUpdate/ToolCall event within 300s"),
        "terminal stall should be rewritten with the codex ACP reason"
    );
    assert!(
        execution.summary.contains("retry_attempted=true"),
        "terminal summary should preserve that the retry budget was spent"
    );
}
