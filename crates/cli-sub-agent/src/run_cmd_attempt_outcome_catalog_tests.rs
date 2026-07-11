use super::*;

#[test]
fn cross_model_retry_clears_non_fork_effective_session() {
    let mut failover_context = None;
    let mut tool = ToolName::Codex;
    let mut model_spec = Some("codex/test-provider/model-a/high".to_string());
    let mut model = None;
    let mut fork_resolution = None;
    let mut effective_session = Some("01OLDSESSION".to_string());
    let mut session_parent = None;
    let mut session_creation_mode = crate::pipeline::SessionCreationMode::DaemonManaged;
    let mut executed_session_id = Some("01OLDSESSION".to_string());

    AttemptRetryState {
        failover_context: &mut failover_context,
        tool: &mut tool,
        model_spec: &mut model_spec,
        model: &mut model,
        fork_resolution: &mut fork_resolution,
        effective_session: &mut effective_session,
        session_parent: &mut session_parent,
        session_creation_mode: &mut session_creation_mode,
        executed_session_id: &mut executed_session_id,
        is_fork: false,
    }
    .apply(AttemptRetryAction::Retry {
        new_tool: ToolName::Codex,
        new_model_spec: Some("codex/test-provider/model-b/high".to_string()),
        failover_context: FailoverContextUpdate::Replace(Some(
            "structured failover handoff".to_string(),
        )),
        source_session_id: Some("01OLDSESSION".to_string()),
    });

    assert_eq!(effective_session, None);
    assert_eq!(session_parent.as_deref(), Some("01OLDSESSION"));
    assert_eq!(
        session_creation_mode,
        crate::pipeline::SessionCreationMode::FreshChild
    );
    assert_eq!(executed_session_id, None);
    assert_eq!(
        model_spec.as_deref(),
        Some("codex/test-provider/model-b/high")
    );
    assert_eq!(
        failover_context.as_deref(),
        Some("structured failover handoff")
    );
}

#[test]
fn slot_failover_identity_uses_fresh_linked_session() {
    let mut failover_context = None;
    let mut tool = ToolName::Codex;
    let mut model_spec = Some("codex/openai/model-a/high".to_string());
    let mut model = Some("stale-override".to_string());
    let mut fork_resolution = None;
    let mut effective_session = Some("01SOURCE".to_string());
    let mut session_parent = None;
    let mut session_creation_mode = crate::pipeline::SessionCreationMode::DaemonManaged;
    let mut executed_session_id = Some("01SOURCE".to_string());

    AttemptRetryState {
        failover_context: &mut failover_context,
        tool: &mut tool,
        model_spec: &mut model_spec,
        model: &mut model,
        fork_resolution: &mut fork_resolution,
        effective_session: &mut effective_session,
        session_parent: &mut session_parent,
        session_creation_mode: &mut session_creation_mode,
        executed_session_id: &mut executed_session_id,
        is_fork: false,
    }
    .apply(AttemptRetryAction::Retry {
        new_tool: ToolName::Opencode,
        new_model_spec: Some("opencode/google/gemini-2.5-pro/high".to_string()),
        failover_context: FailoverContextUpdate::Preserve,
        source_session_id: Some("01SOURCE".to_string()),
    });

    assert_eq!(tool, ToolName::Opencode);
    assert_eq!(
        model_spec.as_deref(),
        Some("opencode/google/gemini-2.5-pro/high")
    );
    assert_eq!(model, None);
    assert_eq!(effective_session, None);
    assert_eq!(session_parent.as_deref(), Some("01SOURCE"));
    assert_eq!(
        session_creation_mode,
        crate::pipeline::SessionCreationMode::FreshChild
    );
    assert_eq!(executed_session_id, None);
}
