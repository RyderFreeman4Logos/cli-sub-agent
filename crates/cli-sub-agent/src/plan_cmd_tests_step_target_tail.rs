#[test]
fn resolve_step_tool_weave_returns_include_marker() {
    let step = PlanStep {
        id: 1,
        title: "include".into(),
        tool: Some("weave".into()),
        prompt: String::new(),
        tier: None,
        depends_on: vec![],
        on_fail: FailAction::Abort,
        condition: None,
        loop_var: None,
        session: None,
        workspace_access: None,
    };
    let target = resolve_step_tool(&step, None, None, None).unwrap();
    assert!(matches!(target, StepTarget::WeaveInclude));
}

#[test]
fn resolve_step_tool_unknown_tool_errors() {
    let step = PlanStep {
        id: 1,
        title: "test".into(),
        tool: Some("nonexistent".into()),
        prompt: String::new(),
        tier: None,
        depends_on: vec![],
        on_fail: FailAction::Abort,
        condition: None,
        loop_var: None,
        session: None,
        workspace_access: None,
    };
    assert!(resolve_step_tool(&step, None, None, None).is_err());
}
